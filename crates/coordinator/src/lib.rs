//! Deterministic, bounded coordination of local and future remote providers.
//!
//! This crate deliberately contains no threads, async runtime, sleeps, clocks or
//! network code. Time is a logical tick supplied by the caller, which makes
//! deadline and cancellation behavior reproducible in tests and independent of
//! the renderer.
//!
//! Core invariants:
//!
//! - Every [`Coordinator::begin`] creates a monotonically increasing
//!   [`Invocation`]. A batch from an older invocation is rejected even if
//!   cooperative cancellation failed.
//! - Every provider has an explicit logical deadline, result quota and result
//!   budget. Per-provider and global pending queues are bounded.
//! - A slow or failed provider never hides results from a provider that has
//!   already finished. Partial results stay available and are labelled
//!   [`SearchState::Partial`], never collapsed into [`SearchState::Failed`].
//! - [`SelectionSnapshot`] keeps a previously selected stable identity selected
//!   while late batches add or remove rows.

use ansa_domain::{Command, CommandId};
use ansa_search::{SearchSignals, rank_bounded};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// Maximum query length retained per invocation. Ranking applies its own
/// stricter bound; this limit protects coordinator queues from a memory flood.
pub const MAX_COORDINATOR_QUERY_CHARS: usize = 256;

/// Maximum retained failure-reason length in Unicode scalar values. Longer
/// reasons are truncated so a misbehaving provider cannot grow coordinator
/// state without bound.
pub const MAX_FAILURE_REASON_CHARS: usize = 256;

static NEXT_COORDINATOR: AtomicU64 = AtomicU64::new(1);

/// Error raised while constructing a provider identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderIdError {
    Empty,
    TooLong,
    InvalidCharacter(char),
}

impl fmt::Display for ProviderIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("provider id must not be empty"),
            Self::TooLong => write!(
                f,
                "provider id must be at most {} characters",
                ProviderId::MAX_LEN
            ),
            Self::InvalidCharacter(character) => {
                write!(f, "provider id contains invalid character {character:?}")
            }
        }
    }
}

impl std::error::Error for ProviderIdError {}

/// Stable identity of a search provider.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderId(String);

impl ProviderId {
    /// Maximum accepted provider-id length in Unicode scalar values.
    pub const MAX_LEN: usize = 64;

    pub fn new(raw: impl Into<String>) -> Result<Self, ProviderIdError> {
        let raw = raw.into();
        if raw.is_empty() {
            return Err(ProviderIdError::Empty);
        }
        if raw.chars().count() > Self::MAX_LEN {
            return Err(ProviderIdError::TooLong);
        }
        if let Some(character) = raw
            .chars()
            .find(|character| character.is_whitespace() || character.is_control())
        {
            return Err(ProviderIdError::InvalidCharacter(character));
        }
        Ok(Self(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identity of one coordinator instance. Invocations from another coordinator
/// are a programming error, not a late batch.
///
/// The value is a process-local monotonic counter, not a cross-process
/// authentication token. Two processes can independently allocate the same
/// numeric id; a batch crossing a process or IPC boundary needs a transport-level
/// session identity in addition to this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CoordinatorId(u64);

impl CoordinatorId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Monotonic identity of one query within a coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InvocationId(u64);

impl InvocationId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// A query invocation. Providers should carry this identity with every batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    coordinator: CoordinatorId,
    id: InvocationId,
    query: String,
    started_at: u64,
}

impl Invocation {
    pub fn id(&self) -> InvocationId {
        self.id
    }

    pub fn coordinator(&self) -> CoordinatorId {
        self.coordinator
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Logical tick at which this invocation started.
    pub fn started_at(&self) -> u64 {
        self.started_at
    }
}

/// Stable selection across batches.
///
/// The snapshot stores row identities, not positions. Applying a new row list
/// keeps the selected [`CommandId`] when it is still present, repairs selection
/// to the first row when it disappeared, and clears selection when there are no
/// rows. Navigation helpers mutate only the selected identity; late batches
/// therefore cannot move the selected row under a navigating user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionSnapshot {
    rows: Vec<CommandId>,
    selected: Option<CommandId>,
}

/// What happened to the selected identity when rows changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionOutcome {
    /// The selected identity still exists.
    Retained,
    /// Selection was repaired to a different identity.
    Repaired,
    /// There are no rows, so nothing can be selected.
    Cleared,
}

impl SelectionSnapshot {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn capture(rows: &[Command], selected: Option<&CommandId>) -> Self {
        Self {
            rows: rows.iter().map(|command| command.id.clone()).collect(),
            selected: selected.cloned(),
        }
    }

    pub fn from_ids(rows: Vec<CommandId>, selected: Option<CommandId>) -> Self {
        Self { rows, selected }
    }

    pub fn rows(&self) -> &[CommandId] {
        &self.rows
    }

    pub fn selected(&self) -> Option<&CommandId> {
        self.selected.as_ref()
    }

    pub fn selected_index(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.rows.iter().position(|row| row == selected)
    }

    /// Selects a row by position. Returns `false` when out of range.
    pub fn select_index(&mut self, index: usize) -> bool {
        match self.rows.get(index) {
            Some(id) => {
                self.selected = Some(id.clone());
                true
            }
            None => false,
        }
    }

    /// Selects a still-present identity. Returns `false` when it is absent.
    pub fn select_id(&mut self, id: &CommandId) -> bool {
        if self.rows.iter().any(|row| row == id) {
            self.selected = Some(id.clone());
            true
        } else {
            false
        }
    }

    /// Applies a new row list while preserving selected identity.
    pub fn apply_rows(&mut self, rows: &[Command]) -> SelectionOutcome {
        self.rows = rows.iter().map(|command| command.id.clone()).collect();

        let still_present = self
            .selected
            .as_ref()
            .is_some_and(|id| self.rows.iter().any(|row| row == id));
        if still_present {
            return SelectionOutcome::Retained;
        }

        self.selected = self.rows.first().cloned();
        if self.selected.is_some() {
            SelectionOutcome::Repaired
        } else {
            SelectionOutcome::Cleared
        }
    }

    /// Moves selection by `delta` rows, clamped to the row list.
    pub fn move_by(&mut self, delta: i32) {
        if self.rows.is_empty() {
            self.selected = None;
            return;
        }
        let current = self.selected_index().unwrap_or(0);
        let last = self.rows.len() - 1;
        let next = (current as i64 + delta as i64).clamp(0, last as i64) as usize;
        self.selected = Some(self.rows[next].clone());
    }
}

/// Provider and aggregate result state.
///
/// [`SearchState::Loading`] is the only non-terminal state. A provider stays
/// `Loading` while it is in flight, even after a non-final batch has produced
/// results. On deadline expiry or failure it becomes either
/// [`SearchState::Partial`] (results retained, provider did not finish) or
/// [`SearchState::Failed`] (no results). `Partial`, `Ready`, `Empty`, `Failed`,
/// `Unavailable` and `Cancelled` are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchState {
    /// At least one provider is still in flight and no terminal state applies.
    Loading,
    /// All usable providers reached a terminal success state.
    Ready,
    /// Terminal partial results: some results are available, but a provider
    /// timed out, failed after producing results, or was cancelled.
    Partial,
    /// All usable providers completed successfully with no results.
    Empty,
    /// No results are available and at least one provider failed or timed out.
    Failed,
    /// The provider was not offered for this host, or every provider is
    /// unavailable.
    Unavailable,
    /// The invocation was cancelled. Any accepted results are retained.
    Cancelled,
}

impl SearchState {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Loading)
    }

    /// True when this state can be accompanied by retained results.
    pub fn has_results(self) -> bool {
        matches!(
            self,
            Self::Loading | Self::Ready | Self::Partial | Self::Cancelled
        )
    }
}

/// A batch supplied by one provider.
///
/// The coordinator and invocation identities are part of the batch itself so a
/// reordered or cross-coordinator batch can be rejected even when the caller
/// passes the current invocation token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderBatch {
    coordinator: CoordinatorId,
    invocation: InvocationId,
    items: Vec<Command>,
    complete: bool,
    truncated: bool,
}

impl ProviderBatch {
    pub fn new(invocation: &Invocation, items: Vec<Command>) -> Self {
        Self {
            coordinator: invocation.coordinator(),
            invocation: invocation.id(),
            items,
            complete: false,
            truncated: false,
        }
    }

    pub fn complete(invocation: &Invocation, items: Vec<Command>) -> Self {
        Self {
            coordinator: invocation.coordinator(),
            invocation: invocation.id(),
            items,
            complete: true,
            truncated: false,
        }
    }

    /// Marks this batch as the complete prefix of a larger ranked result set.
    /// The provider chose not to send more items because of a result budget.
    pub fn complete_truncated(invocation: &Invocation, items: Vec<Command>) -> Self {
        Self {
            coordinator: invocation.coordinator(),
            invocation: invocation.id(),
            items,
            complete: true,
            truncated: true,
        }
    }

    pub fn coordinator(&self) -> CoordinatorId {
        self.coordinator
    }

    pub fn invocation(&self) -> InvocationId {
        self.invocation
    }

    pub fn items(&self) -> &[Command] {
        &self.items
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// True when the provider deliberately truncated its ranked output to fit
    /// a result budget. This is not an error and does not make the batch
    /// over-budget.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Result of submitting one provider batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchOutcome {
    /// A non-final batch was accepted and the provider remains partial.
    Accepted { accepted: usize },
    /// A final batch was accepted and the provider is ready.
    Completed { accepted: usize },
    /// A final batch was accepted and the provider is ready, but the provider
    /// reported that it deliberately truncated its ranked output to a result
    /// budget. No error is raised for this case.
    CompletedTruncated { accepted: usize },
    /// A late report arrived for an already-terminal provider and was ignored.
    /// The returned state is the unchanged terminal state.
    Settled { state: SearchState },
    /// A final batch contained no new results; the provider is empty.
    Empty,
    /// The provider hit its configured quota; excess items were dropped.
    QuotaExhausted { accepted: usize, dropped: usize },
    /// The batch arrived after its invocation was replaced.
    StaleInvocation,
    /// The invocation was cancelled.
    Cancelled,
    /// The batch arrived at or after the provider deadline.
    Expired,
    /// The provider reported failure. Already accepted results are retained.
    Failed { retained: usize },
    /// The provider is not offered on this host.
    Unavailable,
}

/// Result of dispatching an invocation to providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchReport {
    pub invocation: InvocationId,
    pub stale: bool,
    pub cancelled: bool,
    pub dispatches: Vec<ProviderDispatch>,
    /// Retained for API compatibility. Queue overflow is now an explicit error
    /// and live work is never evicted, so this is always zero.
    pub evicted: usize,
}

/// One unit of queued provider work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDispatch {
    pub provider: ProviderId,
    pub query: String,
    pub deadline: u64,
}

