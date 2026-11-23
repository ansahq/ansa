//! Desktop-side helpers for the alpha daily-driver surfaces.
//!
//! This module keeps the GPUI integration small and testable: it resolves the
//! per-user data directory, turns the composition root into a launcher catalog,
//! parses the explicit `emoji` mode, and adapts onboarding to real host probes.
//! No storage call happens on the render path; hosts call these helpers from
//! startup, refresh and explicit user actions only.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ansa_alpha::{DailyDriver, EmojiHit, SkippedQuicklink};
use ansa_domain::{BuiltinCommand, Command, CommandId, CommandKind};
use ansa_onboarding::{
    LaunchOutcome as OnboardingLaunchOutcome, LaunchProbe, Onboarding, OnboardingStatus,
    ProbeOutcome, Shortcut, ShortcutProbe, Step,
};
use ansa_platform_api::{AppEntry, LaunchOutcome, Platform};
use ansa_storage::{LaunchMode, SettingKey, SettingValue, Theme as StoredTheme};

/// Environment variable that overrides the per-user data directory.
pub const DATA_DIR_ENV: &str = "ANSA_DATA_DIR";

/// Stable desktop-only identity used to open the setup surface.
pub const SETUP_COMMAND_ID: &str = "ansa:setup";

/// Maximum emoji results rendered in the desktop mode.
pub const EMOJI_LIMIT: usize = 24;

/// Resolves the data directory using an injectable environment lookup.
///
/// Precedence: `ANSA_DATA_DIR`, the platform per-user application data
/// directory, then a relative fallback for unusual environments.
pub fn resolve_data_dir(env: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    if let Some(value) = env(DATA_DIR_ENV).filter(|value| !is_blank(value)) {
        return PathBuf::from(value);
    }
    platform_data_dir(&env).unwrap_or_else(|| PathBuf::from(".ansa-alpha"))
}

/// Resolves the data directory from the real process environment.
pub fn default_data_dir() -> PathBuf {
    resolve_data_dir(|key| std::env::var_os(key))
}

/// Creates a disposable data directory for smoke and fixture runs.
///
/// The path is under the operating system's temporary directory and includes
/// the process id plus a timestamp so concurrent smoke runs do not collide.
pub fn smoke_data_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!("ansa-smoke-{}-{nanos}", std::process::id()))
}

fn is_blank(value: &OsString) -> bool {
    value.to_string_lossy().trim().is_empty()
}

#[cfg(windows)]
fn platform_data_dir(env: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    env("LOCALAPPDATA")
        .filter(|value| !is_blank(value))
        .map(|value| PathBuf::from(value).join("Ansa").join("alpha"))
}

#[cfg(target_os = "macos")]
fn platform_data_dir(env: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    env("HOME").filter(|value| !is_blank(value)).map(|value| {
        PathBuf::from(value)
            .join("Library")
            .join("Application Support")
            .join("Ansa")
            .join("alpha")
    })
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_data_dir(env: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(path) = env("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Some(path.join("Ansa").join("alpha"));
    }
    env("HOME")
        .filter(|value| !is_blank(value))
        .map(|value| PathBuf::from(value).join(".local/share/Ansa/alpha"))
}

fn builtin(id: &str, title: &str, subtitle: &str, kind: BuiltinCommand) -> Command {
    Command {
        id: CommandId::new(id).expect("static command ID"),
        title: title.into(),
        subtitle: Some(subtitle.into()),
        keywords: vec!["ansa".into()],
        alias: None,
        kind: CommandKind::Builtin(kind),
    }
}

/// The desktop-only setup entry point.
///
/// The kind intentionally reuses a reviewed built-in: executing it is always
/// intercepted by the launcher by identity, so this never changes the domain
/// vocabulary owned by other crates.
pub fn setup_command() -> Command {
    builtin(
        SETUP_COMMAND_ID,
        "Start Setup",
        "First-run setup, shortcut check and a real launch test",
        BuiltinCommand::Settings,
    )
}

/// Built-in commands shown before or instead of a composed catalog.
pub fn builtin_commands() -> Vec<Command> {
    vec![
        builtin(
            "ansa:refresh",
            "Refresh Applications",
            "Rescan the application catalog and quicklinks",
            BuiltinCommand::RefreshApplications,
        ),
        builtin(
            "ansa:settings",
            "Integration Status",
            "See what this alpha build supports",
            BuiltinCommand::Settings,
        ),
        builtin(
            "ansa:about",
            "About Ansa",
            "An open-source shortcut to your everyday",
            BuiltinCommand::About,
        ),
        setup_command(),
    ]
}

fn app_commands(apps: &[AppEntry]) -> Vec<Command> {
    apps.iter()
        .filter_map(|app| {
            let id = CommandId::new(&app.id).ok()?;
            let subtitle = if let Some(reason) = &app.broken {
                let kind = if cfg!(windows) {
                    "Broken shortcut"
                } else {
                    "Unavailable application"
                };
                format!("{kind} · {reason} · refresh required")
            } else if app.packaged {
                "Packaged application · Windows".to_owned()
            } else {
                match Platform::current() {
                    Platform::Windows => "Application · Windows Start Menu",
                    Platform::MacOs => "Application · macOS",
                    Platform::Linux => "Application · Linux desktop entry",
                    Platform::Other => "Application",
                }
                .to_owned()
            };
            let mut keywords = vec!["application".to_owned(), "app".to_owned()];
            if app.packaged {
                keywords.push("packaged".to_owned());
            }
            Some(
                Command::new(
                    id,
                    app.title.clone(),
                    CommandKind::Application {
                        path: app.path.clone(),
                    },
                )
                .with_subtitle(subtitle)
                .with_keywords(keywords),
            )
        })
        .collect()
}

#[cfg(any(windows, test))]
fn with_setup_command(mut commands: Vec<Command>) -> Vec<Command> {
    if !commands
        .iter()
        .any(|command| command.id.as_str() == SETUP_COMMAND_ID)
    {
        commands.push(setup_command());
    }
    commands
}

/// Result of loading the launcher catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogLoad {
    /// Searchable commands, including built-ins and the setup entry point.
    pub commands: Vec<Command>,
    /// Number of discovered application entries.
    pub applications: usize,
    /// Number of enabled, directly launchable quicklinks in the catalog.
    pub quicklinks: usize,
    /// Enabled quicklinks omitted from the catalog with the reason.
    pub skipped: Vec<SkippedQuicklink>,
    /// Non-fatal warning from a fallback composition.
    pub warning: Option<String>,
}

