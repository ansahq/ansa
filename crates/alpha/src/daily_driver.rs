//! The alpha composition root: one handle for durable state, catalog search,
//! quicklinks, onboarding, emoji and calculator routing.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use ansa_domain::Command;
use ansa_emoji::Diagnostics;
use ansa_onboarding::{JsonFileStore, Onboarding, OnboardingStatus, Step};
use ansa_quicklinks::{
    ImportOptions, ImportReport, Library, OpenPolicy, OpenerProbe, Preview, Quicklink, QuicklinkId,
};
use ansa_storage::{
    BackupSummary, PragmaReport, RestoreSummary, SettingKey, SettingSnapshot, SettingValue,
    StorageConfig, Store,
};

use crate::catalog::{
    CatalogComposition, EmojiHit, SearchOutcome, compose_catalog, search_catalog,
};
use crate::error::AlphaError;

/// Durable record identity of the versioned quicklink library.
pub const QUICKLINK_RECORD_ID: &str = "quicklink-library";
/// Durable record kind of the versioned quicklink library.
pub const QUICKLINK_RECORD_KIND: &str = "quicklink.library";
/// Main SQLite database file inside the data directory.
pub const MAIN_DATABASE_FILE: &str = "ansa.sqlite";
/// Rebuildable cache database file inside the data directory.
pub const CACHE_DATABASE_FILE: &str = "cache.sqlite";
/// Backup file written by [`DailyDriver::backup_to`] and the CLI.
pub const BACKUP_DATABASE_FILE: &str = "backup.sqlite";
/// Onboarding JSON state file inside the data directory.
pub const ONBOARDING_FILE: &str = "onboarding.json";

/// The single composition root used by the CLI and the alpha harness.
///
/// It owns the durable [`Store`], the in-memory quicklink [`Library`] together
/// with the durable record revision used for compare-and-swap saves, and the
/// onboarding state path. It performs no network access and never launches an
/// application.
pub struct DailyDriver {
    data_dir: PathBuf,
    store: Store,
    quicklinks: Library,
    quicklink_record_revision: Option<u64>,
    quicklink_warning: Option<String>,
    onboarding_path: PathBuf,
    backup_path: PathBuf,
}

impl DailyDriver {
    /// Opens (or creates) storage in `data_dir` and loads the persisted
    /// quicklink library.
    ///
    /// A corrupt quicklink record does not prevent startup. It is reported by
    /// [`DailyDriver::quicklink_warning`] and an empty in-memory library is used
    /// until an explicit save replaces the record.
    pub fn open(data_dir: impl Into<PathBuf>) -> Result<Self, AlphaError> {
        let data_dir = data_dir.into();
        std::fs::create_dir_all(&data_dir)?;
        let config = StorageConfig::new(
            data_dir.join(MAIN_DATABASE_FILE),
            data_dir.join(CACHE_DATABASE_FILE),
        );
        let store = Store::open(config)?;
        let mut driver = Self {
            onboarding_path: data_dir.join(ONBOARDING_FILE),
            backup_path: data_dir.join(BACKUP_DATABASE_FILE),
            data_dir,
            store,
            quicklinks: Library::new(),
            quicklink_record_revision: None,
            quicklink_warning: None,
        };
        driver.reload_quicklinks()?;
        Ok(driver)
    }

    /// Data directory supplied at open time.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Durable database path.
    pub fn main_database_path(&self) -> &Path {
        self.store.main_path()
    }

    /// Default backup path inside the data directory.
    pub fn backup_path(&self) -> &Path {
        &self.backup_path
    }

    /// Onboarding state path inside the data directory.
    pub fn onboarding_path(&self) -> &Path {
        &self.onboarding_path
    }

    /// Current schema version.
    pub fn schema_version(&self) -> u32 {
        self.store.schema_version()
    }

    /// Writer pragmas as observed by the storage actor.
    pub fn pragma_report(&self) -> Result<PragmaReport, AlphaError> {
        Ok(self.store.pragma_report()?)
    }

    /// Read-only storage handle.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Mutable storage handle, required for restore.
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    /// The currently loaded quicklink library.
    pub fn quicklinks(&self) -> &Library {
        &self.quicklinks
    }

