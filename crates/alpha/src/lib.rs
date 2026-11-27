//! Daily-driver alpha composition and deterministic readiness harness.
//!
//! This crate is the integration root for the daily launcher. It composes the
//! already-tested building blocks without modifying them:
//!
//! * [`DailyDriver`] opens durable storage in a supplied data directory and
//!   exposes typed settings, the versioned quicklink library (persisted as a
//!   storage [`UserRecord`](ansa_storage::UserRecord)), onboarding state
//!   (`<data_dir>/onboarding.json`), emoji search, catalog composition and
//!   calculator routing.
//! * [`compose_catalog`] builds built-ins, discovered Windows applications
//!   (classic, packaged and broken entries) and enabled persisted quicklinks as
//!   [`CommandKind::Quicklink`](ansa_domain::CommandKind::Quicklink).
//! * [`search_catalog`] routes through the public
//!   [`ansa_core::query::route_query`] and adds bounded emoji hits. Calculator
//!   trigger behavior is therefore **not** reimplemented here: `=` explicitly
//!   requests arithmetic (and is required for units or constants), and a
//!   digit-containing string made only of arithmetic characters is evaluated.
//!   Names such as `7-Zip` and `1Password` remain searches. Calculator results
//!   are rendered through `CalcResult::format`, preserving bounded units.
//! * [`alpha_smoke`] runs the real engine and coordinator for at least the
//!   default 1,000 deterministic cycles.
//!
//! No network access, no automatic paste, no real application launch and no
//! unsafe code are part of this crate.

#![forbid(unsafe_code)]

mod catalog;
mod coordinator;
mod daily_driver;
mod error;
mod feature_registry;
mod harness;
mod records;

pub use catalog::{
    CatalogComposition, EmojiHit, QueryKind, QuicklinkSkipReason, SearchOutcome, SkippedQuicklink,
    compose_catalog, search_catalog, search_emoji,
};
pub use coordinator::{LOCAL_RANK_PROVIDER, build_coordinator};
pub use daily_driver::{
    BACKUP_DATABASE_FILE, CACHE_DATABASE_FILE, DailyDriver, MAIN_DATABASE_FILE, ONBOARDING_FILE,
    OnboardingInfo, QUICKLINK_RECORD_ID, QUICKLINK_RECORD_KIND, VAULT_DIR, emoji_info,
};
pub use error::AlphaError;
pub use feature_registry::{
    CommandHandler, CommandPreview, FeatureError, FeatureRegistry, HandlerOutcome, SearchProvider,
    SearchQuery, SearchResult, build_registry,
};
pub use harness::{
    AlphaSmokeReport, DEFAULT_SMOKE_CYCLES, HarnessError, STABLE_COMMAND_ID, alpha_smoke,
};
pub use records::{
    AGENT_CONVERSATION_RECORD_KIND, FeatureRecordKind, MCP_SERVER_RECORD_KIND, SCRIPT_RECORD_KIND,
    SNIPPET_RECORD_KIND,
};

/// Version of the alpha composition crate.
pub const ALPHA_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Version pin of the ranking relevance corpus shipped by `ansa-search`.
///
/// Mirrored from `crates/search/data/relevance_corpus.json` because the search
/// crate does not expose a public corpus-version API. The alpha integration
/// test fails if this mirror drifts.
pub const RANKING_CORPUS_VERSION: &str = "2026-11-19.3";

/// Version pin of the calculator corpus shipped by `ansa-core`.
///
/// Mirrored from `crates/core/data/calculator_corpus.json`; the alpha
/// integration test fails if this mirror drifts.
pub const CALCULATOR_CORPUS_VERSION: &str = "1.2.0";

/// Human-readable calculator trigger, delegated to `ansa_core::query`.
pub const CALCULATOR_TRIGGER: &str = "explicit '=' prefix (required for units and constants), or digit-only arithmetic using + - * / ^ % ( ) .";
