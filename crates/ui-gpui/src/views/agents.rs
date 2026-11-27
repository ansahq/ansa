//! Agents view scaffold.
//!
//! Agent conversations will use the generic `feature.agent-conversation`
//! record namespace for local metadata and messages. Secrets and provider keys
//! must go through the vault, never the record payload.

use super::FeatureView;
use ansa_alpha::AGENT_CONVERSATION_RECORD_KIND;

pub(crate) const VIEW: FeatureView = FeatureView {
    id: "agents",
    title: "Agents",
    record_kind: Some(AGENT_CONVERSATION_RECORD_KIND),
    state: "Scaffold only — no conversation or model provider is enabled",
    owner: "views/agents.rs",
};