impl CatalogLoad {
    /// Compact status-line summary with explicit counts.
    pub fn summary(&self) -> String {
        let mut text = format!("{} applications", self.applications);
        if self.quicklinks > 0 {
            let label = if self.quicklinks == 1 {
                "quicklink"
            } else {
                "quicklinks"
            };
            text.push_str(&format!(" · {} {label}", self.quicklinks));
        }
        if !self.skipped.is_empty() {
            let label = if self.skipped.len() == 1 {
                "quicklink omitted"
            } else {
                "quicklinks omitted"
            };
            text.push_str(&format!(" · {} {label}", self.skipped.len()));
        }
        text.push_str(" · local only");
        text
    }

    /// Appends a non-fatal warning for the UI banner.
    pub fn add_warning(&mut self, warning: impl Into<String>) {
        let warning = warning.into();
        self.warning = Some(match self.warning.take() {
            Some(existing) if !existing.is_empty() => format!("{existing} {warning}"),
            _ => warning,
        });
    }
}

/// Persisted preference values shown by the desktop.
///
/// The desktop deliberately does not apply all of them yet. Showing them keeps
/// the settings surface truthful instead of implying that stored theme,
/// launch-mode or maximum-results values drive this build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedSettings {
    /// Stored activation shortcut.
    pub hotkey: String,
    /// Stored appearance preference.
    pub theme: String,
    /// Stored result bound.
    pub maximum_results: String,
    /// Stored launch mode.
    pub launch_mode: String,
    /// Stored alias prefix.
    pub alias_prefix: String,
    /// Per-key diagnostics for unreadable stored values.
    pub diagnostics: Vec<String>,
}

impl PersistedSettings {
    /// Compact multi-line summary for Integration Status.
    pub fn summary(&self) -> String {
        let mut text = format!(
            "hotkey={} · theme={} · maximum_results={} · launch_mode={} · alias_prefix={}",
            self.hotkey, self.theme, self.maximum_results, self.launch_mode, self.alias_prefix
        );
        if !self.diagnostics.is_empty() {
            text.push_str(&format!(" · issues: {}", self.diagnostics.join("; ")));
        }
        text
    }
}

/// Reads typed settings with their safe defaults.
pub fn read_persisted_settings(driver: &DailyDriver) -> PersistedSettings {
    let mut diagnostics = Vec::new();
    let read = |key: SettingKey, diagnostics: &mut Vec<String>| match driver.get_or_default(key) {
        Ok(snapshot) => {
            if let Some(diagnostic) = &snapshot.diagnostic {
                diagnostics.push(format!("{}: {diagnostic}", key.as_str()));
            }
            describe_setting_value(&snapshot.value)
        }
        Err(error) => {
            diagnostics.push(format!("{}: {error}", key.as_str()));
            "unavailable".to_owned()
        }
    };

    PersistedSettings {
        hotkey: read(SettingKey::Hotkey, &mut diagnostics),
        theme: read(SettingKey::Theme, &mut diagnostics),
        maximum_results: read(SettingKey::MaximumResults, &mut diagnostics),
        launch_mode: read(SettingKey::LaunchMode, &mut diagnostics),
        alias_prefix: read(SettingKey::AliasPrefix, &mut diagnostics),
        diagnostics,
    }
}

fn describe_setting_value(value: &SettingValue) -> String {
    match value {
        SettingValue::Hotkey(value) => value.clone(),
        SettingValue::AliasPrefix(value) => value.clone(),
        SettingValue::Theme(StoredTheme::System) => "system".to_owned(),
        SettingValue::Theme(StoredTheme::Light) => "light".to_owned(),
        SettingValue::Theme(StoredTheme::Dark) => "dark".to_owned(),
        SettingValue::LaunchMode(LaunchMode::Toggle) => "toggle".to_owned(),
        SettingValue::LaunchMode(LaunchMode::Show) => "show".to_owned(),
        SettingValue::MaximumResults(limit) => limit.to_string(),
    }
}

/// One-line warning naming the first omitted quicklinks and why.
///
/// Reasons come from the alpha composition root: unresolved templates, local
/// paths, unsupported openers, placeholder targets and unsafe/unavailable
/// openers are never represented as launchable commands.
pub fn skipped_warning(skipped: &[SkippedQuicklink]) -> Option<String> {
    if skipped.is_empty() {
        return None;
    }
    const SHOWN: usize = 3;
    let details = skipped
        .iter()
        .take(SHOWN)
        .map(|link| format!("{}: {}", link.title, link.reason))
        .collect::<Vec<_>>()
        .join("; ");
    let more = skipped.len().saturating_sub(SHOWN);
    let more = if more > 0 {
        format!("; +{more} more")
    } else {
        String::new()
    };
    let label = if skipped.len() == 1 {
        "quicklink"
    } else {
        "quicklinks"
    };
    Some(format!(
        "{} {label} omitted from the catalog — {details}{more}. Raw templates are never launched.",
        skipped.len()
    ))
}

/// Loads the catalog from an opened driver, falling back to the legacy catalog
/// when composition fails.
///
/// A corrupt quicklink record does not fail composition: the driver exposes it
/// through `quicklink_warning`, and the desktop must surface that warning
/// instead of pretending the library was simply empty.
pub fn compose_catalog(driver: &DailyDriver, apps: &[AppEntry]) -> Result<CatalogLoad, String> {
    #[cfg(windows)]
    let composition = driver
        .compose_catalog(
            &apps
                .iter()
                .map(|app| ansa_platform_windows::AppEntry {
                    id: app.id.clone(),
                    title: app.title.clone(),
                    path: app.path.clone(),
                    target: app.target.clone(),
                    arguments: app.arguments.clone(),
                    working_directory: app.working_directory.clone(),
                    icon_source: app.icon_source.clone(),
                    packaged: app.packaged,
                    broken: app.broken.clone(),
                })
                .collect::<Vec<_>>(),
        )
        .map_err(|error| error.to_string())?;
    // Alpha's application input and labels still belong to its Windows
    // slice. Reuse its saved quicklink policy without passing Unix entries
    // through that adapter or duplicating target validation.
    #[cfg(not(windows))]
    let composition = driver
        .compose_catalog(&[])
        .map_err(|error| error.to_string())?;
    #[cfg(not(windows))]
    let commands = builtin_commands()
        .into_iter()
        .chain(app_commands(apps))
        .chain(
            composition
                .commands
                .into_iter()
                .filter(|command| matches!(command.kind, CommandKind::Quicklink { .. })),
        )
        .collect();
    #[cfg(windows)]
    let commands = with_setup_command(composition.commands);
    let quicklinks = commands
        .iter()
        .filter(|command| matches!(command.kind, CommandKind::Quicklink { .. }))
        .count();
    Ok(CatalogLoad {
        commands,
        applications: apps.len(),
        quicklinks,
        skipped: composition.skipped_quicklinks,
        warning: driver
            .quicklink_warning()
            .map(|warning| format!("Quicklink library warning: {warning}")),
    })
}