/// Explicit coordinator errors. Late and cancelled batches are normal outcomes,
/// not errors; malformed identities are errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordinatorError {
    DuplicateProvider(ProviderId),
    UnknownProvider(ProviderId),
    InvocationMismatch {
        expected: CoordinatorId,
        actual: CoordinatorId,
    },
    UnknownInvocation(InvocationId),
    BatchInvocationMismatch {
        expected: InvocationId,
        actual: InvocationId,
    },
    BatchCoordinatorMismatch {
        expected: CoordinatorId,
        actual: CoordinatorId,
    },
    BatchOverBudget {
        provider: ProviderId,
        size: usize,
        budget: usize,
    },
    InvalidProviderSpec {
        provider: ProviderId,
        reason: &'static str,
    },
    ProviderUnavailable(ProviderId),
    /// Reserved for a future multi-invocation queue. With one current
    /// invocation and duplicate-dispatch coalescing, the global limit is the
    /// limit that can actually be reached today; the per-provider check remains
    /// a defensive invariant.
    ProviderQueueFull {
        provider: ProviderId,
        limit: usize,
    },
    GlobalQueueFull {
        limit: usize,
        requested: usize,
        queued: usize,
    },
    InvocationIdExhausted,
}

impl fmt::Display for CoordinatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateProvider(provider) => {
                write!(f, "provider {provider} is already registered")
            }
            Self::UnknownProvider(provider) => write!(f, "provider {provider} is not registered"),
            Self::InvocationMismatch { expected, actual } => write!(
                f,
                "invocation belongs to coordinator {} but this coordinator is {}",
                actual.get(),
                expected.get()
            ),
            Self::UnknownInvocation(id) => {
                write!(f, "invocation {} was never issued", id.get())
            }
            Self::BatchInvocationMismatch { expected, actual } => write!(
                f,
                "batch carries invocation {} but {} was submitted",
                actual.get(),
                expected.get()
            ),
            Self::BatchCoordinatorMismatch { expected, actual } => write!(
                f,
                "batch belongs to coordinator {} but this coordinator is {}",
                actual.get(),
                expected.get()
            ),
            Self::BatchOverBudget {
                provider,
                size,
                budget,
            } => write!(
                f,
                "provider {provider} returned {size} items, exceeding its budget of {budget}"
            ),
            Self::InvalidProviderSpec { provider, reason } => {
                write!(f, "invalid spec for provider {provider}: {reason}")
            }
            Self::ProviderUnavailable(provider) => {
                write!(f, "provider {provider} is unavailable")
            }
            Self::ProviderQueueFull { provider, limit } => write!(
                f,
                "provider {provider} queue is full at its limit of {limit}"
            ),
            Self::GlobalQueueFull {
                limit,
                requested,
                queued,
            } => write!(
                f,
                "global queue limit {limit} cannot accept {requested} new dispatches with {queued} already queued"
            ),
            Self::InvocationIdExhausted => f.write_str("coordinator invocation id space exhausted"),
        }
    }
}

impl std::error::Error for CoordinatorError {}

/// Provider configuration. Quotas are per invocation and `result_budget` bounds
/// the number of items in a single batch. Budgets are item counts, not byte
/// bounds; a caller that accepts untrusted titles must apply its own text-size
/// limits before constructing [`Command`] values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSpec {
    id: ProviderId,
    quota: usize,
    result_budget: usize,
    deadline_ticks: u64,
    queue_limit: usize,
    available: bool,
}

impl ProviderSpec {
    pub const DEFAULT_QUOTA: usize = 50;
    pub const DEFAULT_RESULT_BUDGET: usize = 200;
    pub const DEFAULT_DEADLINE_TICKS: u64 = 30;
    pub const DEFAULT_QUEUE_LIMIT: usize = 4;

    pub fn new(id: ProviderId) -> Self {
        Self {
            id,
            quota: Self::DEFAULT_QUOTA,
            result_budget: Self::DEFAULT_RESULT_BUDGET,
            deadline_ticks: Self::DEFAULT_DEADLINE_TICKS,
            queue_limit: Self::DEFAULT_QUEUE_LIMIT,
            available: true,
        }
    }

    pub fn with_quota(mut self, quota: usize) -> Self {
        self.quota = quota;
        self
    }

    pub fn with_result_budget(mut self, result_budget: usize) -> Self {
        self.result_budget = result_budget;
        self
    }

    pub fn with_deadline_ticks(mut self, deadline_ticks: u64) -> Self {
        self.deadline_ticks = deadline_ticks;
        self
    }

    pub fn with_queue_limit(mut self, queue_limit: usize) -> Self {
        self.queue_limit = queue_limit;
        self
    }

    /// Marks the provider as not offered on this host.
    pub fn unavailable(mut self) -> Self {
        self.available = false;
        self
    }

    pub fn id(&self) -> &ProviderId {
        &self.id
    }

    pub fn quota(&self) -> usize {
        self.quota
    }

    pub fn result_budget(&self) -> usize {
        self.result_budget
    }

    pub fn deadline_ticks(&self) -> u64 {
        self.deadline_ticks
    }

    pub fn queue_limit(&self) -> usize {
        self.queue_limit
    }

    pub fn is_available(&self) -> bool {
        self.available
    }
}

/// Coordinator-wide limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoordinatorConfig {
    /// Maximum total queued provider invocations across all providers.
    pub global_queue_limit: usize,
}

impl CoordinatorConfig {
    pub fn new(global_queue_limit: usize) -> Self {
        Self { global_queue_limit }
    }
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            global_queue_limit: 64,
        }
    }
}

/// Read-only per-provider view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSnapshot {
    pub id: ProviderId,
    pub state: SearchState,
    pub result_count: usize,
    pub accepted: usize,
    pub pending: usize,
    pub deadline: u64,
    pub failure: Option<String>,
}

/// Read-only coordinator view for renderers. It owns clones so callers can
/// render without holding a borrow of the coordinator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorSnapshot {
    pub coordinator: CoordinatorId,
    pub invocation: Option<InvocationId>,
    pub query: Option<String>,
    pub state: SearchState,
    pub results: Vec<Command>,
    pub providers: Vec<ProviderSnapshot>,
    pub cancelled: bool,
    pub now: u64,
}

#[derive(Debug, Clone)]
struct PendingWork {
    invocation: InvocationId,
    deadline: u64,
}

#[derive(Debug, Clone)]
struct ProviderRuntime {
    spec: ProviderSpec,
    state: SearchState,
    results: Vec<Command>,
    accepted: usize,
    deadline: u64,
    pending: Vec<PendingWork>,
    dispatched: bool,
    failure: Option<String>,
}

impl ProviderRuntime {
    fn new(spec: ProviderSpec, started_at: u64) -> Self {
        let deadline = started_at.saturating_add(spec.deadline_ticks);
        Self {
            state: if spec.available {
                SearchState::Loading
            } else {
                SearchState::Unavailable
            },
            results: Vec::new(),
            accepted: 0,
            deadline,
            pending: Vec::new(),
            dispatched: false,
            failure: None,
            spec,
        }
    }

    fn reset(&mut self, started_at: u64) {
        self.state = if self.spec.available {
            SearchState::Loading
        } else {
            SearchState::Unavailable
        };
        self.results.clear();
        self.accepted = 0;
        self.deadline = started_at.saturating_add(self.spec.deadline_ticks);
        self.pending.clear();
        self.dispatched = false;
        self.failure = None;
    }

    fn clear_pending(&mut self) {
        self.pending.clear();
    }
}

/// The provider coordinator. All operations are synchronous and deterministic.
#[derive(Debug)]
pub struct Coordinator {
    id: CoordinatorId,
    providers: Vec<ProviderRuntime>,
    index: HashMap<ProviderId, usize>,
    current: Option<Invocation>,
    last_issued: InvocationId,
    cancelled: bool,
    now: u64,
    config: CoordinatorConfig,
    aggregate: SearchState,
    aggregated_results: Vec<Command>,
}

impl Coordinator {
    /// Creates a coordinator with the default global queue limit.
    pub fn new(specs: Vec<ProviderSpec>) -> Result<Self, CoordinatorError> {
        Self::with_config(specs, CoordinatorConfig::default())
    }

    /// Creates a coordinator with explicit limits.
    pub fn with_config(
        specs: Vec<ProviderSpec>,
        config: CoordinatorConfig,
    ) -> Result<Self, CoordinatorError> {
        let mut providers = Vec::with_capacity(specs.len());
        let mut index = HashMap::with_capacity(specs.len());

        for spec in specs {
            if index.contains_key(&spec.id) {
                return Err(CoordinatorError::DuplicateProvider(spec.id));
            }
            if spec.quota > spec.result_budget {
                return Err(CoordinatorError::InvalidProviderSpec {
                    provider: spec.id,
                    reason: "quota must not exceed the result budget",
                });
            }
            if spec.queue_limit == 0 {
                return Err(CoordinatorError::InvalidProviderSpec {
                    provider: spec.id,
                    reason: "queue limit must be positive",
                });
            }
            index.insert(spec.id.clone(), providers.len());
            providers.push(ProviderRuntime::new(spec, 0));
        }

        Ok(Self {
            id: CoordinatorId(NEXT_COORDINATOR.fetch_add(1, Ordering::Relaxed)),
            providers,
            index,
            current: None,
            last_issued: InvocationId(0),
            cancelled: false,
            now: 0,
            config,
            aggregate: SearchState::Unavailable,
            aggregated_results: Vec::new(),
        })
    }

    pub fn id(&self) -> CoordinatorId {
        self.id
    }

    pub fn now(&self) -> u64 {
        self.now
    }

    pub fn config(&self) -> CoordinatorConfig {
        self.config
    }

    /// Starts a new invocation at the coordinator's current logical time.
    pub fn begin(&mut self, query: &str) -> Invocation {
        self.begin_at(query, self.now)
    }

    /// Fallible variant of [`Coordinator::begin`].
    pub fn try_begin(&mut self, query: &str) -> Result<Invocation, CoordinatorError> {
        self.try_begin_at(query, self.now)
    }

    /// Starts a new invocation at `now`, advancing the logical clock.
    ///
    /// This invalidates every outstanding invocation, clears pending queues and
    /// clears aggregate results so no stale action target survives a query.
    ///
    /// Invocation-id exhaustion is fail-closed: the infallible method returns a
    /// non-current sentinel invocation, clears the current invocation and leaves
    /// the coordinator unable to accept provider work. Use
    /// [`Coordinator::try_begin_at`] to observe the explicit error instead.
    pub fn begin_at(&mut self, query: &str, now: u64) -> Invocation {
        match self.try_begin_at(query, now) {
            Ok(invocation) => invocation,
            Err(_) => {
                self.now = self.now.max(now);
                self.current = None;
                self.cancelled = false;
                self.aggregated_results.clear();
                for provider in &mut self.providers {
                    provider.reset(self.now);
                }
                self.refresh_aggregate();
                Invocation {
                    coordinator: self.id,
                    id: self.last_issued,
                    query: bounded_query(query),
                    started_at: self.now,
                }
            }
        }
    }

