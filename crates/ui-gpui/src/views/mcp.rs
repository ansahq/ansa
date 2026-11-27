//! MCP servers view scaffold.
//!
//! Server definitions use the generic `feature.mcp-server` record namespace;
//! credentials belong in the vault. No transport, handshake or tool execution
//! exists in this phase.

use super::FeatureView;
use ansa_alpha::MCP_SERVER_RECORD_KIND;

pub(crate) const VIEW: FeatureView = FeatureView {
    id: "mcp",
    title: "MCP servers",
    record_kind: Some(MCP_SERVER_RECORD_KIND),
    state: "Scaffold only — no server connection or tool execution is enabled",
    owner: "views/mcp.rs",
};