/// Reloads the in-memory quicklink library from the store, then composes the
/// catalog.
///
/// This is for same-process state changes only. The desktop holds the storage
/// lock for its lifetime, so a separate `ansa quicklink import --data-dir`
/// cannot run while Ansa is live; that workflow is close Ansa, import, start
/// Ansa. Reload errors are reported as a warning while the previously loaded
/// library remains usable.
pub fn reload_and_compose(
    driver: &mut DailyDriver,
    apps: &[AppEntry],
) -> Result<CatalogLoad, String> {
    let reload_warning = driver
        .reload_quicklinks()
        .err()
        .map(|error| format!("Quicklink reload failed: {error}"));
    let mut load = compose_catalog(driver, apps)?;
    if let Some(warning) = reload_warning {
        load.add_warning(warning);
    }
    Ok(load)
}

/// The pre-alpha behavior: built-ins plus discovered applications, with no
/// quicklinks. Used when storage cannot be opened or composition fails.
pub fn legacy_catalog(apps: &[AppEntry]) -> CatalogLoad {
    let mut commands = builtin_commands();
    commands.extend(app_commands(apps));
    CatalogLoad {
        commands,
        applications: apps.len(),
        quicklinks: 0,
        skipped: Vec::new(),
        warning: None,
    }
}

/// Refreshes the shared catalog without losing access to saved quicklinks on
/// a macOS/Linux profile whose native app discovery is unavailable.
pub fn refresh_catalog(
    driver: Option<&mut DailyDriver>,
    discovery: Result<Vec<AppEntry>, String>,
) -> Result<CatalogLoad, String> {
    let mut discovery_warning = None;
    let apps = match discovery {
        Ok(apps) => apps,
        Err(error) => {
            let warning = format!("Application discovery failed: {error}");
            if cfg!(windows) {
                return Err(warning);
            }
            discovery_warning = Some(warning);
            Vec::new()
        }
    };
    let mut load = match driver {
        Some(driver) => reload_and_compose(driver, &apps).unwrap_or_else(|error| {
            let mut load = legacy_catalog(&apps);
            load.add_warning(format!(
                "Quicklink composition failed; using built-ins and applications only: {error}"
            ));
            load
        }),
        None => legacy_catalog(&apps),
    };
    if let Some(warning) = discovery_warning {
        load.add_warning(warning);
    }
    Ok(load)
}

/// Actionable recovery line for a storage failure.
pub fn storage_recovery(data_dir: &Path, error: &str) -> String {
    format!(
        "Storage unavailable: {error}. Using built-ins and discovered applications only. Data directory: {}. Close another Ansa instance, or move a corrupt or newer database aside, then refresh.",
        data_dir.display()
    )
}

/// Parses the explicit emoji mode.
///
/// `emoji smile`, `emoji: smile` and bare `emoji` (browse) are recognized.
/// Any other query, including a command whose name merely contains "emoji",
/// stays a normal catalog search.
pub fn emoji_mode_query(query: &str) -> Option<&str> {
    let trimmed = query.trim_start();
    let prefix = trimmed.get(..5)?;
    if prefix != "emoji" {
        return None;
    }
    let rest = &trimmed[5..];
    if rest.is_empty() {
        return Some("");
    }
    if let Some(rest) = rest.strip_prefix(':') {
        return Some(rest.trim());
    }
    if rest.chars().next().is_some_and(char::is_whitespace) {
        Some(rest.trim())
    } else {
        None
    }
}

/// Searches the embedded emoji dataset with the desktop bound.
pub fn search_emoji(query: &str, limit: usize) -> Result<Vec<EmojiHit>, String> {
    ansa_alpha::search_emoji(query, limit.min(EMOJI_LIMIT)).map_err(|error| error.to_string())
}

/// A read-only snapshot of onboarding for rendering and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnboardingView {
    /// Current step.
    pub step: Step,
    /// Overall status.
    pub status: OnboardingStatus,
    /// Step heading.
    pub title: String,
    /// Shortcut state, including whether the registered host hotkey backs it.
    pub shortcut: String,
    /// Whether a launch has been verified.
    pub launch: String,
    /// Persisted warnings for skipped or fallback steps.
    pub warnings: Vec<String>,
    /// A warning from a corrupt or unreadable saved document.
    pub load_warning: Option<String>,
}

impl OnboardingView {
    /// True when setup is finished.
    pub fn is_completed(&self) -> bool {
        self.status.is_completed()
    }
}

/// Describes onboarding state without performing any host action.
pub fn describe_onboarding(onboarding: &Onboarding, hotkey_registered: bool) -> OnboardingView {
    let session = if hotkey_registered {
        "this session: registered"
    } else {
        "this session: not registered"
    };
    let shortcut = match onboarding.shortcut_choice() {
        Some(choice) => format!(
            "{} — {} · {session}",
            choice.shortcut().display(),
            choice.verification().label()
        ),
        None if hotkey_registered => {
            "Ctrl+Space is registered; continue to record it for setup".to_owned()
        }
        None => format!("Ctrl+Space is not registered · {session}"),
    };
    let launch = match onboarding.verified_launch() {
        Some(verification) => {
            let label = if cfg!(windows) {
                "Verified launch"
            } else {
                "Recorded launch handoff or verified application switch"
            };
            format!("{label}: {}", verification.command_id())
        }
        None if onboarding.is_skipped(Step::TryLaunch) => {
            "Skipped — no application launch was verified".to_owned()
        }
        None => "No launch verified yet".to_owned(),
    };
    let warnings = Step::ALL
        .into_iter()
        .filter_map(|step| {
            onboarding
                .warning_for(step)
                .map(|warning| format!("{step}: {warning}"))
        })
        .collect();

    OnboardingView {
        step: onboarding.step(),
        status: onboarding.status(),
        title: onboarding.step().title().to_owned(),
        shortcut,
        launch,
        warnings,
        load_warning: onboarding.load_warning().map(str::to_owned),
    }
}

