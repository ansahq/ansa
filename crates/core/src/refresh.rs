//! Bounded refresh scheduling: one active task and at most one coalesced rerun.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshTicket(u64);

#[derive(Debug, Default)]
pub struct RefreshGate {
    next: u64,
    active: Option<RefreshTicket>,
    queued: bool,
}

impl RefreshGate {
    /// Repeated requests while busy set one pending rerun, not an unbounded queue.
    pub fn request(&mut self) -> Option<RefreshTicket> {
        if self.active.is_some() {
            self.queued = true;
            return None;
        }
        self.next = self
            .next
            .checked_add(1)
            .expect("refresh ticket space exhausted");
        let ticket = RefreshTicket(self.next);
        self.active = Some(ticket);
        Some(ticket)
    }

    /// Only the active task may publish its result. Stale completions do nothing.
    pub fn complete(&mut self, ticket: RefreshTicket) -> bool {
        if self.active != Some(ticket) {
            return false;
        }
        self.active = None;
        true
    }

    /// Call after an accepted completion, then `request` to schedule that rerun.
    pub fn take_queued(&mut self) -> bool {
        if self.active.is_some() {
            return false;
        }
        std::mem::take(&mut self.queued)
    }

    /// Invalidates outstanding work without pretending to forcibly stop its I/O.
    pub fn invalidate(&mut self) {
        self.active = None;
        self.queued = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flood_coalesces_to_one_followup() {
        let mut gate = RefreshGate::default();
        let first = gate.request().unwrap();
        for _ in 0..10_000 {
            assert!(gate.request().is_none());
        }
        assert!(!gate.take_queued());
        assert!(gate.complete(first));
        assert!(gate.take_queued());
        assert!(!gate.take_queued());
        let next = gate.request().unwrap();
        assert_ne!(first, next);
        assert!(gate.complete(next));
        assert!(!gate.take_queued());
    }

    #[test]
    fn stale_completion_cannot_publish_or_clear_new_work() {
        let mut gate = RefreshGate::default();
        let old = gate.request().unwrap();
        gate.invalidate();
        let new = gate.request().unwrap();
        assert!(!gate.complete(old));
        assert!(gate.request().is_none());
        assert!(gate.complete(new));
        assert!(gate.take_queued());
    }

    #[test]
    fn a_completion_is_accepted_only_once() {
        let mut gate = RefreshGate::default();
        let ticket = gate.request().unwrap();
        assert!(gate.complete(ticket));
        assert!(!gate.complete(ticket));
    }
}