    /// Fallible variant of [`Coordinator::begin_at`].
    pub fn try_begin_at(&mut self, query: &str, now: u64) -> Result<Invocation, CoordinatorError> {
        self.now = self.now.max(now);
        let next = self
            .last_issued
            .0
            .checked_add(1)
            .ok_or(CoordinatorError::InvocationIdExhausted)?;
        let invocation = Invocation {
            coordinator: self.id,
            id: InvocationId(next),
            query: bounded_query(query),
            started_at: self.now,
        };
        self.last_issued = invocation.id;
        self.current = Some(invocation.clone());
        self.cancelled = false;
        self.aggregated_results.clear();
        for provider in &mut self.providers {
            provider.reset(invocation.started_at);
        }
        self.refresh_aggregate();
        Ok(invocation)
    }

    /// True when no further invocation can be issued safely.
    pub fn is_exhausted(&self) -> bool {
        self.last_issued.0 == u64::MAX
    }

    pub fn current_invocation(&self) -> Option<&Invocation> {
        self.current.as_ref()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    /// Queues provider work for `invocation`.
    ///
    /// Duplicate dispatches for the same provider and invocation are coalesced.
    /// Queue limits are enforced before any mutation: if a per-provider or
    /// global limit cannot hold all new work, the call returns an explicit error
    /// and queues nothing. Live work is never evicted.
    pub fn dispatch(
        &mut self,
        invocation: &Invocation,
    ) -> Result<DispatchReport, CoordinatorError> {
        self.validate_invocation(invocation)?;

        let current = self.current.as_ref().map(|current| current.id);
        if current != Some(invocation.id) {
            return Ok(DispatchReport {
                invocation: invocation.id,
                stale: true,
                cancelled: self.cancelled,
                dispatches: Vec::new(),
                evicted: 0,
            });
        }
        if self.cancelled {
            return Ok(DispatchReport {
                invocation: invocation.id,
                stale: false,
                cancelled: true,
                dispatches: Vec::new(),
                evicted: 0,
            });
        }

        let mut targets = Vec::with_capacity(self.providers.len());
        for (index, runtime) in self.providers.iter().enumerate() {
            if !runtime.spec.available
                || runtime.state != SearchState::Loading
                || runtime.dispatched
            {
                continue;
            }
            if runtime
                .pending
                .iter()
                .any(|work| work.invocation == invocation.id)
            {
                continue;
            }
            if runtime.pending.len() >= runtime.spec.queue_limit {
                return Err(CoordinatorError::ProviderQueueFull {
                    provider: runtime.spec.id.clone(),
                    limit: runtime.spec.queue_limit,
                });
            }
            targets.push(index);
        }

        let queued = self.pending_total();
        if queued.saturating_add(targets.len()) > self.config.global_queue_limit {
            return Err(CoordinatorError::GlobalQueueFull {
                limit: self.config.global_queue_limit,
                requested: targets.len(),
                queued,
            });
        }

        let mut dispatches = Vec::with_capacity(targets.len());
        for index in targets {
            let runtime = &mut self.providers[index];
            runtime.deadline = invocation
                .started_at
                .saturating_add(runtime.spec.deadline_ticks);
            runtime.pending.push(PendingWork {
                invocation: invocation.id,
                deadline: runtime.deadline,
            });
            runtime.dispatched = true;
            dispatches.push(ProviderDispatch {
                provider: runtime.spec.id.clone(),
                query: invocation.query.clone(),
                deadline: runtime.deadline,
            });
        }

        Ok(DispatchReport {
            invocation: invocation.id,
            stale: false,
            cancelled: false,
            dispatches,
            evicted: 0,
        })
    }

    /// Applies one provider batch.
    ///
    /// The batch must carry the submitted invocation. Old invocations, cancelled
    /// invocations and expired providers are rejected without touching results.
    pub fn submit(
        &mut self,
        invocation: &Invocation,
        provider: &ProviderId,
        batch: ProviderBatch,
        now: u64,
    ) -> Result<BatchOutcome, CoordinatorError> {
        self.validate_invocation(invocation)?;
        let index = self.provider_index(provider)?;
        if batch.coordinator != self.id {
            return Err(CoordinatorError::BatchCoordinatorMismatch {
                expected: self.id,
                actual: batch.coordinator,
            });
        }
        if batch.invocation != invocation.id {
            return Err(CoordinatorError::BatchInvocationMismatch {
                expected: invocation.id,
                actual: batch.invocation,
            });
        }

        let spec = self.providers[index].spec.clone();
        if !spec.available {
            return Err(CoordinatorError::ProviderUnavailable(provider.clone()));
        }
        let size = batch.items.len();
        if size > spec.result_budget {
            // Validate before advancing the clock: a rejected batch must not
            // mutate observable coordinator state.
            return Err(CoordinatorError::BatchOverBudget {
                provider: provider.clone(),
                size,
                budget: spec.result_budget,
            });
        }

        let current = self.current.as_ref().map(|current| current.id);
        if current != Some(invocation.id) {
            // Stale work must not advance the logical clock. Otherwise a
            // far-future timestamp from an old invocation can poison every
            // subsequent query on this coordinator.
            return Ok(BatchOutcome::StaleInvocation);
        }
        if self.cancelled {
            return Ok(BatchOutcome::Cancelled);
        }

        remove_pending(&mut self.providers[index], invocation.id);
        let state = self.providers[index].state;
        let effective_now = self.now.max(now);
        if effective_now >= self.providers[index].deadline {
            if state == SearchState::Loading {
                // Clamp the clock to the deadline; a far-future late batch must
                // not leak into subsequent invocations.
                self.now = self.now.max(self.providers[index].deadline);
                self.expire_provider(index, "deadline exceeded");
                self.refresh_aggregate();
            }
            return Ok(BatchOutcome::Expired);
        }

        match state {
            SearchState::Unavailable => {
                return Err(CoordinatorError::ProviderUnavailable(provider.clone()));
            }
            SearchState::Cancelled => return Ok(BatchOutcome::Cancelled),
            SearchState::Failed | SearchState::Partial => {
                return Ok(BatchOutcome::Failed {
                    retained: self.providers[index].results.len(),
                });
            }
            SearchState::Ready | SearchState::Empty => {
                let runtime = &self.providers[index];
                let known: HashSet<CommandId> = runtime
                    .results
                    .iter()
                    .map(|command| command.id.clone())
                    .collect();
                let unique_new = batch
                    .items
                    .iter()
                    .filter(|command| !known.contains(&command.id))
                    .count();
                let remaining = spec.quota.saturating_sub(runtime.accepted);
                let dropped = unique_new.saturating_sub(remaining);
                if dropped > 0 {
                    return Ok(BatchOutcome::QuotaExhausted {
                        accepted: 0,
                        dropped,
                    });
                }
                return Ok(if batch.complete {
                    if state == SearchState::Empty {
                        BatchOutcome::Empty
                    } else {
                        BatchOutcome::Completed { accepted: 0 }
                    }
                } else {
                    BatchOutcome::Accepted { accepted: 0 }
                });
            }
            SearchState::Loading => {}
        }

        // Only accepted, in-flight work advances the clock.
        self.now = effective_now;

        let truncated = batch.truncated;
        let runtime = &mut self.providers[index];
        let mut known: HashSet<CommandId> = runtime
            .results
            .iter()
            .map(|command| command.id.clone())
            .collect();
        let remaining = spec.quota.saturating_sub(runtime.accepted);
        let mut accepted = Vec::new();
        let mut dropped = 0usize;
        for item in batch.items {
            if !known.insert(item.id.clone()) {
                continue;
            }
            if accepted.len() >= remaining {
                dropped += 1;
            } else {
                accepted.push(item);
            }
        }

        let accepted_count = accepted.len();
        runtime.results.extend(accepted);
        runtime.accepted = runtime.accepted.saturating_add(accepted_count);

        let outcome = if dropped > 0 {
            runtime.state = SearchState::Ready;
            runtime.failure = None;
            BatchOutcome::QuotaExhausted {
                accepted: accepted_count,
                dropped,
            }
        } else if accepted_count > 0 {
            if batch.complete {
                runtime.state = SearchState::Ready;
                runtime.failure = None;
                if truncated {
                    BatchOutcome::CompletedTruncated {
                        accepted: accepted_count,
                    }
                } else {
                    BatchOutcome::Completed {
                        accepted: accepted_count,
                    }
                }
            } else {
                // Still in flight. Results are visible, but the provider state
                // remains Loading until completion, failure or its deadline.
                BatchOutcome::Accepted {
                    accepted: accepted_count,
                }
            }
        } else if batch.complete {
            if runtime.results.is_empty() {
                runtime.state = SearchState::Empty;
                runtime.failure = None;
                if truncated {
                    BatchOutcome::CompletedTruncated { accepted: 0 }
                } else {
                    BatchOutcome::Empty
                }
            } else {
                // Duplicate final batch: the provider already retains results.
                runtime.state = SearchState::Ready;
                runtime.failure = None;
                BatchOutcome::Completed { accepted: 0 }
            }
        } else {
            BatchOutcome::Accepted { accepted: 0 }
        };

        self.refresh_aggregate();
        Ok(outcome)
    }

    /// Reports a provider failure. Previously accepted results are retained and
    /// the provider becomes [`SearchState::Partial`] when it already produced
    /// results, otherwise [`SearchState::Failed`].
    pub fn fail(
        &mut self,
        invocation: &Invocation,
        provider: &ProviderId,
        reason: &str,
    ) -> Result<BatchOutcome, CoordinatorError> {
        self.validate_invocation(invocation)?;
        let index = self.provider_index(provider)?;
        if !self.providers[index].spec.available {
            return Err(CoordinatorError::ProviderUnavailable(provider.clone()));
        }
        let current = self.current.as_ref().map(|current| current.id);
        if current != Some(invocation.id) {
            return Ok(BatchOutcome::StaleInvocation);
        }
        if self.cancelled {
            return Ok(BatchOutcome::Cancelled);
        }

        remove_pending(&mut self.providers[index], invocation.id);
        let runtime = &mut self.providers[index];
        let retained = runtime.results.len();
        match runtime.state {
            SearchState::Unavailable | SearchState::Cancelled => {
                return Ok(BatchOutcome::Cancelled);
            }
            SearchState::Failed | SearchState::Partial => {
                return Ok(BatchOutcome::Failed { retained });
            }
            SearchState::Ready | SearchState::Empty => {
                // A terminal success is not downgraded by a late failure report.
                return Ok(BatchOutcome::Settled {
                    state: runtime.state,
                });
            }
            SearchState::Loading => {}
        }
        runtime.failure = Some(bounded_reason(reason));
        runtime.state = if retained == 0 {
            SearchState::Failed
        } else {
            SearchState::Partial
        };
        self.refresh_aggregate();
        Ok(BatchOutcome::Failed { retained })
    }

    /// Cancels an invocation cooperatively and settles it as
    /// [`SearchState::Cancelled`]. Accepted results are retained. Late batches
    /// are still rejected by invocation identity even if the provider ignores
    /// cancellation.
    pub fn cancel(&mut self, invocation: &Invocation) -> Result<(), CoordinatorError> {
        self.validate_invocation(invocation)?;
        if self.current.as_ref().map(|current| current.id) == Some(invocation.id) {
            self.cancelled = true;
            for provider in &mut self.providers {
                provider.clear_pending();
                if provider.state != SearchState::Unavailable {
                    provider.state = SearchState::Cancelled;
                    provider.failure = Some(bounded_reason("cancelled"));
                }
            }
            self.refresh_aggregate();
        }
        Ok(())
    }

    /// Advances the logical clock and finalizes provider deadlines.
    ///
    /// Returns the number of providers that moved from [`SearchState::Loading`]
    /// to a terminal state. A provider with retained results becomes
    /// [`SearchState::Partial`]; one with no results becomes
    /// [`SearchState::Failed`]. Already-terminal providers are unchanged.
    pub fn tick(&mut self, now: u64) -> usize {
        self.now = self.now.max(now);
        let mut expired = 0;
        for index in 0..self.providers.len() {
            let deadline = self.providers[index].deadline;
            self.providers[index]
                .pending
                .retain(|work| work.deadline > self.now);
            if self.providers[index].state == SearchState::Loading && self.now >= deadline {
                self.expire_provider(index, "deadline exceeded");
                expired += 1;
            }
        }
        self.refresh_aggregate();
        expired
    }

    /// Aggregate state for the current invocation.
    pub fn state(&self) -> SearchState {
        self.aggregate
    }

    /// Deduplicated aggregate results in provider registration order, then in
    /// provider-supplied rank order.
    pub fn results(&self) -> &[Command] {
        &self.aggregated_results
    }

    pub fn result_count(&self) -> usize {
        self.aggregated_results.len()
    }

    pub fn provider_state(&self, provider: &ProviderId) -> Result<SearchState, CoordinatorError> {
        let index = self.provider_index(provider)?;
        Ok(self.providers[index].state)
    }

    /// Number of queued units for one provider.
    pub fn pending_len(&self, provider: &ProviderId) -> Result<usize, CoordinatorError> {
        let index = self.provider_index(provider)?;
        Ok(self.providers[index].pending.len())
    }

    /// Total queued units across all providers.
    pub fn pending_total(&self) -> usize {
        self.providers
            .iter()
            .map(|provider| provider.pending.len())
            .sum()
    }

    pub fn snapshot(&self) -> CoordinatorSnapshot {
        CoordinatorSnapshot {
            coordinator: self.id,
            invocation: self.current.as_ref().map(|current| current.id),
            query: self.current.as_ref().map(|current| current.query.clone()),
            state: self.aggregate,
            results: self.aggregated_results.clone(),
            providers: self
                .providers
                .iter()
                .map(|provider| ProviderSnapshot {
                    id: provider.spec.id.clone(),
                    state: provider.state,
                    result_count: provider.results.len(),
                    accepted: provider.accepted,
                    pending: provider.pending.len(),
                    deadline: provider.deadline,
                    failure: provider.failure.clone(),
                })
                .collect(),
            cancelled: self.cancelled,
            now: self.now,
        }
    }

    fn validate_invocation(&self, invocation: &Invocation) -> Result<(), CoordinatorError> {
        if invocation.coordinator != self.id {
            return Err(CoordinatorError::InvocationMismatch {
                expected: self.id,
                actual: invocation.coordinator,
            });
        }
        if invocation.id > self.last_issued {
            return Err(CoordinatorError::UnknownInvocation(invocation.id));
        }
        Ok(())
    }

    fn provider_index(&self, provider: &ProviderId) -> Result<usize, CoordinatorError> {
        self.index
            .get(provider)
            .copied()
            .ok_or_else(|| CoordinatorError::UnknownProvider(provider.clone()))
    }

    fn expire_provider(&mut self, index: usize, reason: &str) {
        let runtime = &mut self.providers[index];
        runtime.pending.clear();
        runtime.failure = Some(bounded_reason(reason));
        runtime.state = if runtime.results.is_empty() {
            SearchState::Failed
        } else {
            SearchState::Partial
        };
    }

    fn refresh_aggregate(&mut self) {
        let mut seen = HashSet::new();
        let mut results = Vec::new();
        for provider in &self.providers {
            for command in &provider.results {
                if seen.insert(command.id.clone()) {
                    results.push(command.clone());
                }
            }
        }
        self.aggregated_results = results;
        self.aggregate = self.compute_aggregate_state();
    }

    fn compute_aggregate_state(&self) -> SearchState {
        if self.cancelled {
            return SearchState::Cancelled;
        }
        let usable: Vec<&ProviderRuntime> = self
            .providers
            .iter()
            .filter(|provider| provider.state != SearchState::Unavailable)
            .collect();
        if usable.is_empty() {
            return SearchState::Unavailable;
        }

        let any_loading = usable
            .iter()
            .any(|provider| provider.state == SearchState::Loading);
        if any_loading {
            return SearchState::Loading;
        }

        let has_results = !self.aggregated_results.is_empty();
        let any_partial = usable
            .iter()
            .any(|provider| provider.state == SearchState::Partial);
        let any_failed = usable
            .iter()
            .any(|provider| provider.state == SearchState::Failed);

        if has_results {
            if any_partial || any_failed {
                SearchState::Partial
            } else {
                SearchState::Ready
            }
        } else if any_failed {
            SearchState::Failed
        } else if any_partial {
            SearchState::Partial
        } else {
            // Every usable provider is Ready or Empty with no results. A Ready
            // provider with zero results is only possible when its quota is
            // zero, which is still an empty result set, not a failure.
            SearchState::Empty
        }
    }
}

impl Default for Coordinator {
    fn default() -> Self {
        Self::new(Vec::new()).expect("an empty provider list is always valid")
    }
}

fn bounded_query(query: &str) -> String {
    query
        .chars()
        .take(MAX_COORDINATOR_QUERY_CHARS)
        .collect::<String>()
}

fn bounded_reason(reason: &str) -> String {
    reason
        .chars()
        .take(MAX_FAILURE_REASON_CHARS)
        .collect::<String>()
}

fn remove_pending(provider: &mut ProviderRuntime, invocation: InvocationId) {
    provider
        .pending
        .retain(|work| work.invocation != invocation);
}

/// Adapter that ranks a supplied in-memory catalog with [`ansa_search::rank`].
///
/// The catalog and the ranked output are de-duplicated by stable [`CommandId`]
/// identity, preserving the first occurrence. Ordering is exactly the ranker's
/// deterministic order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRankProvider {
    id: ProviderId,
    catalog: Vec<Command>,
    result_budget: usize,
}

