//! Deterministic alpha readiness harness.
//!
//! The harness runs the real [`Engine`] and the real provider [`Coordinator`]
//! with synthetic commands. It never touches storage, the network, the Start
//! Menu or a real application. Every cycle performs a summon, an alternating
//! query, a selection, a stale engine batch, a stale coordinator batch and a
//! current coordinator batch. It then asserts that stale results never win, the
//! selected stable identity survives when it is still present, and queues stay
//! bounded.

use std::fmt;
use std::time::{Duration, Instant};

use ansa_coordinator::{BatchOutcome, CoordinatorError, Invocation, ProviderBatch};
use ansa_core::{Engine, QueryToken};
use ansa_domain::{BuiltinCommand, Command, CommandId, CommandKind};

use crate::coordinator::build_coordinator;

/// Identity that must stay selected whenever it is present.
pub const STABLE_COMMAND_ID: &str = "app:stable-alpha";

/// Default cycle count for the readiness gate.
pub const DEFAULT_SMOKE_CYCLES: usize = 1_000;

/// Metrics from one successful [`alpha_smoke`] run.
#[derive(Debug, Clone, PartialEq)]
pub struct AlphaSmokeReport {
    /// Completed summon/query/selection cycles.
    pub cycles: usize,
    /// Batches accepted from the current coordinator invocation.
    pub accepted_batches: u64,
    /// Stale or mismatched batches rejected without changing state.
    pub rejected_batches: u64,
    /// Times the stable identity was checked after a late batch.
    pub stable_selection_checks: u64,
    /// Highest observed coordinator pending count.
    pub max_pending: usize,
    /// Highest observed engine result count.
    pub max_results: usize,
    /// Catalog size held by the engine.
    pub catalog_size: usize,
    /// Harness assertions executed.
    pub assertions: u64,
    /// Wall-clock duration of the run.
    pub elapsed: Duration,
    /// Deterministic digest of the non-timing metrics.
    pub fingerprint: u64,
}

impl AlphaSmokeReport {
    /// True once the harness has completed without an assertion failure.
    pub fn passed(&self) -> bool {
        true
    }
}

impl fmt::Display for AlphaSmokeReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cycles={} accepted={} rejected={} stable_checks={} max_pending={} max_results={} catalog={} assertions={} elapsed_ms={:.1} fingerprint=0x{:016x}",
            self.cycles,
            self.accepted_batches,
            self.rejected_batches,
            self.stable_selection_checks,
            self.max_pending,
            self.max_results,
            self.catalog_size,
            self.assertions,
            self.elapsed.as_secs_f64() * 1_000.0,
            self.fingerprint
        )
    }
}

/// A harness assertion failure with the cycle that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessError {
    /// Zero-based cycle, or 0 for a pre-flight failure.
    pub cycle: usize,
    /// What the harness expected.
    pub message: String,
}

impl HarnessError {
    fn new(cycle: usize, message: impl Into<String>) -> Self {
        Self {
            cycle,
            message: message.into(),
        }
    }
}

impl fmt::Display for HarnessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "alpha smoke failed in cycle {}: {}",
            self.cycle, self.message
        )
    }
}

impl std::error::Error for HarnessError {}

