//! The native platform adapter publishes a path-keyed discovery snapshot
//! before the GPUI catalog can be redrawn. Do not let a launch selected from
//! the previous view consult that newer snapshot.

use ansa_platform_api::LaunchOutcome;
use std::sync::Mutex;

#[cfg(not(windows))]
pub(crate) static NATIVE_CATALOG: CatalogDispatch = CatalogDispatch::new();

pub(crate) struct CatalogDispatch {
    current: Mutex<u64>,
}

impl CatalogDispatch {
    const fn new() -> Self {
        Self {
            current: Mutex::new(0),
        }
    }

    pub(crate) fn refresh<T>(&self, discover: impl FnOnce() -> T) -> (u64, T) {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *current = current
            .checked_add(1)
            .expect("catalog generation exhausted");
        (*current, discover())
    }

    pub(crate) fn launch(
        &self,
        selected_generation: Option<u64>,
        launch: impl FnOnce() -> LaunchOutcome,
    ) -> LaunchOutcome {
        // Hold the generation across the native launch's own revalidation and
        // handoff. Never make the foreground UI wait for a background refresh.
        let Ok(current) = self.current.try_lock() else {
            return Self::changed();
        };
        if selected_generation != Some(*current) || *current == 0 {
            return Self::changed();
        }
        launch()
    }

    fn changed() -> LaunchOutcome {
        LaunchOutcome::NoTarget {
            reason: "The application catalog is busy or has changed. Review the current result and try again.".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_does_not_run_against_a_replaced_catalog_snapshot() {
        let dispatch = CatalogDispatch::new();
        let (old, ()) = dispatch.refresh(|| ());
        let (new, ()) = dispatch.refresh(|| ());
        assert_ne!(old, new);
        assert!(matches!(
            dispatch.launch(Some(old), || panic!("stale selection cannot launch")),
            LaunchOutcome::NoTarget { .. }
        ));
        assert!(
            dispatch
                .launch(Some(new), || LaunchOutcome::Launched)
                .is_launched()
        );
    }

    #[test]
    fn unrendered_catalog_and_in_progress_refresh_cannot_launch() {
        let dispatch = CatalogDispatch::new();
        let (old, ()) = dispatch.refresh(|| ());
        assert!(matches!(
            dispatch.launch(None, || panic!("unrendered selection cannot launch")),
            LaunchOutcome::NoTarget { .. }
        ));
        dispatch.refresh(|| {
            assert!(matches!(
                dispatch.launch(Some(old), || panic!("refresh in progress")),
                LaunchOutcome::NoTarget { .. }
            ));
        });
    }
}