    /// Warning from the last quicklink load, if the record was unusable.
    pub fn quicklink_warning(&self) -> Option<&str> {
        self.quicklink_warning.as_deref()
    }

    /// Current durable revision of the quicklink record, if one exists.
    pub fn quicklink_record_revision(&self) -> Option<u64> {
        self.quicklink_record_revision
    }

    /// Reads a typed setting.
    pub fn get_setting(&self, key: SettingKey) -> Result<Option<SettingSnapshot>, AlphaError> {
        Ok(self.store.get_setting(key)?)
    }

    /// Reads a typed setting or its safe default.
    pub fn get_or_default(&self, key: SettingKey) -> Result<SettingSnapshot, AlphaError> {
        Ok(self.store.get_or_default(key)?)
    }

    /// Compare-and-swap setting write.
    pub fn set_setting(
        &self,
        key: SettingKey,
        value: &SettingValue,
        expected_revision: Option<u64>,
    ) -> Result<u64, AlphaError> {
        Ok(self.store.set_setting(key, value, expected_revision)?)
    }

    /// Backs up the durable database and verifies the copy.
    pub fn backup_to(&self, path: impl AsRef<Path>) -> Result<BackupSummary, AlphaError> {
        Ok(self.store.backup_to(path)?)
    }

    /// Restores a verified backup and reloads the quicklink library from it.
    pub fn restore_from(&mut self, path: impl AsRef<Path>) -> Result<RestoreSummary, AlphaError> {
        let summary = self.store.restore_from(path)?;
        self.reload_quicklinks()?;
        Ok(summary)
    }

    /// Rebuilds the searchable catalog from live discovery plus persisted
    /// launchable quicklinks.
    pub fn discover_catalog(&self) -> Result<CatalogComposition, AlphaError> {
        let apps = ansa_platform_windows::discover_applications().map_err(AlphaError::Catalog)?;
        compose_catalog(&self.quicklinks, &apps)
    }

    /// Composes a catalog from an already-discovered application list.
    ///
    /// Quicklinks that cannot be launched directly and safely are omitted and
    /// reported in [`CatalogComposition::skipped_quicklinks`].
    pub fn compose_catalog(
        &self,
        apps: &[ansa_platform_windows::AppEntry],
    ) -> Result<CatalogComposition, AlphaError> {
        compose_catalog(&self.quicklinks, apps)
    }

    /// Runs one query against an already-composed catalog.
    pub fn search_catalog(
        &self,
        catalog: &[Command],
        query: &str,
        emoji_limit: usize,
    ) -> Result<SearchOutcome, AlphaError> {
        search_catalog(catalog, query, emoji_limit)
    }

    /// Discovers the live catalog and runs one query.
    ///
    /// Prefer [`DailyDriver::search_catalog`] when a catalog is already loaded;
    /// this convenience method rescans the Start Menu.
    pub fn search(&self, query: &str, emoji_limit: usize) -> Result<SearchOutcome, AlphaError> {
        let composition = self.discover_catalog()?;
        self.search_catalog(&composition.commands, query, emoji_limit)
    }

    /// Bounded emoji search.
    pub fn search_emoji(&self, query: &str, limit: usize) -> Result<Vec<EmojiHit>, AlphaError> {
        crate::catalog::search_emoji(query, limit)
    }

    /// Builds a preview for a candidate quicklink without saving it.
    pub fn preview_quicklink(
        &self,
        candidate: &Quicklink,
        values: &BTreeMap<String, String>,
        policy: OpenPolicy,
        probe: &dyn OpenerProbe,
    ) -> Result<Preview, ansa_quicklinks::PreviewError> {
        self.quicklinks.preview(candidate, values, policy, probe)
    }

    /// Commits a still-current preview and persists the library.
    pub fn commit_quicklink(&mut self, preview: &Preview) -> Result<Quicklink, AlphaError> {
        let link = self
            .quicklinks
            .commit(preview)
            .map_err(|error| AlphaError::Quicklink(error.to_string()))?;
        self.save_quicklinks()?;
        Ok(link)
    }