impl LocalRankProvider {
    /// Creates a local adapter with the default provider result budget.
    pub fn new(id: ProviderId, catalog: Vec<Command>) -> Self {
        Self::new_with_budget(id, catalog, ProviderSpec::DEFAULT_RESULT_BUDGET)
    }

    /// Creates a local adapter whose `batch` output is capped to `result_budget`.
    ///
    /// Consumers that configure a smaller or larger coordinator budget should
    /// pass the same value here (or use [`LocalRankProvider::for_spec`]).
    pub fn new_with_budget(id: ProviderId, catalog: Vec<Command>, result_budget: usize) -> Self {
        Self {
            id,
            catalog: deduplicate(catalog),
            result_budget,
        }
    }

    /// Creates a local adapter from a provider spec, using its ID and effective
    /// result budget.
    pub fn for_spec(spec: &ProviderSpec, catalog: Vec<Command>) -> Self {
        Self::new_with_budget(spec.id.clone(), catalog, spec.result_budget)
    }

    pub fn id(&self) -> &ProviderId {
        &self.id
    }

    pub fn catalog(&self) -> &[Command] {
        &self.catalog
    }

    /// Result budget used by [`LocalRankProvider::batch`].
    pub fn result_budget(&self) -> usize {
        self.result_budget
    }

    /// Overrides the budget used by [`LocalRankProvider::batch`].
    pub fn with_result_budget(mut self, result_budget: usize) -> Self {
        self.result_budget = result_budget;
        self
    }

    /// Ranks the catalog for `query` with the supplied signals.
    pub fn rank(&self, query: &str, signals: &SearchSignals) -> Vec<Command> {
        deduplicate(
            rank_bounded(&self.catalog, query, signals, usize::MAX)
                .into_iter()
                .map(|result| result.command.clone())
                .collect(),
        )
    }

    /// Produces a complete batch for `invocation` using this adapter's stored
    /// result budget. Ranked output is truncated deterministically to that
    /// budget and marked [`ProviderBatch::is_truncated`] when a larger result
    /// set existed, so the batch cannot trigger `BatchOverBudget` when the
    /// coordinator uses the same budget.
    pub fn batch(&self, invocation: &Invocation, signals: &SearchSignals) -> ProviderBatch {
        self.batch_with_budget(invocation, signals, self.result_budget)
    }

    /// Produces a complete batch truncated to `result_budget` items.
    pub fn batch_with_budget(
        &self,
        invocation: &Invocation,
        signals: &SearchSignals,
        result_budget: usize,
    ) -> ProviderBatch {
        let mut ranked = rank_bounded(
            &self.catalog,
            invocation.query(),
            signals,
            result_budget.saturating_add(1),
        );
        let truncated = ranked.len() > result_budget;
        ranked.truncate(result_budget);
        let items = deduplicate(
            ranked
                .into_iter()
                .map(|result| result.command.clone())
                .collect(),
        );
        if truncated {
            ProviderBatch::complete_truncated(invocation, items)
        } else {
            ProviderBatch::complete(invocation, items)
        }
    }
}

