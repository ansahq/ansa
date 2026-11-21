//! Ranking and provider-coordinator composition for the alpha.

use ansa_coordinator::{
    Coordinator, CoordinatorConfig, LocalRankProvider, ProviderId, ProviderSpec,
};
use ansa_domain::Command;

use crate::error::AlphaError;

/// Provider identity used by the alpha's local rank provider.
pub const LOCAL_RANK_PROVIDER: &str = "local-rank";

/// Builds a bounded single-provider coordinator and its local rank provider.
///
/// Quotas and the result budget are derived from the catalog size so one
/// provider can return the whole catalog, while the queue remains explicit.
pub fn build_coordinator(
    commands: &[Command],
) -> Result<(Coordinator, LocalRankProvider), AlphaError> {
    let provider_id = ProviderId::new(LOCAL_RANK_PROVIDER)
        .map_err(|error| AlphaError::Coordinator(error.to_string()))?;
    let budget = commands.len().max(1);
    let spec = ProviderSpec::new(provider_id.clone())
        .with_quota(commands.len())
        .with_result_budget(budget)
        .with_deadline_ticks(ProviderSpec::DEFAULT_DEADLINE_TICKS)
        .with_queue_limit(ProviderSpec::DEFAULT_QUEUE_LIMIT);
    let coordinator = Coordinator::with_config(vec![spec], CoordinatorConfig::default())
        .map_err(|error| AlphaError::Coordinator(error.to_string()))?;
    let provider = LocalRankProvider::new(provider_id, commands.to_vec());
    Ok((coordinator, provider))
}