/// One keyboard-reachable control on the setup surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupControl {
    /// Advance to the next step.
    Continue,
    /// Finish onboarding at the Done step.
    Finish,
    /// Probe and record the live shortcut.
    CheckShortcut,
    /// Run the real launch test.
    TestLaunch,
    /// Skip the current optional step with a warning when needed.
    Skip,
    /// Clear saved onboarding state.
    Reset,
    /// Close the setup surface.
    Close,
}

impl SetupControl {
    /// Visible and accessible label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Continue => "Continue",
            Self::Finish => "Finish setup",
            Self::CheckShortcut => "Check Ctrl+Space",
            Self::TestLaunch => "Test launch",
            Self::Skip => "Skip step",
            Self::Reset => "Reset setup",
            Self::Close => "Close",
        }
    }

    /// Stable element identity.
    pub fn id(self) -> &'static str {
        match self {
            Self::Continue => "setup-continue",
            Self::Finish => "setup-finish",
            Self::CheckShortcut => "setup-check-shortcut",
            Self::TestLaunch => "setup-test-launch",
            Self::Skip => "setup-skip",
            Self::Reset => "setup-reset",
            Self::Close => "setup-close",
        }
    }
}

/// Ordered controls for the current step.
///
/// Completed setup keeps Reset and Close reachable so the surface is never
/// keyboard-trapped.
pub fn setup_controls(step: Step, completed: bool) -> Vec<SetupControl> {
    if completed {
        return vec![SetupControl::Reset, SetupControl::Close];
    }
    match step {
        Step::Welcome => vec![
            SetupControl::Continue,
            SetupControl::Reset,
            SetupControl::Close,
        ],
        Step::Shortcut => vec![
            SetupControl::Continue,
            SetupControl::CheckShortcut,
            SetupControl::Skip,
            SetupControl::Reset,
            SetupControl::Close,
        ],
        Step::TryLaunch => vec![
            SetupControl::Continue,
            SetupControl::TestLaunch,
            SetupControl::Skip,
            SetupControl::Reset,
            SetupControl::Close,
        ],
        Step::Integrations => vec![
            SetupControl::Continue,
            SetupControl::Skip,
            SetupControl::Reset,
            SetupControl::Close,
        ],
        Step::Done => vec![
            SetupControl::Finish,
            SetupControl::Reset,
            SetupControl::Close,
        ],
    }
}

/// Headless setup state machine used by the GPUI surface and its tests.
///
/// The session owns the real [`Onboarding`] model and a deterministic focus
/// index. Every transition is reachable without a pointer, and every shortcut
/// or launch claim comes from a host-supplied probe.
pub struct SetupSession {
    onboarding: Onboarding,
    focus_index: usize,
    message: Option<String>,
    launch_target: Option<CommandId>,
}

impl SetupSession {
    /// Wraps an onboarding model at the Welcome (or persisted) step.
    pub fn new(onboarding: Onboarding) -> Self {
        Self {
            onboarding,
            focus_index: 0,
            message: None,
            launch_target: None,
        }
    }

    /// Current onboarding view.
    pub fn view(&self, hotkey_registered: bool) -> OnboardingView {
        describe_onboarding(&self.onboarding, hotkey_registered)
    }

    /// Read-only onboarding handle for rendering and diagnostics.
    pub fn onboarding(&self) -> &Onboarding {
        &self.onboarding
    }

    /// Keyboard-reachable controls in traversal order.
    pub fn controls(&self) -> Vec<SetupControl> {
        setup_controls(
            self.onboarding.step(),
            self.onboarding.status().is_completed(),
        )
    }

    /// Focused control index.
    pub fn focus_index(&self) -> usize {
        self.focus_index
    }

    /// Focused control, clamped to the current step's controls.
    pub fn focused(&self) -> Option<SetupControl> {
        self.controls().get(self.current_index()).copied()
    }

    /// Moves focus with wrapping by one control.
    pub fn focus_next(&mut self, forward: bool) {
        let len = self.controls().len();
        if len == 0 {
            self.focus_index = 0;
            return;
        }
        let current = self.current_index();
        self.focus_index = if forward {
            (current + 1) % len
        } else {
            (current + len - 1) % len
        };
    }

    /// Selects a control, if it is present on this step.
    pub fn focus_control(&mut self, control: SetupControl) {
        if let Some(index) = self.controls().iter().position(|item| *item == control) {
            self.focus_index = index;
        }
    }

    /// Last action result shown on the setup surface.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Application identity used by the last launch test.
    pub fn launch_target(&self) -> Option<&CommandId> {
        self.launch_target.as_ref()
    }

    /// True when onboarding recorded a launch.
    pub fn launch_verified(&self) -> bool {
        self.onboarding.verified_launch().is_some()
    }