fn deduplicate(commands: Vec<Command>) -> Vec<Command> {
    let mut seen = HashSet::with_capacity(commands.len());
    commands
        .into_iter()
        .filter(|command| seen.insert(command.id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ansa_domain::{BuiltinCommand, CommandKind};

    fn provider_id(raw: &str) -> ProviderId {
        ProviderId::new(raw).unwrap()
    }

    fn spec(raw: &str, quota: usize, budget: usize, deadline: u64, queue: usize) -> ProviderSpec {
        ProviderSpec::new(provider_id(raw))
            .with_quota(quota)
            .with_result_budget(budget)
            .with_deadline_ticks(deadline)
            .with_queue_limit(queue)
    }

    fn command(id: &str, title: &str) -> Command {
        Command::new(
            CommandId::parse(id).unwrap(),
            title,
            CommandKind::Builtin(BuiltinCommand::About),
        )
    }

    fn coordinator() -> (Coordinator, ProviderId, ProviderId) {
        let local = provider_id("local");
        let remote = provider_id("remote");
        let coordinator = Coordinator::new(vec![
            spec("local", 20, 50, 10, 2),
            spec("remote", 20, 50, 10, 2),
        ])
        .unwrap();
        (coordinator, local, remote)
    }

    fn batch_complete(invocation: &Invocation, ids: &[&str]) -> ProviderBatch {
        ProviderBatch::complete(
            invocation,
            ids.iter()
                .map(|id| command(id, &format!("Title {id}")))
                .collect(),
        )
    }

    /// Deterministic provider simulator. Batches are submitted in logical-tick
    /// order, regardless of the order they were scheduled, and `tick` runs
    /// before each submission so hangs can expire.
    #[derive(Debug, Clone)]
    struct SimulatedBatch {
        at: u64,
        provider: ProviderId,
        items: Vec<Command>,
        complete: bool,
    }

    struct Simulator {
        coordinator: Coordinator,
        batches: Vec<SimulatedBatch>,
    }

    impl Simulator {
        fn new(coordinator: Coordinator) -> Self {
            Self {
                coordinator,
                batches: Vec::new(),
            }
        }

        fn schedule(
            &mut self,
            at: u64,
            provider: &ProviderId,
            items: Vec<Command>,
            complete: bool,
        ) {
            self.batches.push(SimulatedBatch {
                at,
                provider: provider.clone(),
                items,
                complete,
            });
            // Stable sort: batches at the same tick retain scheduling order.
            self.batches.sort_by_key(|batch| batch.at);
        }

        fn next(&mut self, invocation: &Invocation) -> Option<(u64, BatchOutcome)> {
            let batch = self.batches.first()?.clone();
            self.batches.remove(0);
            self.coordinator.tick(batch.at);
            let provider_batch = if batch.complete {
                ProviderBatch::complete(invocation, batch.items)
            } else {
                ProviderBatch::new(invocation, batch.items)
            };
            let outcome = self
                .coordinator
                .submit(invocation, &batch.provider, provider_batch, batch.at)
                .unwrap();
            Some((batch.at, outcome))
        }
    }

    #[test]
    fn provider_ids_and_duplicate_registration_are_explicit_errors() {
        assert_eq!(ProviderId::new(""), Err(ProviderIdError::Empty));
        assert_eq!(
            ProviderId::new("bad id"),
            Err(ProviderIdError::InvalidCharacter(' '))
        );
        assert!(ProviderId::new("x".repeat(ProviderId::MAX_LEN + 1)).is_err());
        assert_eq!(ProviderId::new("local").unwrap().as_str(), "local");

        let id = provider_id("local");
        let error = Coordinator::new(vec![
            ProviderSpec::new(id.clone()),
            ProviderSpec::new(id.clone()),
        ])
        .unwrap_err();
        assert_eq!(error, CoordinatorError::DuplicateProvider(id));
    }

    #[test]
    fn invalid_spec_and_unknown_provider_are_explicit_errors() {
        let id = provider_id("bad");
        let error = Coordinator::new(vec![
            ProviderSpec::new(id.clone())
                .with_quota(10)
                .with_result_budget(5),
        ])
        .unwrap_err();
        assert!(matches!(
            error,
            CoordinatorError::InvalidProviderSpec { .. }
        ));

        let (mut coordinator, _, _) = coordinator();
        let invocation = coordinator.begin("term");
        let unknown = provider_id("missing");
        let error = coordinator
            .submit(
                &invocation,
                &unknown,
                ProviderBatch::complete(&invocation, Vec::new()),
                0,
            )
            .unwrap_err();
        assert_eq!(error, CoordinatorError::UnknownProvider(unknown));
    }

    #[test]
    fn duplicate_dispatch_is_coalesced_and_stale_dispatch_is_flagged() {
        let (mut coordinator, local, remote) = coordinator();
        let old = coordinator.begin("one");
        let first = coordinator.dispatch(&old).unwrap();
        assert_eq!(first.dispatches.len(), 2);
        assert!(!first.stale);
        assert!(!first.cancelled);
        for dispatch in &first.dispatches {
            assert_eq!(dispatch.deadline, 10);
        }
        assert_eq!(coordinator.pending_len(&local).unwrap(), 1);
        assert_eq!(coordinator.pending_len(&remote).unwrap(), 1);

        let second = coordinator.dispatch(&old).unwrap();
        assert!(second.dispatches.is_empty());
        assert_eq!(coordinator.pending_total(), 2);

        let fresh = coordinator.begin("two");
        let stale = coordinator.dispatch(&old).unwrap();
        assert!(stale.stale);
        assert!(stale.dispatches.is_empty());
        assert_eq!(coordinator.pending_total(), 0);
        let current = coordinator.dispatch(&fresh).unwrap();
        assert_eq!(current.dispatches.len(), 2);
    }

    #[test]
    fn deterministic_simulator_reorders_delays_and_expires_hangs() {
        let local = provider_id("local");
        let remote = provider_id("remote");
        let mut simulator = Simulator::new(
            Coordinator::new(vec![
                spec("local", 10, 10, 4, 2),
                spec("remote", 10, 10, 4, 2),
            ])
            .unwrap(),
        );

        let invocation = simulator.coordinator.begin_at("term", 0);
        simulator.coordinator.dispatch(&invocation).unwrap();

        // Scheduled remote-first, but the deterministic simulator releases by
        // tick, so the local result arrives first despite being scheduled later.
        simulator.schedule(3, &remote, vec![command("app:remote", "Remote")], true);
        simulator.schedule(1, &local, vec![command("app:local", "Local")], true);

        let (at, outcome) = simulator.next(&invocation).unwrap();
        assert_eq!(at, 1);
        assert_eq!(outcome, BatchOutcome::Completed { accepted: 1 });
        assert_eq!(simulator.coordinator.results().len(), 1);
        assert_eq!(simulator.coordinator.state(), SearchState::Loading);

        let (at, outcome) = simulator.next(&invocation).unwrap();
        assert_eq!(at, 3);
        assert_eq!(outcome, BatchOutcome::Completed { accepted: 1 });
        assert_eq!(simulator.coordinator.results().len(), 2);
        assert_eq!(simulator.coordinator.state(), SearchState::Ready);

        // A provider that never reports before its logical deadline is expired
        // by the simulator's tick, and its late batch is rejected.
        let hung = simulator.coordinator.begin_at("hang", 4);
        simulator.coordinator.dispatch(&hung).unwrap();
        simulator.schedule(9, &remote, vec![command("app:too-late", "Too Late")], true);
        let (at, outcome) = simulator.next(&hung).unwrap();
        assert_eq!(at, 9);
        assert_eq!(outcome, BatchOutcome::Expired);
        assert_eq!(simulator.coordinator.state(), SearchState::Failed);
        assert!(simulator.coordinator.results().is_empty());
    }

    #[test]
    fn late_and_reordered_batches_never_replace_a_newer_invocation() {
        let (mut coordinator, local, remote) = coordinator();
        let old = coordinator.begin("old");
        coordinator.dispatch(&old).unwrap();
        let current = coordinator.begin("new");
        coordinator.dispatch(&current).unwrap();

        let stale = coordinator
            .submit(&old, &local, batch_complete(&old, &["app:stale"]), 0)
            .unwrap();
        assert_eq!(stale, BatchOutcome::StaleInvocation);
        assert!(coordinator.results().is_empty());

        // A batch that carries an old invocation cannot be smuggled in under the
        // current invocation token.
        let smuggled = ProviderBatch::complete(&old, vec![command("app:smuggled", "Smuggled")]);
        let error = coordinator
            .submit(&current, &local, smuggled, 0)
            .unwrap_err();
        assert!(matches!(
            error,
            CoordinatorError::BatchInvocationMismatch { .. }
        ));

        let outcome = coordinator
            .submit(
                &current,
                &local,
                batch_complete(&current, &["app:fresh"]),
                0,
            )
            .unwrap();
        assert_eq!(outcome, BatchOutcome::Completed { accepted: 1 });
        assert_eq!(coordinator.results()[0].id.as_str(), "app:fresh");

        // The remote provider is still loading, so the aggregate is Loading even
        // though the local provider is ready. Results remain available.
        assert_eq!(coordinator.state(), SearchState::Loading);
        assert_eq!(coordinator.results().len(), 1);
        assert_eq!(
            coordinator.provider_state(&local).unwrap(),
            SearchState::Ready
        );
        assert_eq!(
            coordinator.provider_state(&remote).unwrap(),
            SearchState::Loading
        );
    }

    #[test]
    fn cancellation_is_best_effort_and_late_batches_are_still_ignored() {
        let (mut coordinator, local, _) = coordinator();
        let invocation = coordinator.begin("term");
        coordinator.dispatch(&invocation).unwrap();
        coordinator.cancel(&invocation).unwrap();
        assert!(coordinator.is_cancelled());
        assert_eq!(coordinator.state(), SearchState::Cancelled);
        assert!(coordinator.state().is_terminal());
        assert_eq!(
            coordinator.provider_state(&local).unwrap(),
            SearchState::Cancelled
        );

        let outcome = coordinator
            .submit(
                &invocation,
                &local,
                batch_complete(&invocation, &["app:ignored"]),
                0,
            )
            .unwrap();
        assert_eq!(outcome, BatchOutcome::Cancelled);
        assert!(coordinator.results().is_empty());
        assert!(coordinator.snapshot().cancelled);

        // A later tick cannot turn cancellation into a failure.
        assert_eq!(coordinator.tick(100), 0);
        assert_eq!(coordinator.state(), SearchState::Cancelled);
        assert!(coordinator.state().is_terminal());

        // A new invocation clears the cancellation flag and starts clean.
        let next = coordinator.begin("next");
        assert!(!coordinator.is_cancelled());
        assert_eq!(coordinator.current_invocation().unwrap().id(), next.id());
    }

    #[test]
    fn tick_expires_hanging_providers_and_rejects_late_batches() {
        let (mut coordinator, local, remote) = coordinator();
        let invocation = coordinator.begin_at("hang", 0);
        coordinator.dispatch(&invocation).unwrap();
        assert_eq!(coordinator.tick(9), 0);
        assert_eq!(
            coordinator.provider_state(&local).unwrap(),
            SearchState::Loading
        );
        assert_eq!(coordinator.tick(10), 2);
        assert_eq!(
            coordinator.provider_state(&local).unwrap(),
            SearchState::Failed
        );
        assert_eq!(coordinator.state(), SearchState::Failed);
        assert_eq!(coordinator.pending_total(), 0);

        let outcome = coordinator
            .submit(
                &invocation,
                &remote,
                batch_complete(&invocation, &["app:late"]),
                10,
            )
            .unwrap();
        assert_eq!(outcome, BatchOutcome::Expired);
        assert!(coordinator.results().is_empty());
    }

    #[test]
    fn partial_results_survive_a_later_failure_and_are_not_collapsed() {
        let (mut coordinator, local, remote) = coordinator();
        let invocation = coordinator.begin("term");
        coordinator.dispatch(&invocation).unwrap();

        coordinator
            .submit(
                &invocation,
                &local,
                batch_complete(&invocation, &["app:local"]),
                0,
            )
            .unwrap();
        let partial = coordinator
            .submit(
                &invocation,
                &remote,
                ProviderBatch::new(&invocation, vec![command("app:remote-partial", "Partial")]),
                0,
            )
            .unwrap();
        assert_eq!(partial, BatchOutcome::Accepted { accepted: 1 });
        assert_eq!(
            coordinator.provider_state(&remote).unwrap(),
            SearchState::Loading
        );
        assert_eq!(coordinator.state(), SearchState::Loading);
        assert!(coordinator.results().len() == 2);

        let failed = coordinator
            .fail(&invocation, &remote, "connection reset")
            .unwrap();
        assert_eq!(failed, BatchOutcome::Failed { retained: 1 });
        assert_eq!(
            coordinator.provider_state(&remote).unwrap(),
            SearchState::Partial
        );
        assert_eq!(coordinator.state(), SearchState::Partial);
        assert!(coordinator.state().is_terminal());
        assert_eq!(coordinator.results().len(), 2);
        assert!(
            coordinator
                .snapshot()
                .providers
                .iter()
                .any(|provider| provider.failure.as_deref() == Some("connection reset"))
        );

        let failed_outcome = coordinator
            .submit(
                &invocation,
                &local,
                batch_complete(&invocation, &["app:local-2"]),
                0,
            )
            .unwrap();
        // A ready provider is terminal: a duplicate final batch is acknowledged
        // without appending new identities.
        assert_eq!(failed_outcome, BatchOutcome::Completed { accepted: 0 });
        assert_eq!(coordinator.results().len(), 2);
        assert_eq!(coordinator.state(), SearchState::Partial);
        assert!(coordinator.state().is_terminal());
    }

    #[test]
    fn local_results_are_available_while_remote_is_slow_or_failed() {
        let (mut coordinator, local, remote) = coordinator();
        let invocation = coordinator.begin("term");
        coordinator.dispatch(&invocation).unwrap();

        coordinator
            .submit(
                &invocation,
                &local,
                batch_complete(&invocation, &["app:local"]),
                0,
            )
            .unwrap();
        assert_eq!(coordinator.results().len(), 1);
        assert_eq!(coordinator.state(), SearchState::Loading);

        coordinator.fail(&invocation, &remote, "timeout").unwrap();
        assert_eq!(coordinator.results().len(), 1);
        assert_eq!(coordinator.state(), SearchState::Partial);
        assert!(coordinator.state().is_terminal());

        // A terminal failed provider cannot be resurrected by a late complete
        // batch, and local results remain available.
        let late = coordinator
            .submit(
                &invocation,
                &remote,
                batch_complete(&invocation, &["app:remote"]),
                0,
            )
            .unwrap();
        assert_eq!(late, BatchOutcome::Failed { retained: 0 });
        assert_eq!(coordinator.results().len(), 1);
        assert_eq!(coordinator.state(), SearchState::Partial);
    }

    #[test]
    fn all_complete_results_are_ready_and_empty_is_distinct_from_failure() {
        let (mut coordinator, local, remote) = coordinator();
        let invocation = coordinator.begin("term");
        coordinator.dispatch(&invocation).unwrap();
        coordinator
            .submit(
                &invocation,
                &local,
                batch_complete(&invocation, &["app:local"]),
                0,
            )
            .unwrap();
        coordinator
            .submit(
                &invocation,
                &remote,
                batch_complete(&invocation, &["app:remote"]),
                0,
            )
            .unwrap();
        assert_eq!(coordinator.state(), SearchState::Ready);
        assert_eq!(coordinator.results().len(), 2);

        let empty = coordinator.begin("empty");
        coordinator.dispatch(&empty).unwrap();
        coordinator
            .submit(
                &empty,
                &local,
                ProviderBatch::complete(&empty, Vec::new()),
                0,
            )
            .unwrap();
        coordinator
            .submit(
                &empty,
                &remote,
                ProviderBatch::complete(&empty, Vec::new()),
                0,
            )
            .unwrap();
        assert_eq!(coordinator.state(), SearchState::Empty);

        let failed = coordinator.begin("failed");
        coordinator.dispatch(&failed).unwrap();
        coordinator.fail(&failed, &local, "boom").unwrap();
        coordinator.fail(&failed, &remote, "boom").unwrap();
        assert_eq!(coordinator.state(), SearchState::Failed);
    }

    #[test]
    fn unavailable_providers_are_excluded_and_reported() {
        let local = provider_id("local");
        let remote = provider_id("remote");
        let mut coordinator = Coordinator::new(vec![
            spec("local", 10, 10, 5, 2),
            spec("remote", 10, 10, 5, 2).unavailable(),
        ])
        .unwrap();
        let invocation = coordinator.begin("term");
        coordinator.dispatch(&invocation).unwrap();
        assert_eq!(
            coordinator.provider_state(&remote).unwrap(),
            SearchState::Unavailable
        );
        coordinator
            .submit(
                &invocation,
                &local,
                batch_complete(&invocation, &["app:local"]),
                0,
            )
            .unwrap();
        assert_eq!(coordinator.state(), SearchState::Ready);

        let only_unavailable =
            Coordinator::new(vec![ProviderSpec::new(provider_id("remote")).unavailable()]).unwrap();
        assert_eq!(only_unavailable.state(), SearchState::Unavailable);
    }

    #[test]
    fn quota_exhaustion_drops_excess_without_unbounded_growth() {
        let local = provider_id("local");
        let mut coordinator = Coordinator::new(vec![spec("local", 2, 10, 10, 2)]).unwrap();
        let invocation = coordinator.begin("term");
        coordinator.dispatch(&invocation).unwrap();

        let outcome = coordinator
            .submit(
                &invocation,
                &local,
                batch_complete(&invocation, &["app:1", "app:2", "app:3", "app:4", "app:5"]),
                0,
            )
            .unwrap();
        assert_eq!(
            outcome,
            BatchOutcome::QuotaExhausted {
                accepted: 2,
                dropped: 3
            }
        );
        assert_eq!(coordinator.results().len(), 2);
        assert_eq!(
            coordinator.provider_state(&local).unwrap(),
            SearchState::Ready
        );

        let again = coordinator
            .submit(
                &invocation,
                &local,
                batch_complete(&invocation, &["app:6"]),
                0,
            )
            .unwrap();
        assert_eq!(
            again,
            BatchOutcome::QuotaExhausted {
                accepted: 0,
                dropped: 1
            }
        );
        assert_eq!(coordinator.results().len(), 2);
        assert_eq!(coordinator.state(), SearchState::Ready);
    }

    #[test]
    fn over_budget_batches_are_errors_and_do_not_change_state() {
        let local = provider_id("local");
        let mut coordinator = Coordinator::new(vec![spec("local", 3, 3, 10, 2)]).unwrap();
        let invocation = coordinator.begin("term");
        let before = coordinator.snapshot();
        let error = coordinator
            .submit(
                &invocation,
                &local,
                batch_complete(&invocation, &["app:1", "app:2", "app:3", "app:4"]),
                50,
            )
            .unwrap_err();
        assert_eq!(
            error,
            CoordinatorError::BatchOverBudget {
                provider: local,
                size: 4,
                budget: 3
            }
        );
        // The rejected batch must not advance the clock or mutate the snapshot.
        assert_eq!(coordinator.snapshot(), before);
        assert_eq!(coordinator.now(), 0);
    }

    #[test]
    fn duplicate_items_are_deduplicated_by_stable_identity() {
        let (mut coordinator, local, _) = coordinator();
        let invocation = coordinator.begin("term");
        coordinator.dispatch(&invocation).unwrap();
        coordinator
            .submit(
                &invocation,
                &local,
                batch_complete(&invocation, &["app:a", "app:a", "app:b"]),
                0,
            )
            .unwrap();
        assert_eq!(coordinator.results().len(), 2);
        assert_eq!(coordinator.results()[0].id.as_str(), "app:a");
        assert_eq!(coordinator.results()[1].id.as_str(), "app:b");
    }

    #[test]
    fn aggregate_results_keep_provider_registration_order_and_identity_dedup() {
        let (mut coordinator, local, remote) = coordinator();
        let invocation = coordinator.begin("term");
        coordinator.dispatch(&invocation).unwrap();
        coordinator
            .submit(
                &invocation,
                &remote,
                ProviderBatch::new(&invocation, vec![command("app:shared", "Remote Shared")]),
                0,
            )
            .unwrap();
        coordinator
            .submit(
                &invocation,
                &local,
                ProviderBatch::new(
                    &invocation,
                    vec![
                        command("app:local", "Local"),
                        command("app:shared", "Local Shared"),
                    ],
                ),
                0,
            )
            .unwrap();

        let results: Vec<&str> = coordinator
            .results()
            .iter()
            .map(|command| command.id.as_str())
            .collect();
        // Local was registered first, so its ordering wins, including the shared
        // identity, and the remote duplicate is dropped.
        assert_eq!(results, vec!["app:local", "app:shared"]);
        assert_eq!(coordinator.result_count(), 2);
    }

    #[test]
    fn query_flood_keeps_queues_and_results_bounded() {
        let global_limit = 3;
        let mut coordinator = Coordinator::with_config(
            vec![
                spec("a", 5, 10, 50, 2),
                spec("b", 5, 10, 50, 2),
                spec("c", 5, 10, 50, 2),
            ],
            CoordinatorConfig::new(global_limit),
        )
        .unwrap();

        let mut oldest = None;
        for index in 0..10_000u64 {
            let invocation = coordinator.begin_at(&format!("query-{index}"), index);
            if oldest.is_none() {
                oldest = Some(invocation.clone());
            }
            let report = coordinator.dispatch(&invocation).unwrap();
            assert!(!report.stale);
            assert_eq!(report.dispatches.len(), 3);
            assert_eq!(report.evicted, 0);
            assert!(
                coordinator.pending_total() <= global_limit,
                "pending={} limit={}",
                coordinator.pending_total(),
                global_limit
            );
            for provider in &report.dispatches {
                assert!(coordinator.pending_len(&provider.provider).unwrap() <= 2);
            }
            assert!(coordinator.results().is_empty());
            assert_eq!(coordinator.result_count(), 0);
        }

        let oldest = oldest.unwrap();
        let stale = coordinator
            .submit(
                &oldest,
                &provider_id("a"),
                batch_complete(&oldest, &["app:stale"]),
                10_000,
            )
            .unwrap();
        assert_eq!(stale, BatchOutcome::StaleInvocation);
        assert!(coordinator.results().is_empty());

        let snapshot = coordinator.snapshot();
        assert_eq!(
            snapshot.invocation,
            coordinator.current_invocation().map(|i| i.id())
        );
        assert!(
            snapshot
                .providers
                .iter()
                .all(|provider| provider.result_count == 0)
        );
        assert!(
            snapshot
                .providers
                .iter()
                .map(|provider| provider.pending)
                .sum::<usize>()
                <= global_limit
        );
    }

    #[test]
    fn selection_snapshot_preserves_identity_as_rows_arrive_late() {
        let rows = vec![
            command("app:a", "Alpha"),
            command("app:b", "Beta"),
            command("app:c", "Gamma"),
            command("app:d", "Delta"),
        ];
        let selected = CommandId::parse("app:c").unwrap();
        let mut selection = SelectionSnapshot::capture(&rows, Some(&selected));
        assert_eq!(selection.selected_index(), Some(2));

        // A late higher-ranked row is inserted before the selection. The
        // selected identity stays selected; only its index changes.
        let mut grown = vec![command("app:late", "Late")];
        grown.extend(rows.clone());
        assert_eq!(selection.apply_rows(&grown), SelectionOutcome::Retained);
        assert_eq!(selection.selected().unwrap().as_str(), "app:c");
        assert_eq!(selection.selected_index(), Some(3));

        // Navigation remains anchored to identity.
        selection.move_by(-1);
        assert_eq!(selection.selected().unwrap().as_str(), "app:b");
        selection.move_by(10);
        assert_eq!(selection.selected().unwrap().as_str(), "app:d");
        selection.move_by(-100);
        assert_eq!(selection.selected().unwrap().as_str(), "app:late");

        // Removing the selected identity repairs selection deterministically.
        let repaired = vec![command("app:a", "Alpha"), command("app:b", "Beta")];
        assert_eq!(selection.apply_rows(&repaired), SelectionOutcome::Repaired);
        assert_eq!(selection.selected().unwrap().as_str(), "app:a");

        // Empty rows clear selection.
        assert_eq!(selection.apply_rows(&[]), SelectionOutcome::Cleared);
        assert!(selection.selected().is_none());
        assert_eq!(selection.selected_index(), None);
    }

    #[test]
    fn property_selection_stability_over_many_late_row_lists() {
        let base: Vec<Command> = (0..24)
            .map(|index| command(&format!("app:{index:02}"), &format!("Title {index:02}")))
            .collect();
        for selected_index in 0..base.len() {
            let selected = base[selected_index].id.clone();
            let mut selection = SelectionSnapshot::capture(&base, Some(&selected));
            for round in 0..40usize {
                // Build a row list that sometimes inserts new rows before the
                // selection, sometimes reorders, and sometimes omits the
                // selected row entirely.
                let mut rows = Vec::new();
                if round % 3 != 0 {
                    rows.push(command("app:new", "New"));
                }
                for (index, row) in base.iter().enumerate() {
                    if round % 7 == 0 && index == selected_index {
                        continue;
                    }
                    rows.push(row.clone());
                }
                if round % 2 == 0 {
                    rows.reverse();
                }
                let previous = selection.selected().cloned();
                let prior_present = previous
                    .as_ref()
                    .is_some_and(|id| rows.iter().any(|row| &row.id == id));
                let expected = if prior_present {
                    previous
                } else {
                    rows.first().map(|row| row.id.clone())
                };
                let expected_outcome = if prior_present {
                    SelectionOutcome::Retained
                } else if expected.is_some() {
                    SelectionOutcome::Repaired
                } else {
                    SelectionOutcome::Cleared
                };
                assert_eq!(selection.apply_rows(&rows), expected_outcome);
                assert_eq!(selection.selected().cloned(), expected);
            }
        }
    }

    #[test]
    fn local_rank_provider_adapter_ranks_and_deduplicates() {
        let catalog = vec![
            command("app:shell", "Shell Settings"),
            command("app:settings", "Settings"),
            command("app:settings", "Duplicate Settings"),
            command("app:term", "Terminal"),
        ];
        let provider = LocalRankProvider::new(provider_id("local"), catalog);
        assert_eq!(provider.catalog().len(), 3);
        assert_eq!(provider.id().as_str(), "local");

        let ranked = provider.rank("settings", &SearchSignals::new());
        let ids: Vec<&str> = ranked.iter().map(|command| command.id.as_str()).collect();
        assert_eq!(ids, vec!["app:settings", "app:shell"]);

        let mut coordinator = Coordinator::new(vec![spec("local", 10, 10, 10, 2)]).unwrap();
        let invocation = coordinator.begin("settings");
        coordinator.dispatch(&invocation).unwrap();
        let outcome = coordinator
            .submit(
                &invocation,
                provider.id(),
                provider.batch(&invocation, &SearchSignals::new()),
                0,
            )
            .unwrap();
        assert_eq!(outcome, BatchOutcome::Completed { accepted: 2 });
        assert_eq!(coordinator.results()[0].id.as_str(), "app:settings");
    }

    #[test]
    fn invocation_tokens_from_another_coordinator_are_errors() {
        let (mut first, _, _) = coordinator();
        let (mut second, _, _) = coordinator();
        let foreign = first.begin("term");
        let error = second.dispatch(&foreign).unwrap_err();
        assert!(matches!(error, CoordinatorError::InvocationMismatch { .. }));

        // An invocation id that was never issued is also explicit. The tests
        // are a child module, so this deliberately forges the opaque identity.
        let mut forged = first.begin("next");
        forged.id = InvocationId(u64::MAX);
        let error = first.dispatch(&forged).unwrap_err();
        assert_eq!(
            error,
            CoordinatorError::UnknownInvocation(InvocationId(u64::MAX))
        );
    }

    #[test]
    fn local_rank_provider_uses_configured_budget() {
        let catalog: Vec<Command> = (0..100)
            .map(|index| command(&format!("app:{index:03}"), &format!("Catalog {index:03}")))
            .collect();
        let provider =
            LocalRankProvider::new_with_budget(provider_id("local"), catalog.clone(), 10);
        assert_eq!(provider.result_budget(), 10);
        let signals = SearchSignals::new();

        let spec = ProviderSpec::new(provider_id("local"))
            .with_quota(10)
            .with_result_budget(10);
        let mut coordinator = Coordinator::new(vec![spec.clone()]).unwrap();
        let invocation = coordinator.begin("");
        coordinator.dispatch(&invocation).unwrap();
        let batch = provider.batch(&invocation, &signals);
        assert_eq!(batch.len(), 10);
        assert!(batch.is_truncated());
        let outcome = coordinator
            .submit(&invocation, provider.id(), batch, 0)
            .unwrap();
        assert_eq!(outcome, BatchOutcome::CompletedTruncated { accepted: 10 });
        assert_eq!(coordinator.results().len(), 10);
        assert_eq!(
            coordinator.provider_state(provider.id()).unwrap(),
            SearchState::Ready
        );

        let from_spec = LocalRankProvider::for_spec(&spec, catalog.clone());
        assert_eq!(from_spec.result_budget(), 10);
        let overridden =
            LocalRankProvider::new(provider_id("local"), catalog).with_result_budget(5);
        assert_eq!(overridden.result_budget(), 5);
        assert_eq!(overridden.batch(&invocation, &signals).len(), 5);

        // A zero budget is still reported as a deliberate truncation rather
        // than a plain Empty outcome.
        let zero_catalog: Vec<Command> = (0..3)
            .map(|index| command(&format!("app:z{index}"), &format!("Zero {index}")))
            .collect();
        let zero_provider =
            LocalRankProvider::new_with_budget(provider_id("zero"), zero_catalog, 0);
        let mut zero_coordinator = Coordinator::new(vec![
            ProviderSpec::new(provider_id("zero"))
                .with_quota(0)
                .with_result_budget(0),
        ])
        .unwrap();
        let invocation = zero_coordinator.begin("");
        zero_coordinator.dispatch(&invocation).unwrap();
        let batch = zero_provider.batch(&invocation, &signals);
        assert_eq!(batch.len(), 0);
        assert!(batch.is_truncated());
        let outcome = zero_coordinator
            .submit(&invocation, zero_provider.id(), batch, 0)
            .unwrap();
        assert_eq!(outcome, BatchOutcome::CompletedTruncated { accepted: 0 });
        assert_eq!(zero_coordinator.state(), SearchState::Empty);
    }

    #[test]
    fn stale_submission_does_not_poison_the_clock() {
        let (mut coordinator, local, _) = coordinator();
        let old = coordinator.begin_at("old", 0);
        coordinator.dispatch(&old).unwrap();
        let current = coordinator.begin_at("new", 0);
        coordinator.dispatch(&current).unwrap();

        let stale = coordinator
            .submit(
                &old,
                &local,
                batch_complete(&old, &["app:stale"]),
                u64::MAX - 1,
            )
            .unwrap();
        assert_eq!(stale, BatchOutcome::StaleInvocation);
        assert_eq!(coordinator.now(), 0);
        assert_eq!(
            coordinator.provider_state(&local).unwrap(),
            SearchState::Loading
        );

        // The current query still follows its normal deadline, rather than
        // being expired by the stale batch's far-future timestamp.
        assert_eq!(coordinator.tick(9), 0);
        assert_eq!(coordinator.tick(10), 2);
        assert_eq!(coordinator.state(), SearchState::Failed);
        assert_eq!(coordinator.now(), 10);

        // Cancellation is also settled without clock advancement.
        let cancelled = coordinator.begin_at("cancel", 10);
        coordinator.dispatch(&cancelled).unwrap();
        coordinator.cancel(&cancelled).unwrap();
        let outcome = coordinator
            .submit(
                &cancelled,
                &local,
                batch_complete(&cancelled, &["app:ignored"]),
                u64::MAX - 1,
            )
            .unwrap();
        assert_eq!(outcome, BatchOutcome::Cancelled);
        assert_eq!(coordinator.now(), 10);
        assert_eq!(coordinator.state(), SearchState::Cancelled);
    }

    #[test]
    fn local_rank_provider_caps_default_budget_and_reports_truncation() {
        let catalog: Vec<Command> = (0..1_000)
            .map(|index| command(&format!("app:{index:04}"), &format!("Catalog {index:04}")))
            .collect();
        let provider = LocalRankProvider::new(provider_id("local"), catalog);
        let signals = SearchSignals::new();

        let mut coordinator =
            Coordinator::new(vec![ProviderSpec::new(provider_id("local"))]).unwrap();
        let invocation = coordinator.begin("");
        coordinator.dispatch(&invocation).unwrap();
        let batch = provider.batch(&invocation, &signals);
        assert_eq!(batch.len(), ProviderSpec::DEFAULT_RESULT_BUDGET);
        assert!(batch.is_truncated());
        let outcome = coordinator
            .submit(&invocation, provider.id(), batch, 0)
            .unwrap();
        assert_eq!(
            outcome,
            BatchOutcome::QuotaExhausted {
                accepted: 50,
                dropped: 150
            }
        );
        assert_eq!(coordinator.results().len(), 50);
        assert_eq!(
            coordinator.provider_state(provider.id()).unwrap(),
            SearchState::Ready
        );
        assert!(coordinator.state().is_terminal());

        // With a quota as large as the batch, the truncation is still reported
        // as an explicit outcome rather than an over-budget error.
        let mut roomy = Coordinator::new(vec![
            ProviderSpec::new(provider_id("local"))
                .with_quota(200)
                .with_result_budget(200),
        ])
        .unwrap();
        let invocation = roomy.begin("");
        roomy.dispatch(&invocation).unwrap();
        let batch = provider.batch_with_budget(&invocation, &signals, 200);
        assert!(batch.is_truncated());
        assert_eq!(batch.len(), 200);
        let outcome = roomy.submit(&invocation, provider.id(), batch, 0).unwrap();
        assert_eq!(outcome, BatchOutcome::CompletedTruncated { accepted: 200 });
        assert_eq!(roomy.results().len(), 200);
        assert!(roomy.state().is_terminal());
    }

    #[test]
    fn partial_results_finalize_at_the_provider_deadline() {
        let (mut coordinator, local, _) = coordinator();
        let invocation = coordinator.begin_at("partial", 0);
        coordinator.dispatch(&invocation).unwrap();
        let outcome = coordinator
            .submit(
                &invocation,
                &local,
                ProviderBatch::new(&invocation, vec![command("app:partial", "Partial")]),
                0,
            )
            .unwrap();
        assert_eq!(outcome, BatchOutcome::Accepted { accepted: 1 });
        assert_eq!(
            coordinator.provider_state(&local).unwrap(),
            SearchState::Loading
        );
        assert_eq!(coordinator.state(), SearchState::Loading);
        // A provider that already received this invocation's work is not
        // dispatched again just because it returned a non-final batch.
        assert!(
            coordinator
                .dispatch(&invocation)
                .unwrap()
                .dispatches
                .is_empty()
        );

        assert_eq!(coordinator.tick(10), 2);
        assert_eq!(
            coordinator.provider_state(&local).unwrap(),
            SearchState::Partial
        );
        assert_eq!(coordinator.state(), SearchState::Partial);
        assert!(coordinator.state().is_terminal());
        assert_eq!(coordinator.results().len(), 1);
        assert!(
            coordinator
                .snapshot()
                .providers
                .iter()
                .any(|provider| provider.state == SearchState::Partial
                    && provider.failure.as_deref() == Some("deadline exceeded"))
        );

        // Further ticks are stable; no provider remains in flight.
        assert_eq!(coordinator.tick(1_000), 0);
        assert_eq!(coordinator.state(), SearchState::Partial);
        assert!(coordinator.state().is_terminal());
    }

    #[test]
    fn complete_success_is_not_downgraded_by_a_late_failure() {
        let (mut coordinator, local, remote) = coordinator();
        let invocation = coordinator.begin("term");
        coordinator.dispatch(&invocation).unwrap();
        coordinator
            .submit(
                &invocation,
                &local,
                batch_complete(&invocation, &["app:local"]),
                0,
            )
            .unwrap();
        coordinator
            .submit(
                &invocation,
                &remote,
                batch_complete(&invocation, &["app:remote"]),
                0,
            )
            .unwrap();
        assert_eq!(coordinator.state(), SearchState::Ready);
        assert!(coordinator.state().is_terminal());

        let late = coordinator
            .fail(&invocation, &local, "late failure")
            .unwrap();
        assert_eq!(
            late,
            BatchOutcome::Settled {
                state: SearchState::Ready
            }
        );
        assert_eq!(
            coordinator.provider_state(&local).unwrap(),
            SearchState::Ready
        );
        assert_eq!(coordinator.state(), SearchState::Ready);
        assert!(coordinator.state().is_terminal());
        assert_eq!(coordinator.results().len(), 2);
    }

    #[test]
    fn batches_from_another_coordinator_are_rejected() {
        let (mut first, local, _) = coordinator();
        let (mut second, _, _) = coordinator();
        let first_invocation = first.begin("first");
        let second_invocation = second.begin("second");
        assert_eq!(first_invocation.id(), second_invocation.id());
        first.dispatch(&first_invocation).unwrap();

        let error = first
            .submit(
                &first_invocation,
                &local,
                ProviderBatch::complete(
                    &second_invocation,
                    vec![command("app:foreign", "Foreign")],
                ),
                0,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            CoordinatorError::BatchCoordinatorMismatch { .. }
        ));
        assert!(first.results().is_empty());
    }

    #[test]
    fn unavailable_provider_is_sticky_and_not_dispatchable() {
        let unavailable = provider_id("unavailable");
        let mut coordinator =
            Coordinator::new(vec![ProviderSpec::new(unavailable.clone()).unavailable()]).unwrap();
        let invocation = coordinator.begin("term");
        let report = coordinator.dispatch(&invocation).unwrap();
        assert!(report.dispatches.is_empty());
        assert_eq!(coordinator.pending_total(), 0);
        assert_eq!(
            coordinator.provider_state(&unavailable).unwrap(),
            SearchState::Unavailable
        );

        let error = coordinator
            .submit(
                &invocation,
                &unavailable,
                ProviderBatch::complete(&invocation, vec![command("app:x", "X")]),
                1_000,
            )
            .unwrap_err();
        assert_eq!(
            error,
            CoordinatorError::ProviderUnavailable(unavailable.clone())
        );
        let error = coordinator
            .fail(&invocation, &unavailable, "boom")
            .unwrap_err();
        assert_eq!(
            error,
            CoordinatorError::ProviderUnavailable(unavailable.clone())
        );
        assert_eq!(coordinator.tick(1_000), 0);
        assert_eq!(
            coordinator.provider_state(&unavailable).unwrap(),
            SearchState::Unavailable
        );
        assert_eq!(coordinator.state(), SearchState::Unavailable);
        assert!(coordinator.state().is_terminal());
    }

    #[test]
    fn queue_limits_reject_instead_of_evicting_live_work() {
        let mut tight = Coordinator::with_config(
            vec![
                spec("a", 5, 10, 50, 2),
                spec("b", 5, 10, 50, 2),
                spec("c", 5, 10, 50, 2),
            ],
            CoordinatorConfig::new(1),
        )
        .unwrap();
        let invocation = tight.begin("term");
        let error = tight.dispatch(&invocation).unwrap_err();
        assert_eq!(
            error,
            CoordinatorError::GlobalQueueFull {
                limit: 1,
                requested: 3,
                queued: 0
            }
        );
        assert_eq!(tight.pending_total(), 0);
        assert!(
            tight
                .snapshot()
                .providers
                .iter()
                .all(|provider| provider.pending == 0)
        );

        let mut roomy = Coordinator::with_config(
            vec![
                spec("a", 5, 10, 50, 2),
                spec("b", 5, 10, 50, 2),
                spec("c", 5, 10, 50, 2),
            ],
            CoordinatorConfig::new(3),
        )
        .unwrap();
        let invocation = roomy.begin("term");
        let report = roomy.dispatch(&invocation).unwrap();
        assert_eq!(report.dispatches.len(), 3);
        assert_eq!(report.evicted, 0);
        assert_eq!(roomy.pending_total(), 3);
        for dispatch in &report.dispatches {
            assert_eq!(roomy.pending_len(&dispatch.provider).unwrap(), 1);
        }
        roomy
            .submit(
                &invocation,
                &provider_id("a"),
                batch_complete(&invocation, &["app:a"]),
                0,
            )
            .unwrap();
        assert_eq!(roomy.pending_len(&provider_id("a")).unwrap(), 0);
        assert_eq!(roomy.pending_len(&provider_id("b")).unwrap(), 1);
        assert_eq!(roomy.pending_len(&provider_id("c")).unwrap(), 1);
        let duplicate = roomy.dispatch(&invocation).unwrap();
        assert!(duplicate.dispatches.is_empty());
        assert_eq!(roomy.pending_total(), 2);

        let zero_queue = Coordinator::new(vec![
            ProviderSpec::new(provider_id("zero")).with_queue_limit(0),
        ]);
        assert!(matches!(
            zero_queue,
            Err(CoordinatorError::InvalidProviderSpec { .. })
        ));
    }

    #[test]
    fn duplicate_final_batch_reports_completed_not_empty() {
        let (mut coordinator, local, _) = coordinator();
        let invocation = coordinator.begin("term");
        let first = coordinator
            .submit(
                &invocation,
                &local,
                batch_complete(&invocation, &["app:only"]),
                0,
            )
            .unwrap();
        assert_eq!(first, BatchOutcome::Completed { accepted: 1 });
        let duplicate = coordinator
            .submit(
                &invocation,
                &local,
                batch_complete(&invocation, &["app:only"]),
                0,
            )
            .unwrap();
        assert_eq!(duplicate, BatchOutcome::Completed { accepted: 0 });
        assert_eq!(
            coordinator.provider_state(&local).unwrap(),
            SearchState::Ready
        );
        assert_eq!(coordinator.results().len(), 1);
    }

    #[test]
    fn invocation_exhaustion_is_explicit_and_fail_closed() {
        let mut coordinator = Coordinator::new(vec![spec("local", 1, 1, 5, 1)]).unwrap();
        coordinator.last_issued = InvocationId(u64::MAX);
        assert!(coordinator.is_exhausted());
        let error = coordinator.try_begin_at("exhausted", 1).unwrap_err();
        assert_eq!(error, CoordinatorError::InvocationIdExhausted);

        let sentinel = coordinator.begin_at("exhausted", 2);
        assert!(coordinator.current_invocation().is_none());
        let report = coordinator.dispatch(&sentinel).unwrap();
        assert!(report.stale);
        assert!(coordinator.results().is_empty());
    }

    #[test]
    fn selection_stability_interleaves_navigation_and_coordinator_snapshots() {
        let (mut coordinator, local, remote) = coordinator();
        let invocation = coordinator.begin("term");
        coordinator.dispatch(&invocation).unwrap();
        coordinator
            .submit(
                &invocation,
                &local,
                batch_complete(&invocation, &["app:a", "app:b", "app:c", "app:d", "app:e"]),
                0,
            )
            .unwrap();

        let snapshot = coordinator.snapshot();
        let mut selection = SelectionSnapshot::capture(&snapshot.results, None);
        selection.move_by(2);
        assert_eq!(selection.selected().unwrap().as_str(), "app:c");

        coordinator
            .submit(
                &invocation,
                &remote,
                ProviderBatch::new(&invocation, vec![command("app:f", "F")]),
                0,
            )
            .unwrap();
        let snapshot = coordinator.snapshot();
        assert_eq!(
            selection.apply_rows(&snapshot.results),
            SelectionOutcome::Retained
        );
        assert_eq!(selection.selected().unwrap().as_str(), "app:c");

        selection.move_by(1);
        assert_eq!(selection.selected().unwrap().as_str(), "app:d");
        coordinator
            .submit(
                &invocation,
                &remote,
                ProviderBatch::complete(&invocation, vec![command("app:g", "G")]),
                0,
            )
            .unwrap();
        let snapshot = coordinator.snapshot();
        assert_eq!(
            selection.apply_rows(&snapshot.results),
            SelectionOutcome::Retained
        );
        assert_eq!(selection.selected().unwrap().as_str(), "app:d");
        assert_eq!(selection.selected_index(), Some(3));

        // Duplicate row identities are still stable by first occurrence.
        let duplicate_rows = vec![
            CommandId::parse("app:d").unwrap(),
            CommandId::parse("app:d").unwrap(),
            CommandId::parse("app:e").unwrap(),
        ];
        let mut duplicates =
            SelectionSnapshot::from_ids(duplicate_rows, Some(CommandId::parse("app:e").unwrap()));
        assert_eq!(duplicates.selected_index(), Some(2));
        duplicates.move_by(-1);
        assert_eq!(duplicates.selected().unwrap().as_str(), "app:d");
        assert_eq!(duplicates.selected_index(), Some(0));
    }
}