    /// Removes a quicklink and persists the library.
    pub fn remove_quicklink(&mut self, id: &QuicklinkId) -> Result<Quicklink, AlphaError> {
        let link = self
            .quicklinks
            .remove(id)
            .map_err(|error| AlphaError::Quicklink(error.to_string()))?;
        self.save_quicklinks()?;
        Ok(link)
    }

    /// Imports a versioned JSON library and persists every accepted item.
    pub fn import_quicklinks(
        &mut self,
        json: &str,
        options: ImportOptions,
    ) -> Result<ImportReport, AlphaError> {
        let report = self.quicklinks.import_json(json, options);
        let changed = !options.dry_run && (report.replaced || report.applied > 0);
        if changed {
            self.save_quicklinks()?;
        }
        Ok(report)
    }

    /// Serializes the loaded library as versioned JSON.
    pub fn export_quicklinks(&self) -> Result<String, AlphaError> {
        self.quicklinks
            .export_json()
            .map_err(|error| AlphaError::Quicklink(error.to_string()))
    }

    /// Loads the persisted library again, keeping any load warning.
    pub fn reload_quicklinks(&mut self) -> Result<(), AlphaError> {
        self.quicklink_warning = None;
        let Some(record) = self.store.get_record(QUICKLINK_RECORD_ID)? else {
            self.quicklinks = Library::new();
            self.quicklink_record_revision = None;
            return Ok(());
        };

        let mut library = Library::new();
        let report = library.import_json(&record.payload_json, ImportOptions::replace_confirmed());
        if report.has_errors() {
            self.quicklink_warning = Some(format!(
                "quicklink record {} (revision {}) was not used: {}",
                record.id,
                record.revision,
                report
                    .errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
            library = Library::new();
        }
        self.quicklinks = library;
        self.quicklink_record_revision = Some(record.revision);
        Ok(())
    }

    /// Persists the in-memory library through its durable record revision.
    pub fn save_quicklinks(&mut self) -> Result<(), AlphaError> {
        let payload = self.export_quicklinks()?;
        let record = match self.quicklink_record_revision {
            Some(revision) => self
                .store
                .update_record(QUICKLINK_RECORD_ID, &payload, revision)?,
            None => {
                self.store
                    .create_record(QUICKLINK_RECORD_ID, QUICKLINK_RECORD_KIND, &payload)?
            }
        };
        self.quicklink_record_revision = Some(record.revision);
        Ok(())
    }

    /// Onboarding handle backed by `<data_dir>/onboarding.json`.
    pub fn onboarding(&self) -> Onboarding {
        Onboarding::load(JsonFileStore::new(&self.onboarding_path))
    }

    /// Onboarding status for `doctor`, or `None` when no state file exists.
    pub fn onboarding_info(&self) -> Result<Option<OnboardingInfo>, AlphaError> {
        if !self.onboarding_path.exists() {
            return Ok(None);
        }
        let onboarding = self.onboarding();
        Ok(Some(OnboardingInfo {
            status: onboarding.status(),
            step: onboarding.step(),
            completion_count: onboarding.completion_count(),
            warning: onboarding.load_warning().map(str::to_owned),
            verified_launch: onboarding
                .verified_launch()
                .map(|verification| verification.command_id().to_string()),
        }))
    }
}

/// Snapshot of persisted onboarding state for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnboardingInfo {
    /// Overall progress.
    pub status: OnboardingStatus,
    /// Current step.
    pub step: Step,
    /// Number of completed onboarding runs.
    pub completion_count: u32,
    /// Warning from a corrupt or unreadable saved document.
    pub warning: Option<String>,
    /// Command identity whose launch was verified, if any.
    pub verified_launch: Option<String>,
}

/// Emoji dataset metadata used by `doctor`.
pub fn emoji_info() -> Result<Diagnostics, AlphaError> {
    ansa_emoji::diagnostics().map_err(|error| AlphaError::Emoji(error.to_string()))
}

impl fmt::Debug for DailyDriver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DailyDriver")
            .field("data_dir", &self.data_dir)
            .field("store", &self.store)
            .field("quicklinks", &self.quicklinks)
            .field("quicklink_record_revision", &self.quicklink_record_revision)
            .field("quicklink_warning", &self.quicklink_warning)
            .finish_non_exhaustive()
    }
}