    /// Replaces the message, used by the UI to add host outcome detail.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
    }

    /// Continues the current step.
    ///
    /// The Shortcut step always probes the host, even when a persisted choice
    /// exists. A stored "Registered" value must never bypass the current
    /// session's live registration.
    pub fn continue_setup(&mut self, probe: &dyn ShortcutProbe) {
        if self.onboarding.status().is_completed() {
            self.message = Some("Setup is already complete. Reset setup to run it again.".into());
            return;
        }
        let step = self.onboarding.step();
        let result = match step {
            Step::Shortcut => {
                match self
                    .onboarding
                    .select_shortcut(&Shortcut::documented_default(), probe)
                {
                    Ok(outcome) if outcome.is_available() => self.onboarding.next(),
                    Ok(outcome) => {
                        self.message = Some(format!(
                            "Cannot continue: {}. Fix the conflict and check again, or skip this step with a warning.",
                            outcome
                                .explanation()
                                .unwrap_or_else(|| "the shortcut is unavailable".into())
                        ));
                        return;
                    }
                    Err(error) => Err(error),
                }
            }
            Step::TryLaunch => {
                if self.onboarding.verified_launch().is_none() {
                    self.message = Some(
                        "Run Test launch before continuing, or skip this step with a warning."
                            .into(),
                    );
                    return;
                }
                self.onboarding.next()
            }
            Step::Done => self.onboarding.complete(),
            _ => self.onboarding.next(),
        };
        match result {
            Ok(()) => {
                self.focus_index = 0;
                self.message = Some(format!(
                    "Step: {} ({}).",
                    self.onboarding.step(),
                    self.onboarding.status().label()
                ));
            }
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    /// Probes the live host shortcut and records it only when available.
    pub fn check_shortcut(&mut self, probe: &dyn ShortcutProbe) {
        match self
            .onboarding
            .select_shortcut(&Shortcut::documented_default(), probe)
        {
            Ok(outcome) if outcome.is_available() => {
                self.message = Some("Ctrl+Space is registered; the shortcut was recorded.".into());
            }
            Ok(outcome) => {
                self.message = Some(format!(
                    "Shortcut not recorded: {}.",
                    outcome
                        .explanation()
                        .unwrap_or_else(|| "the shortcut is unavailable".into())
                ));
            }
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    /// Runs a real launch test through the supplied probe.
    ///
    /// The onboarding model records verification only for a positive observed
    /// outcome, so this method never fabricates success.
    pub fn test_launch(&mut self, title: &str, id: CommandId, probe: &dyn LaunchProbe) {
        match self.onboarding.try_launch(&id, probe) {
            Ok(outcome) => {
                self.launch_target = Some(id);
                self.message = Some(format!(
                    "{title}: {}. Launch verification: {}.",
                    outcome.label(),
                    if outcome.is_launched() {
                        "recorded"
                    } else {
                        "not recorded"
                    }
                ));
            }
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    /// Skips the current optional step, reporting any recorded warning.
    pub fn skip(&mut self) {
        let step = self.onboarding.step();
        match self.onboarding.skip() {
            Ok(()) => {
                self.focus_index = 0;
                self.message = Some(match self.onboarding.warning_for(step) {
                    Some(warning) => format!("Skipped with warning: {warning}"),
                    None => format!(
                        "Skipped {step}. No new warning was needed because this step already had a result."
                    ),
                });
            }
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    /// Clears onboarding state and returns to Welcome.
    pub fn reset(&mut self) {
        match self.onboarding.reset() {
            Ok(()) => {
                self.focus_index = 0;
                self.launch_target = None;
                self.message = Some(
                    "Setup reset. The saved onboarding state was cleared; step Welcome.".into(),
                );
            }
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    fn current_index(&self) -> usize {
        let len = self.controls().len();
        if len == 0 {
            0
        } else {
            self.focus_index.min(len - 1)
        }
    }
}

/// Real shortcut probe backed by the launcher's registered hotkey.
pub struct HotkeyProbe {
    outcome: ProbeOutcome,
}

impl HotkeyProbe {
    /// Builds a probe from the launcher's `hotkey.is_some()` state.
    #[cfg(any(windows, test))]
    pub fn new(registered: bool, reason: impl Into<String>) -> Self {
        Self {
            outcome: if registered {
                ProbeOutcome::Available
            } else {
                ProbeOutcome::Denied {
                    reason: reason.into(),
                }
            },
        }
    }

    #[cfg(not(windows))]
    pub fn from_outcome(outcome: ProbeOutcome) -> Self {
        Self { outcome }
    }
}

impl ShortcutProbe for HotkeyProbe {
    fn probe(&self, _shortcut: &Shortcut) -> ProbeOutcome {
        self.outcome.clone()
    }
}

/// Finds the application to use for the onboarding launch test.
///
/// A selected application wins; otherwise the first application in the catalog
/// is used. Built-ins and quicklinks are never launch-test targets.
pub fn find_launch_target(
    catalog: &[Command],
    preferred: Option<&CommandId>,
) -> Option<(CommandId, String)> {
    if let Some(preferred) = preferred
        && let Some(command) = catalog.iter().find(|command| &command.id == preferred)
        && let CommandKind::Application { path } = &command.kind
    {
        return Some((command.id.clone(), path.clone()));
    }
    catalog.iter().find_map(|command| match &command.kind {
        CommandKind::Application { path } => Some((command.id.clone(), path.clone())),
        _ => None,
    })
}

/// Replays an already-observed host outcome into onboarding.
///
/// The GPUI surface performs `launch_or_switch` on a background task and feeds
/// the mapped result back through this probe, keeping window enumeration off
/// the interaction thread.
pub struct FixedLaunchProbe {
    outcome: OnboardingLaunchOutcome,
}

impl FixedLaunchProbe {
    /// Builds a probe for a completed host attempt.
    pub fn new(outcome: OnboardingLaunchOutcome) -> Self {
        Self { outcome }
    }
}

impl LaunchProbe for FixedLaunchProbe {
    fn launch(&self, _command_id: &CommandId) -> OnboardingLaunchOutcome {
        self.outcome.clone()
    }
}

/// Maps a host launch outcome to the onboarding vocabulary and a UI detail.
///
/// Only an observed `Launched` or `Switched` result is a positive onboarding
/// outcome. Everything else remains failed or unverified.
pub fn map_launch_outcome(outcome: LaunchOutcome) -> (OnboardingLaunchOutcome, String) {
    match outcome {
        LaunchOutcome::Launched => (
            OnboardingLaunchOutcome::Launched,
            launch_handoff_label().to_owned(),
        ),
        LaunchOutcome::Switched { window } => (
            OnboardingLaunchOutcome::Launched,
            format!("Switched to an existing window (0x{window:x})"),
        ),
        LaunchOutcome::SwitchedApplication { process_id } => (
            OnboardingLaunchOutcome::Launched,
            format!("Existing application observed in the foreground (PID {process_id})"),
        ),
        LaunchOutcome::Denied { reason } => (
            OnboardingLaunchOutcome::Failed {
                reason: reason.clone(),
            },
            format!("Denied: {reason}"),
        ),
        LaunchOutcome::Failed { reason } => (
            OnboardingLaunchOutcome::Failed {
                reason: reason.clone(),
            },
            format!("Failed: {reason}"),
        ),
        LaunchOutcome::Unverified { reason } => (
            OnboardingLaunchOutcome::Unverified,
            format!("Unverified: {reason}"),
        ),
        LaunchOutcome::NoTarget { reason } => (
            OnboardingLaunchOutcome::Failed {
                reason: reason.clone(),
            },
            format!("No launch target: {reason}"),
        ),
    }
}

/// A native opener handoff does not verify a new process or foreground window.
pub fn launch_handoff_label() -> &'static str {
    if cfg!(windows) {
        "Launched"
    } else {
        "Launch handed to the desktop"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ansa_onboarding::{MemoryStore, Shortcut};
    use ansa_quicklinks::{
        Library, OpenPolicy, Quicklink, QuicklinkId, StaticOpenerProbe, Template,
    };
    use std::collections::BTreeMap;

    fn env(values: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |key| {
            values
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| OsString::from(*value))
        }
    }

    #[test]
    fn data_dir_prefers_environment_override() {
        let data_dir = resolve_data_dir(env(&[
            (DATA_DIR_ENV, r"D:\portable\ansa"),
            ("LOCALAPPDATA", r"C:\Users\test\AppData\Local"),
        ]));
        assert_eq!(data_dir, PathBuf::from(r"D:\portable\ansa"));
    }

    #[cfg(windows)]
    #[test]
    fn data_dir_uses_local_app_data() {
        let data_dir = resolve_data_dir(env(&[("LOCALAPPDATA", r"C:\Users\test\AppData\Local")]));
        assert_eq!(
            data_dir,
            PathBuf::from(r"C:\Users\test\AppData\Local\Ansa\alpha")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn data_dir_uses_macos_application_support() {
        let data_dir = resolve_data_dir(env(&[("HOME", "/Users/test")]));
        assert_eq!(
            data_dir,
            PathBuf::from("/Users/test/Library/Application Support/Ansa/alpha")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn data_dir_uses_absolute_xdg_path_or_home() {
        assert_eq!(
            resolve_data_dir(env(&[("XDG_DATA_HOME", "/data"), ("HOME", "/home/test")])),
            PathBuf::from("/data/Ansa/alpha")
        );
        assert_eq!(
            resolve_data_dir(env(&[
                ("XDG_DATA_HOME", "relative"),
                ("HOME", "/home/test")
            ])),
            PathBuf::from("/home/test/.local/share/Ansa/alpha")
        );
    }

    #[test]
    fn empty_environment_values_are_ignored() {
        let data_dir = resolve_data_dir(env(&[(DATA_DIR_ENV, "")]));
        assert!(!data_dir.as_os_str().is_empty());
        assert_ne!(data_dir, PathBuf::from(""));
    }

    #[test]
    fn whitespace_override_falls_back_to_platform_directory() {
        let data_dir = resolve_data_dir(env(&[
            (DATA_DIR_ENV, "   "),
            ("LOCALAPPDATA", r"C:\Users\test\AppData\Local"),
        ]));
        assert_ne!(data_dir, PathBuf::from("   "));
        #[cfg(windows)]
        assert_eq!(
            data_dir,
            PathBuf::from(r"C:\Users\test\AppData\Local\Ansa\alpha")
        );
    }

    #[test]
    fn smoke_data_dir_is_disposable_and_named() {
        let data_dir = smoke_data_dir();
        let name = data_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        assert!(name.starts_with("ansa-smoke-"));
        assert!(data_dir.starts_with(std::env::temp_dir()));
    }

    #[test]
    fn emoji_mode_is_explicit() {
        assert_eq!(emoji_mode_query("emoji smile"), Some("smile"));
        assert_eq!(emoji_mode_query("  emoji: red heart "), Some("red heart"));
        assert_eq!(emoji_mode_query("emoji"), Some(""));
        assert_eq!(emoji_mode_query("emojis"), None);
        assert_eq!(emoji_mode_query("smile emoji"), None);
        assert_eq!(emoji_mode_query("Emoji Party"), None);
    }

    #[test]
    fn emoji_search_is_bounded() {
        let hits = search_emoji("smile", 5).expect("embedded emoji data");
        assert!(!hits.is_empty());
        assert!(hits.len() <= 5);
        assert!(hits.iter().all(|hit| !hit.accessible_name.is_empty()));
    }

    #[test]
    fn catalog_summary_counts_skipped_quicklinks() {
        let load = CatalogLoad {
            commands: builtin_commands(),
            applications: 12,
            quicklinks: 1,
            skipped: vec![
                ansa_alpha::SkippedQuicklink {
                    id: "quicklink:1".into(),
                    title: "Docs".into(),
                    reason: ansa_alpha::QuicklinkSkipReason::RequiredArguments,
                },
                ansa_alpha::SkippedQuicklink {
                    id: "quicklink:2".into(),
                    title: "Local file".into(),
                    reason: ansa_alpha::QuicklinkSkipReason::NotWebTarget {
                        target_kind: "local-path",
                    },
                },
            ],
            warning: None,
        };
        assert_eq!(
            load.summary(),
            "12 applications · 1 quicklink · 2 quicklinks omitted · local only"
        );
        let warning = skipped_warning(&load.skipped).expect("warning");
        assert!(warning.contains("Docs: required arguments are not supplied"));
        assert!(warning.contains("Local file: resolved target kind local-path"));
        assert!(warning.contains("never launched"));
    }

    fn sample_library_json() -> String {
        let id = QuicklinkId::from_slug("docs").expect("quicklink id");
        let template = Template::parse("https://example.test/docs", Vec::new()).expect("template");
        let candidate = Quicklink::new(id, "Docs", template);
        let mut library = Library::new();
        let preview = library
            .preview(
                &candidate,
                &BTreeMap::new(),
                OpenPolicy::with_local_paths(),
                &StaticOpenerProbe::all_available(),
            )
            .expect("preview");
        library.commit(&preview).expect("commit");
        library.export_json().expect("export")
    }

    #[test]
    fn persisted_settings_are_read_with_safe_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let driver = DailyDriver::open(dir.path()).expect("driver");
        let settings = read_persisted_settings(&driver);
        assert_eq!(settings.hotkey, "Ctrl+Space");
        assert_eq!(settings.theme, "system");
        assert_eq!(settings.maximum_results, "50");
        assert_eq!(settings.launch_mode, "toggle");
        assert_eq!(settings.alias_prefix, "go");
        assert!(settings.diagnostics.is_empty());
        assert!(settings.summary().contains("hotkey=Ctrl+Space"));
    }

    #[test]
    fn refresh_reloads_a_same_process_quicklink_record() {
        // Simulates a same-process record change. A real CLI --data-dir import
        // cannot run while this process holds the exclusive storage lock, so
        // this is not a cross-process live-import test.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut driver = DailyDriver::open(dir.path()).expect("driver");
        driver
            .store()
            .create_record(
                ansa_alpha::QUICKLINK_RECORD_ID,
                ansa_alpha::QUICKLINK_RECORD_KIND,
                &sample_library_json(),
            )
            .expect("record");
        assert_eq!(driver.quicklinks().len(), 0, "driver starts stale");
        let load = reload_and_compose(&mut driver, &[]).expect("compose");
        assert_eq!(driver.quicklinks().len(), 1);
        assert_eq!(load.quicklinks, 1);
        assert_eq!(load.skipped.len(), 0);
        assert!(load.warning.is_none());
        assert!(
            load.commands
                .iter()
                .any(|command| matches!(command.kind, CommandKind::Quicklink { .. }))
        );
    }

    #[test]
    fn corrupt_quicklink_record_warning_reaches_the_ui_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut driver = DailyDriver::open(dir.path()).expect("driver");
        driver
            .store()
            .create_record(
                ansa_alpha::QUICKLINK_RECORD_ID,
                ansa_alpha::QUICKLINK_RECORD_KIND,
                r#"{"unexpected":"shape"}"#,
            )
            .expect("record");
        let load = reload_and_compose(&mut driver, &[]).expect("compose");
        let warning = load.warning.expect("warning must be surfaced");
        assert!(warning.contains("Quicklink library warning"));
        assert!(warning.contains("not used"));
        assert_eq!(load.quicklinks, 0);
    }

    #[cfg(not(windows))]
    #[test]
    fn native_refresh_keeps_setup_and_saved_links_when_discovery_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut driver = DailyDriver::open(dir.path()).expect("driver");
        driver
            .store()
            .create_record(
                ansa_alpha::QUICKLINK_RECORD_ID,
                ansa_alpha::QUICKLINK_RECORD_KIND,
                &sample_library_json(),
            )
            .expect("record");
        let load = refresh_catalog(Some(&mut driver), Err("fixture scan failure".into()))
            .expect("quicklinks do not depend on discovery");
        assert!(load.warning.unwrap().contains("fixture scan failure"));
        assert_eq!(load.applications, 0);
        assert_eq!(load.quicklinks, 1);
        assert!(
            load.commands
                .iter()
                .any(|command| command.id.as_str() == SETUP_COMMAND_ID)
        );

        let app = AppEntry::new("app:fixture", "Fixture", "/native/fixture");
        let load = refresh_catalog(Some(&mut driver), Ok(vec![app])).expect("native catalog");
        assert_eq!(load.applications, 1);
        assert_eq!(load.quicklinks, 1);
        let command = load
            .commands
            .iter()
            .find(|entry| entry.id.as_str() == "app:fixture")
            .unwrap();
        assert_eq!(
            command.kind,
            CommandKind::Application {
                path: "/native/fixture".into()
            }
        );
        assert!(!command.subtitle.as_deref().unwrap().contains("Windows"));
    }

    #[test]
    fn locked_and_corrupt_profile_recovery_keeps_native_catalog() {
        let dir = tempfile::tempdir().expect("tempdir");
        let locked = DailyDriver::open(dir.path()).expect("first driver");
        let database = locked.main_database_path().to_owned();
        let locked_error = DailyDriver::open(dir.path()).expect_err("exclusive profile lock");
        let recovery = storage_recovery(dir.path(), &locked_error.to_string());
        assert!(recovery.contains("Close another Ansa instance"));
        assert!(recovery.contains(&dir.path().display().to_string()));
        drop(locked);

        std::fs::write(database, b"invalid sqlite fixture").expect("write corrupt profile");
        let corrupt_error = DailyDriver::open(dir.path()).expect_err("corrupt database");
        let recovery = storage_recovery(dir.path(), &corrupt_error.to_string());
        assert!(recovery.contains("move a corrupt or newer database aside, then refresh"));

        let load = refresh_catalog(
            None,
            Ok(vec![AppEntry::new("app:fixture", "Fixture", "/fixture")]),
        )
        .expect("catalog fallback");
        assert_eq!(load.applications, 1);
        assert_eq!(load.quicklinks, 0);
    }

    #[test]
    fn setup_command_is_added_exactly_once() {
        let commands = with_setup_command(vec![]);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].id.as_str(), SETUP_COMMAND_ID);
        let commands = with_setup_command(commands);
        assert_eq!(commands.len(), 1);
    }

    #[test]
    fn launch_target_prefers_a_selected_application() {
        let first = Command::new(
            CommandId::parse("app:first").unwrap(),
            "First",
            CommandKind::Application {
                path: r"C:\first.exe".into(),
            },
        );
        let second = Command::new(
            CommandId::parse("app:second").unwrap(),
            "Second",
            CommandKind::Application {
                path: r"C:\second.exe".into(),
            },
        );
        let quicklink = Command::new(
            CommandId::parse("quicklink:demo").unwrap(),
            "Demo",
            CommandKind::Quicklink {
                url: "https://example.com".into(),
            },
        );
        let catalog = vec![first, second, quicklink];
        let selected = CommandId::parse("app:second").unwrap();
        assert_eq!(
            find_launch_target(&catalog, Some(&selected)),
            Some((selected, r"C:\second.exe".into()))
        );
        let missing = CommandId::parse("app:missing").unwrap();
        assert_eq!(
            find_launch_target(&catalog, Some(&missing)),
            Some((
                CommandId::parse("app:first").unwrap(),
                r"C:\first.exe".into()
            ))
        );
        assert!(find_launch_target(&catalog[2..], None).is_none());
    }

    struct Unavailable;

    impl ShortcutProbe for Unavailable {
        fn probe(&self, _shortcut: &Shortcut) -> ProbeOutcome {
            ProbeOutcome::Unsupported
        }
    }

    struct Launched;

    impl LaunchProbe for Launched {
        fn launch(&self, _command_id: &CommandId) -> ansa_onboarding::LaunchOutcome {
            ansa_onboarding::LaunchOutcome::Launched
        }
    }

    #[test]
    fn onboarding_view_never_fabricates_success() {
        let mut onboarding = Onboarding::fresh(MemoryStore::new());
        let view = describe_onboarding(&onboarding, false);
        assert_eq!(view.step, Step::Welcome);
        assert!(!view.is_completed());
        assert!(view.shortcut.contains("not registered"));
        assert!(view.launch.contains("No launch verified"));

        onboarding.next().unwrap();
        onboarding
            .select_shortcut(&Shortcut::documented_default(), &Unavailable)
            .unwrap();
        let view = describe_onboarding(&onboarding, false);
        assert_eq!(view.step, Step::Shortcut);
        assert!(view.shortcut.contains("not registered") || view.shortcut.contains("No shortcut"));
        assert!(!onboarding.shortcut_registered());

        onboarding.skip().unwrap();
        assert_eq!(onboarding.step(), Step::TryLaunch);
        let view = describe_onboarding(&onboarding, false);
        assert!(
            view.warnings
                .iter()
                .any(|warning| warning.contains("Shortcut setup was skipped"))
        );
        assert!(!view.launch.starts_with("Verified"));

        let id = CommandId::parse("app:demo").unwrap();
        assert!(onboarding.try_launch(&id, &Launched).unwrap().is_launched());
        let view = describe_onboarding(&onboarding, false);
        assert!(view.launch.contains("app:demo"));

        onboarding.next().unwrap();
        onboarding.next().unwrap();
        onboarding.complete().unwrap();
        assert!(describe_onboarding(&onboarding, false).is_completed());
    }

    struct Available;

    impl ShortcutProbe for Available {
        fn probe(&self, _shortcut: &Shortcut) -> ProbeOutcome {
            ProbeOutcome::Available
        }
    }

    fn session_at_shortcut() -> SetupSession {
        let mut onboarding = Onboarding::fresh(MemoryStore::new());
        onboarding.next().unwrap();
        SetupSession::new(onboarding)
    }

    fn session_at_try_launch() -> SetupSession {
        let mut session = session_at_shortcut();
        session.skip();
        assert_eq!(session.onboarding().step(), Step::TryLaunch);
        session
    }

    #[test]
    fn setup_controls_are_keyboard_reachable_on_every_step() {
        for step in Step::ALL {
            let controls = setup_controls(step, false);
            assert!(!controls.is_empty(), "{step}");
            assert!(
                controls.iter().any(|control| matches!(
                    control,
                    SetupControl::Continue | SetupControl::Finish
                ))
            );
            assert!(controls.contains(&SetupControl::Close));
        }
        let try_launch = setup_controls(Step::TryLaunch, false);
        assert!(try_launch.contains(&SetupControl::TestLaunch));
        assert!(try_launch.contains(&SetupControl::Skip));
        assert!(try_launch.contains(&SetupControl::Reset));
        let completed = setup_controls(Step::Done, true);
        assert_eq!(completed, vec![SetupControl::Reset, SetupControl::Close]);
    }

    #[test]
    fn setup_focus_moves_and_wraps_in_both_directions() {
        let mut session = session_at_shortcut();
        assert_eq!(session.focused(), Some(SetupControl::Continue));
        session.focus_next(true);
        assert_eq!(session.focused(), Some(SetupControl::CheckShortcut));
        session.focus_next(false);
        assert_eq!(session.focused(), Some(SetupControl::Continue));
        session.focus_next(false);
        assert_eq!(session.focused(), Some(SetupControl::Close));
        session.focus_next(true);
        assert_eq!(session.focused(), Some(SetupControl::Continue));
        session.focus_control(SetupControl::Skip);
        assert_eq!(session.focused(), Some(SetupControl::Skip));
    }

    #[test]
    fn persisted_shortcut_does_not_bypass_the_live_probe() {
        let mut session = session_at_shortcut();
        session.check_shortcut(&Available);
        assert!(session.onboarding().shortcut_registered());

        session.continue_setup(&Unavailable);
        assert_eq!(
            session.onboarding().step(),
            Step::Shortcut,
            "a persisted registered choice must not advance when the live probe fails"
        );
        assert!(
            session
                .message()
                .unwrap_or_default()
                .contains("Cannot continue")
        );

        session.continue_setup(&Available);
        assert_eq!(session.onboarding().step(), Step::TryLaunch);
    }

    #[test]
    fn try_launch_keyboard_path_tests_retries_and_continues() {
        let mut session = session_at_try_launch();
        session.focus_control(SetupControl::TestLaunch);
        assert_eq!(session.focused(), Some(SetupControl::TestLaunch));

        session.test_launch("Demo", CommandId::parse("app:demo").unwrap(), &Launched);
        assert!(session.launch_verified());
        assert!(session.message().unwrap_or_default().contains("recorded"));

        // Retry remains available and remains truthful with a failing probe.
        session.test_launch("Demo", CommandId::parse("app:demo").unwrap(), &FailedProbe);
        assert!(!session.launch_verified());
        assert!(
            session
                .message()
                .unwrap_or_default()
                .contains("not recorded")
        );

        // Test again, then continue with Enter on Continue.
        session.test_launch("Demo", CommandId::parse("app:demo").unwrap(), &Launched);
        session.focus_control(SetupControl::Continue);
        session.continue_setup(&Unavailable);
        assert_eq!(session.onboarding().step(), Step::Integrations);
    }

    struct FailedProbe;

    impl LaunchProbe for FailedProbe {
        fn launch(&self, _command_id: &CommandId) -> OnboardingLaunchOutcome {
            OnboardingLaunchOutcome::Failed {
                reason: "not installed".to_owned(),
            }
        }
    }

    #[test]
    fn try_launch_skip_records_a_warning_and_advances() {
        let mut session = session_at_try_launch();
        session.focus_control(SetupControl::Skip);
        session.skip();
        assert_eq!(session.onboarding().step(), Step::Integrations);
        let warning = session
            .onboarding()
            .warning_for(Step::TryLaunch)
            .expect("skip warning");
        assert!(warning.contains("No successful application launch"));
        assert!(session.message().unwrap_or_default().contains(warning));
    }

    #[test]
    fn setup_can_complete_and_reset_by_keyboard_actions() {
        let mut session = session_at_shortcut();
        session.skip(); // Shortcut -> TryLaunch with a warning.
        session.test_launch("Demo", CommandId::parse("app:demo").unwrap(), &Launched);
        session.continue_setup(&Unavailable); // -> Integrations
        session.continue_setup(&Unavailable); // -> Done
        session.continue_setup(&Unavailable); // complete
        assert!(session.onboarding().status().is_completed());

        session.reset();
        assert_eq!(session.onboarding().step(), Step::Welcome);
        assert!(!session.onboarding().status().is_completed());
        assert_eq!(session.focused(), Some(SetupControl::Continue));
    }

    #[test]
    fn hotkey_probe_is_truthful() {
        let shortcut = Shortcut::documented_default();
        assert!(HotkeyProbe::new(true, "").probe(&shortcut).is_available());
        assert!(matches!(
            HotkeyProbe::new(false, "conflict").probe(&shortcut),
            ProbeOutcome::Denied { .. }
        ));
    }

    #[test]
    fn launch_outcomes_map_to_onboarding_without_fabricating_success() {
        for (outcome, positive) in [
            (LaunchOutcome::Launched, true),
            (LaunchOutcome::Switched { window: 0x42 }, true),
            (
                LaunchOutcome::Denied {
                    reason: "denied".into(),
                },
                false,
            ),
            (
                LaunchOutcome::Failed {
                    reason: "failed".into(),
                },
                false,
            ),
            (
                LaunchOutcome::Unverified {
                    reason: "unverified".into(),
                },
                false,
            ),
            (
                LaunchOutcome::NoTarget {
                    reason: "missing".into(),
                },
                false,
            ),
        ] {
            let (mapped, detail) = map_launch_outcome(outcome);
            assert_eq!(mapped.is_launched(), positive, "{detail}");
            assert!(!detail.is_empty());
        }
    }
}