/// Runs `cycles` deterministic summon/query/selection cycles.
pub fn alpha_smoke(cycles: usize) -> Result<AlphaSmokeReport, HarnessError> {
    if cycles == 0 {
        return Err(HarnessError::new(0, "cycles must be at least 1"));
    }
    let started = Instant::now();
    let catalog = harness_catalog(400);
    let stable_id = CommandId::parse(STABLE_COMMAND_ID)
        .map_err(|error| HarnessError::new(0, error.to_string()))?;
    let mut engine = Engine::new(catalog.clone());
    let (mut coordinator, provider) =
        build_coordinator(&catalog).map_err(|error| HarnessError::new(0, error.to_string()))?;
    let provider_id = provider.id().clone();

    let mut previous_token: Option<QueryToken> = None;
    let mut accepted_batches = 0_u64;
    let mut rejected_batches = 0_u64;
    let mut stable_selection_checks = 0_u64;
    let mut assertions = 0_u64;
    let mut max_pending = 0_usize;
    let mut max_results = 0_usize;
    let mut final_selected = String::new();

    macro_rules! check {
        ($cycle:expr, $condition:expr, $message:expr) => {
            assertions += 1;
            if !($condition) {
                return Err(HarnessError::new($cycle, $message));
            }
        };
    }

    for cycle in 0..cycles {
        let query = if cycle % 2 == 0 { "alpha" } else { "stable" };
        let tick = cycle as u64;

        engine.begin_invocation();
        let stale_engine_token = previous_token.unwrap_or_else(|| engine.query_token());
        let current_token = engine.set_query(query);

        let stable_index = engine
            .results()
            .iter()
            .position(|command| command.id == stable_id);
        check!(
            cycle,
            stable_index.is_some(),
            "the stable identity was missing from local ranking"
        );
        if let Some(index) = stable_index {
            engine.select_index(index);
        }

        let stale_id = format!("app:stale-{cycle}");
        let stale_command = harness_command(&stale_id, "Stale Batch Result");
        let stale_accepted = engine.apply_batch(stale_engine_token, vec![stale_command.clone()]);
        check!(cycle, !stale_accepted, "a stale engine batch was accepted");
        check!(
            cycle,
            !engine
                .results()
                .iter()
                .any(|command| command.id.as_str() == stale_id.as_str()),
            "a stale engine result entered the result list"
        );
        rejected_batches += 1;

        let stale_invocation = coordinator.begin_at("__stale__", tick);
        let invocation: Invocation = coordinator.begin_at(query, tick);
        let dispatch = coordinator
            .dispatch(&invocation)
            .map_err(|error| HarnessError::new(cycle, error.to_string()))?;
        check!(
            cycle,
            dispatch.dispatches.len() == 1,
            "the current invocation did not dispatch exactly one provider"
        );
        let duplicate = coordinator
            .dispatch(&invocation)
            .map_err(|error| HarnessError::new(cycle, error.to_string()))?;
        check!(
            cycle,
            duplicate.dispatches.is_empty(),
            "a duplicate dispatch was not coalesced"
        );
        check!(
            cycle,
            coordinator.pending_total() <= ansa_coordinator::ProviderSpec::DEFAULT_QUEUE_LIMIT,
            "coordinator pending work exceeded the provider queue limit"
        );
        max_pending = max_pending.max(coordinator.pending_total());

        let stale_outcome = coordinator
            .submit(
                &stale_invocation,
                &provider_id,
                ProviderBatch::complete(&stale_invocation, vec![stale_command.clone()]),
                tick,
            )
            .map_err(|error| HarnessError::new(cycle, error.to_string()))?;
        check!(
            cycle,
            stale_outcome == BatchOutcome::StaleInvocation,
            "a stale coordinator batch was not rejected by invocation identity"
        );
        rejected_batches += 1;

        let mismatch = coordinator.submit(
            &invocation,
            &provider_id,
            ProviderBatch::complete(&stale_invocation, vec![stale_command]),
            tick,
        );
        check!(
            cycle,
            matches!(
                mismatch,
                Err(CoordinatorError::BatchInvocationMismatch { .. })
            ),
            "a batch carrying an old invocation was not rejected"
        );
        rejected_batches += 1;

        let batch = provider.batch_with_budget(&invocation, engine.signals(), catalog.len().max(1));
        let outcome = coordinator
            .submit(&invocation, &provider_id, batch, tick)
            .map_err(|error| HarnessError::new(cycle, error.to_string()))?;
        check!(
            cycle,
            matches!(
                outcome,
                BatchOutcome::Completed { .. } | BatchOutcome::Empty
            ),
            "the current provider batch was not accepted"
        );
        accepted_batches += 1;

        check!(
            cycle,
            coordinator.pending_total() == 0,
            "the accepted batch did not clear pending work"
        );
        max_pending = max_pending.max(coordinator.pending_total());

        let results = coordinator.results().to_vec();
        max_results = max_results.max(results.len());
        let applied = engine.apply_batch(current_token, results);
        check!(cycle, applied, "the current engine batch was rejected");
        check!(
            cycle,
            !engine
                .results()
                .iter()
                .any(|command| command.id.as_str() == stale_id.as_str()),
            "a stale result won after the current batch"
        );
        check!(
            cycle,
            engine.results().len() <= catalog.len(),
            "engine result count exceeded the catalog"
        );
        check!(
            cycle,
            engine.catalog().len() == catalog.len(),
            "engine catalog size changed unexpectedly"
        );
        check!(
            cycle,
            engine.selected_id() == Some(&stable_id),
            "the selected stable identity was not retained"
        );
        stable_selection_checks += 1;

        previous_token = Some(current_token);
        final_selected = engine
            .selected()
            .map(|command| command.id.as_str().to_owned())
            .unwrap_or_default();
    }

    check!(
        cycles,
        max_pending > 0,
        "the harness never queued provider work"
    );
    check!(
        cycles,
        max_pending <= ansa_coordinator::ProviderSpec::DEFAULT_QUEUE_LIMIT,
        "maximum pending work exceeded the provider queue limit"
    );
    check!(
        cycles,
        final_selected == STABLE_COMMAND_ID,
        "the final selection was not the stable identity"
    );

    let fingerprint = fingerprint(&[
        cycles as u64,
        accepted_batches,
        rejected_batches,
        stable_selection_checks,
        max_pending as u64,
        max_results as u64,
        catalog.len() as u64,
        assertions,
        stable_identity_fingerprint(),
    ]);

    Ok(AlphaSmokeReport {
        cycles,
        accepted_batches,
        rejected_batches,
        stable_selection_checks,
        max_pending,
        max_results,
        catalog_size: catalog.len(),
        assertions,
        elapsed: started.elapsed(),
        fingerprint,
    })
}

fn harness_catalog(size: usize) -> Vec<Command> {
    let mut commands: Vec<Command> = BuiltinCommand::ALL
        .into_iter()
        .map(|builtin| builtin.command())
        .collect();
    commands.push(
        Command::new(
            CommandId::parse(STABLE_COMMAND_ID).expect("static harness identity"),
            "Stable Alpha Harness",
            CommandKind::Application {
                path: "C:\\harness\\stable.exe".to_owned(),
            },
        )
        .with_keywords(["alpha", "stable", "harness"]),
    );
    for index in 0..size {
        let id = format!("app:harness-{index:04}");
        commands.push(
            Command::new(
                CommandId::parse(id).expect("generated harness identity"),
                format!("Harness Command {index:04}"),
                CommandKind::Application {
                    path: format!("C:\\harness\\command-{index:04}.exe"),
                },
            )
            .with_keywords(["alpha", "harness"]),
        );
    }
    commands
}

fn harness_command(id: &str, title: &str) -> Command {
    Command::new(
        CommandId::parse(id).expect("generated harness identity"),
        title,
        CommandKind::Application {
            path: "C:\\harness\\stale.exe".to_owned(),
        },
    )
}

fn stable_identity_fingerprint() -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in STABLE_COMMAND_ID.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn fingerprint(values: &[u64]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for value in values {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}
