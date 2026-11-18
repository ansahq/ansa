//! Main-thread dispatch contract for native effects.
//!
//! Native windows are thread-affine. This module models that with two
//! mechanisms:
//!
//! * [`MainThreadOnly<T>`] is a wrapper that is deliberately **not** `Send` or
//!   `Sync` (it contains an `Rc` marker). A native handle wrapped in it cannot
//!   be moved into a background task at compile time.
//! * [`MainThreadDispatcher`] issues a checked [`MainThreadToken`] and refuses to
//!   dispatch from a thread other than its owner, returning a typed
//!   [`DispatchError`] rather than performing the call.
//!
//! A token is intentionally `Send` so the runtime check itself can be tested
//! and so a queue can carry it through a documented main-thread hand-off; the
//! token never authorizes execution on a different thread.

use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, ThreadId};

use crate::geometry::Rect;
use crate::invocation::InvocationId;

static NEXT_DISPATCHER_GENERATION: AtomicU64 = AtomicU64::new(1);

/// A native effect that is only safe on the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MainThreadCall {
    /// Place and show the launcher for one invocation.
    Show {
        /// The invocation requesting the show.
        invocation: InvocationId,
        /// Physical-pixel placement.
        rect: Rect,
    },
    /// Hide the launcher for one invocation.
    Hide {
        /// The invocation requesting the hide.
        invocation: InvocationId,
    },
    /// Request foreground activation for one invocation.
    Focus {
        /// The invocation requesting activation.
        invocation: InvocationId,
    },
    /// Release resources in the controller's ordered shutdown plan.
    Shutdown,
}

/// Typed refusal from the main-thread dispatch contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchError {
    /// The call was attempted from a thread other than the dispatcher owner.
    WrongThread {
        /// The owner thread id.
        owner: ThreadId,
        /// The calling thread id.
        current: ThreadId,
    },
    /// The token was issued by a different dispatcher.
    ForeignToken {
        /// The token's owner thread id.
        owner: ThreadId,
    },
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongThread { owner, current } => write!(
                formatter,
                "native launcher effects must run on the owner UI thread {owner:?}, not {current:?}"
            ),
            Self::ForeignToken { owner } => write!(
                formatter,
                "the dispatch token belongs to a different main-thread dispatcher ({owner:?})"
            ),
        }
    }
}

impl std::error::Error for DispatchError {}

/// Proof that the caller was on a dispatcher's owner thread when it was issued.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MainThreadToken {
    owner: ThreadId,
    generation: u64,
}

impl MainThreadToken {
    /// The thread that issued the token.
    pub fn owner(&self) -> ThreadId {
        self.owner
    }
}

/// Records and vets main-thread native effects.
///
/// This is both the production contract and the test fake: calls accepted on
/// the owner thread are recorded and can be drained for assertions. A future
/// UI loop would drain them at the start of each frame and perform the native
/// effects there.
#[derive(Debug)]
pub struct MainThreadDispatcher {
    owner: ThreadId,
    generation: u64,
    calls: Vec<MainThreadCall>,
}

impl MainThreadDispatcher {
    /// Creates a dispatcher owned by the calling thread.
    pub fn new() -> Self {
        Self {
            owner: thread::current().id(),
            generation: NEXT_DISPATCHER_GENERATION.fetch_add(1, Ordering::Relaxed),
            calls: Vec::new(),
        }
    }

    /// Returns the owner thread id.
    pub fn owner(&self) -> ThreadId {
        self.owner
    }

    /// Issues a token, or refuses when called off the owner thread.
    pub fn token(&self) -> Result<MainThreadToken, DispatchError> {
        let current = thread::current().id();
        if current != self.owner {
            return Err(DispatchError::WrongThread {
                owner: self.owner,
                current,
            });
        }
        Ok(MainThreadToken {
            owner: self.owner,
            generation: self.generation,
        })
    }

    /// Vets and records a main-thread call.
    ///
    /// The token must have been issued by this dispatcher and the current
    /// thread must be the owner. Nothing is recorded when either check fails.
    pub fn dispatch(
        &mut self,
        token: &MainThreadToken,
        call: MainThreadCall,
    ) -> Result<(), DispatchError> {
        if token.owner != self.owner || token.generation != self.generation {
            return Err(DispatchError::ForeignToken { owner: token.owner });
        }
        let current = thread::current().id();
        if current != self.owner {
            return Err(DispatchError::WrongThread {
                owner: self.owner,
                current,
            });
        }
        self.calls.push(call);
        Ok(())
    }

