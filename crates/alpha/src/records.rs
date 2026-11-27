//! Generic durable record kinds for feature crates.
//!
//! Phase 0 deliberately does not add feature-specific SQLite tables or
//! migrations. Every feature uses the existing revisioned `user_records` table
//! through [`ansa_storage::Store`]. The payload remains opaque JSON owned by
//! the feature; this module only fixes the record IDs and kinds so two features
//! cannot collide.
//!
//! ```text
//! feature.snippet:<local-id>
//! feature.script:<local-id>
//! feature.mcp-server:<local-id>
//! feature.agent-conversation:<local-id>
//! ```
//!
//! Payloads are unencrypted. Do not store clipboard bodies, script secrets,
//! MCP credentials or conversation transcripts here until the encrypted
//! payload layer is wired. Use [`crate::DailyDriver::vault`] for secret values.

use ansa_storage::UserRecord;

use crate::error::AlphaError;

/// Durable record kind for a saved snippet.
pub const SNIPPET_RECORD_KIND: &str = "feature.snippet";
/// Durable record kind for a saved script.
pub const SCRIPT_RECORD_KIND: &str = "feature.script";
/// Durable record kind for an MCP server definition.
pub const MCP_SERVER_RECORD_KIND: &str = "feature.mcp-server";
/// Durable record kind for an agent conversation.
pub const AGENT_CONVERSATION_RECORD_KIND: &str = "feature.agent-conversation";

/// Features that own generic records in Phase 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FeatureRecordKind {
    /// Clipboard-independent text snippets with an optional trigger.
    Snippet,
    /// User-authored scripts executed by a reviewed handler.
    Script,
    /// MCP server connection metadata (never its credentials).
    McpServer,
    /// Locally stored agent conversation metadata and messages.
    AgentConversation,
}

impl FeatureRecordKind {
    /// Every feature record kind.
    pub const ALL: [Self; 4] = [
        Self::Snippet,
        Self::Script,
        Self::McpServer,
        Self::AgentConversation,
    ];

    /// Stable storage kind.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Snippet => SNIPPET_RECORD_KIND,
            Self::Script => SCRIPT_RECORD_KIND,
            Self::McpServer => MCP_SERVER_RECORD_KIND,
            Self::AgentConversation => AGENT_CONVERSATION_RECORD_KIND,
        }
    }

    /// Human-readable label for diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::Snippet => "snippet",
            Self::Script => "script",
            Self::McpServer => "MCP server",
            Self::AgentConversation => "agent conversation",
        }
    }

    /// Builds the durable record identity for a feature-local ID.
    ///
    /// The caller supplies an already-stable local ID. The storage layer
    /// validates the final identity, so an invalid local ID fails at write
    /// time rather than being silently rewritten.
    pub fn record_id(self, local_id: &str) -> Result<String, AlphaError> {
        if local_id.is_empty() {
            return Err(AlphaError::Record {
                id: self.as_str().to_owned(),
                reason: "local record id must not be empty".to_owned(),
            });
        }
        let id = format!("{}:{}", self.as_str(), local_id);
        if id.len() > 128 {
            return Err(AlphaError::Record {
                id: self.as_str().to_owned(),
                reason: "record id must be 128 bytes or fewer".to_owned(),
            });
        }
        Ok(id)
    }
}

/// Checks that a loaded record belongs to the requested feature kind.
pub(crate) fn ensure_kind(record: &UserRecord, kind: FeatureRecordKind) -> Result<(), AlphaError> {
    if record.kind == kind.as_str() {
        return Ok(());
    }
    Err(AlphaError::Record {
        id: record.id.clone(),
        reason: format!("expected kind {:?}, found {:?}", kind.as_str(), record.kind),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DailyDriver, VAULT_DIR};

    #[test]
    fn kinds_have_distinct_stable_namespaces() {
        let ids: Vec<_> = FeatureRecordKind::ALL
            .into_iter()
            .map(FeatureRecordKind::as_str)
            .collect();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len());
        assert_eq!(
            FeatureRecordKind::Snippet.record_id("greeting").unwrap(),
            "feature.snippet:greeting"
        );
        assert!(
            FeatureRecordKind::Snippet.record_id("").is_err(),
            "empty local ids must fail"
        );
    }

    #[test]
    fn generic_records_round_trip_without_feature_migrations() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let driver = DailyDriver::open(directory.path()).expect("open driver");

        let created = driver
            .create_feature_record(
                FeatureRecordKind::Snippet,
                "greeting",
                r#"{"text":"hello","trigger":"hi"}"#,
            )
            .expect("create snippet record");
        assert_eq!(created.kind, SNIPPET_RECORD_KIND);
        assert_eq!(created.id, "feature.snippet:greeting");

        let loaded = driver
            .get_feature_record(FeatureRecordKind::Snippet, "greeting")
            .expect("read snippet record")
            .expect("record exists");
        assert_eq!(loaded.payload_json, created.payload_json);

        let updated = driver
            .update_feature_record(
                FeatureRecordKind::Snippet,
                "greeting",
                r#"{"text":"hello again"}"#,
                created.revision,
            )
            .expect("update snippet record");
        assert_eq!(updated.revision, created.revision + 1);

        let listed = driver
            .list_feature_records(FeatureRecordKind::Snippet)
            .expect("list snippets");
        assert_eq!(listed.len(), 1);

        driver
            .store()
            .create_record(
                "feature.script:greeting",
                SNIPPET_RECORD_KIND,
                r#"{"mismatched":true}"#,
            )
            .expect("plant a kind mismatch");
        assert!(
            driver
                .get_feature_record(FeatureRecordKind::Script, "greeting")
                .is_err(),
            "a record cannot be read under another kind"
        );
        driver
            .store()
            .delete_record("feature.script:greeting", None)
            .expect("remove kind-mismatch fixture");

        assert!(
            driver
                .delete_feature_record(
                    FeatureRecordKind::Snippet,
                    "greeting",
                    Some(updated.revision)
                )
                .expect("delete snippet record")
        );
        assert!(
            driver
                .list_feature_records(FeatureRecordKind::Snippet)
                .expect("list after delete")
                .is_empty()
        );
    }

    #[test]
    fn all_feature_kinds_can_be_created_through_the_generic_api() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let driver = DailyDriver::open(directory.path()).expect("open driver");
        for kind in FeatureRecordKind::ALL {
            driver
                .create_feature_record(kind, "one", r#"{"version":1}"#)
                .expect("create feature record");
            assert_eq!(
                driver
                    .list_feature_records(kind)
                    .expect("list feature records")
                    .len(),
                1
            );
        }
    }

    #[test]
    fn vault_is_wired_and_unavailable_storage_is_reported_not_fatal() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let driver = DailyDriver::open(directory.path()).expect("open driver");
        assert_eq!(driver.vault_path(), directory.path().join(VAULT_DIR));
        match (driver.vault(), driver.vault_status(), driver.vault_error()) {
            (Some(vault), Some(status), None) => assert_eq!(vault.status(), status),
            (None, None, Some(error)) => assert!(!error.is_empty()),
            other => panic!("inconsistent vault state: {other:?}"),
        }
    }
}