    /// Number of calls accepted so far.
    pub fn recorded_len(&self) -> usize {
        self.calls.len()
    }

    /// Returns every accepted call and clears the log.
    pub fn drain(&mut self) -> Vec<MainThreadCall> {
        std::mem::take(&mut self.calls)
    }
}

impl Default for MainThreadDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// A value that must remain on the UI thread.
///
/// The private `Rc` marker makes this type `!Send` and `!Sync`. Construction is
/// checked against a [`MainThreadToken`], so a token moved to a worker thread
/// cannot be used to bind a value there either.
pub struct MainThreadOnly<T> {
    value: T,
    _not_send: PhantomData<Rc<()>>,
}

impl<T> MainThreadOnly<T> {
    /// Binds a value after checking that the current thread issued `token`.
    pub fn bind(token: &MainThreadToken, value: T) -> Result<Self, DispatchError> {
        let current = thread::current().id();
        if current != token.owner {
            return Err(DispatchError::WrongThread {
                owner: token.owner,
                current,
            });
        }
        Ok(Self {
            value,
            _not_send: PhantomData,
        })
    }

    /// Borrows the wrapped value.
    pub fn with<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        read(&self.value)
    }

    /// Mutably borrows the wrapped value.
    pub fn with_mut<R>(&mut self, write: impl FnOnce(&mut T) -> R) -> R {
        write(&mut self.value)
    }

    /// Unwraps the value. The caller remains on the thread that owns it.
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for MainThreadOnly<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("MainThreadOnly")
            .field(&self.value)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn owner_thread_records_calls() {
        let mut dispatcher = MainThreadDispatcher::new();
        let token = dispatcher.token().expect("owner thread token");
        let invocation = crate::InvocationIssuer::new().issue();

        dispatcher
            .dispatch(
                &token,
                MainThreadCall::Show {
                    invocation,
                    rect: Rect::new(1, 2, 3, 4),
                },
            )
            .expect("owner dispatch");
        dispatcher
            .dispatch(&token, MainThreadCall::Hide { invocation })
            .expect("owner dispatch");

        assert_eq!(dispatcher.recorded_len(), 2);
        assert_eq!(dispatcher.drain().len(), 2);
        assert_eq!(dispatcher.recorded_len(), 0);
    }

    #[test]
    fn a_different_thread_is_refused_without_recording() {
        let dispatcher = Arc::new(Mutex::new(MainThreadDispatcher::new()));
        let token = dispatcher.lock().unwrap().token().expect("owner token");
        let worker = Arc::clone(&dispatcher);

        let result = thread::spawn(move || {
            let mut dispatcher = worker.lock().unwrap();
            dispatcher.dispatch(&token, MainThreadCall::Shutdown)
        })
        .join()
        .expect("worker did not panic");

        assert!(matches!(result, Err(DispatchError::WrongThread { .. })));
        assert_eq!(dispatcher.lock().unwrap().recorded_len(), 0);
    }

    #[test]
    fn a_token_from_another_dispatcher_is_refused() {
        let first = MainThreadDispatcher::new();
        let mut second = MainThreadDispatcher::new();
        let foreign = first.token().expect("owner token");
        let result = second.dispatch(&foreign, MainThreadCall::Shutdown);
        assert!(matches!(result, Err(DispatchError::ForeignToken { .. })));
        assert_eq!(second.recorded_len(), 0);
    }

    #[test]
    fn main_thread_only_binding_is_checked_and_preserved() {
        let dispatcher = MainThreadDispatcher::new();
        let token = dispatcher.token().expect("owner token");
        let mut value = MainThreadOnly::bind(&token, 7isize).expect("owner bind");
        assert_eq!(value.with(|inner| *inner), 7);
        value.with_mut(|inner| *inner = 9);
        assert_eq!(value.into_inner(), 9);
    }

    #[test]
    fn main_thread_only_cannot_be_bound_on_a_worker_with_a_moved_token() {
        let dispatcher = MainThreadDispatcher::new();
        let token = dispatcher.token().expect("owner token");
        let result = thread::spawn(move || MainThreadOnly::bind(&token, 1isize).is_err())
            .join()
            .expect("worker did not panic");
        assert!(result, "a moved token must not authorize a worker bind");
    }
}
