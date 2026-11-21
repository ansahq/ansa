//! Validation and outcome reporting for application launch and switch.
//!
//! A launch re-checks the entry and its resolved shortcut target immediately
//! before handing a single path to the shell. A missing or moved target returns
//! [`LaunchOutcome::NoTarget`] with a refresh hint and never reaches
//! `ShellExecuteW`. A resolved executable is first offered to the switch path;
//! only when no eligible window exists is a new process launched.

use crate::packaged;
use crate::shell_link;
use crate::switch::{self, SwitchResult};
use crate::types::LaunchOutcome;
use crate::win32;
use std::fs;
use std::path::{Path, PathBuf};

const PACKAGED_PREFIX: &str = "shell:AppsFolder\\";

#[derive(Clone, Debug, PartialEq, Eq)]
enum LaunchPlan {
    Packaged(String),
    File {
        entry: PathBuf,
        target: Option<PathBuf>,
        switchable: bool,
    },
}

/// Validates and opens an application entry without attempting to switch.
pub(crate) fn open_application(path: &str) -> Result<(), String> {
    execute(resolve(path)?)
}

/// Validates an entry, then switches to a running executable or launches it.
pub(crate) fn launch_or_switch(path: &str) -> LaunchOutcome {
    let plan = match resolve(path) {
        Ok(plan) => plan,
        Err(reason) => return LaunchOutcome::NoTarget { reason },
    };

    if let LaunchPlan::File {
        target: Some(target),
        switchable: true,
        ..
    } = &plan
    {
        if let Some(result) = switch::attempt_switch(target) {
            return match result {
                SwitchResult::Verified(window) => LaunchOutcome::Switched { window },
                SwitchResult::Denied(reason) => LaunchOutcome::Denied { reason },
                SwitchResult::Unverified(reason) => LaunchOutcome::Unverified { reason },
            };
        }
    }

    match execute(plan) {
        Ok(()) => LaunchOutcome::Launched,
        Err(reason) => {
            // Validation failures already returned NoTarget above. Reaching
            // here means the shell rejected an existing entry, which is a
            // launch failure rather than a missing target.
            if reason.to_ascii_lowercase().contains("access was denied") {
                LaunchOutcome::Denied { reason }
            } else {
                LaunchOutcome::Failed { reason }
            }
        }
    }
}

fn execute(plan: LaunchPlan) -> Result<(), String> {
    match plan {
        LaunchPlan::Packaged(aumid) => win32::open_packaged(&aumid),
        LaunchPlan::File { entry, .. } => {
            let entry = entry
                .to_str()
                .ok_or_else(|| "application entry path is not valid Unicode".to_owned())?;
            win32::open_application(entry)
        }
    }
}

fn resolve(path: &str) -> Result<LaunchPlan, String> {
    if path.is_empty() {
        return Err(
            "application path must not be empty; refresh the application catalog".to_owned(),
        );
    }
    if path.contains('\0') {
        return Err("application path must not contain NUL".to_owned());
    }

    if let Some(aumid) = packaged_aumid_from_path(path) {
        if !packaged::is_valid_aumid(aumid) {
            return Err(
                "packaged application identity is not a valid AUMID; refresh the application catalog"
                    .to_owned(),
            );
        }
        return Ok(LaunchPlan::Packaged(aumid.to_owned()));
    }

    let supplied = Path::new(path);
    let extension = supplied
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("lnk" | "exe") => {}
        _ => {
            return Err(
                "only .lnk and .exe application entries can be launched; refresh the application catalog"
                    .to_owned(),
            )
        }
    }

    let absolute = if supplied.is_absolute() {
        supplied.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot resolve the current directory: {error}"))?
            .join(supplied)
    };

    let metadata = fs::metadata(&absolute).map_err(|error| {
        format!(
            "application entry '{}' is not accessible ({error}); refresh the application catalog",
            absolute.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "application entry '{}' is not a regular file; refresh the application catalog",
            absolute.display()
        ));
    }

    if extension.as_deref() == Some("lnk") {
        let resolved = shell_link::resolve_shell_link_file(&absolute).map_err(|error| {
            format!(
                "shortcut '{}' could not be resolved ({error}); refresh the application catalog after reinstalling the application",
                absolute.display()
            )
        })?;

        if let Some(target) = resolved.target_path {
            if !target.is_file() {
                return Err(format!(
                    "shortcut target '{}' no longer exists; refresh the application catalog after reinstalling or moving the application",
                    target.display()
                ));
            }
            let switchable = target
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
            return Ok(LaunchPlan::File {
                entry: absolute,
                target: Some(target),
                switchable,
            });
        }

        if !resolved.attempted.is_empty() {
            return Err(format!(
                "shortcut target could not be resolved; tried {}; refresh the application catalog after reinstalling or moving the application",
                describe_attempts(&resolved.attempted)
            ));
        }

        // Shortcuts that only store a shell ID list have no local target to
        // validate; the shell resolves them when the `.lnk` itself is opened.
        return Ok(LaunchPlan::File {
            entry: absolute,
            target: None,
            switchable: false,
        });
    }

    Ok(LaunchPlan::File {
        target: Some(absolute.clone()),
        entry: absolute,
        switchable: true,
    })
}

fn describe_attempts(attempts: &[PathBuf]) -> String {
    attempts
        .iter()
        .map(|path| format!("'{}'", path.display()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn packaged_aumid_from_path(path: &str) -> Option<&str> {
    let prefix = path.get(..PACKAGED_PREFIX.len())?;
    if !prefix.eq_ignore_ascii_case(PACKAGED_PREFIX) {
        return None;
    }
    Some(&path[PACKAGED_PREFIX.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempTree {
        path: PathBuf,
    }

    impl TempTree {
        fn new(label: &str) -> Self {
            let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "ansa-platform-launch-{}-{label}-{sequence}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp tree");
            Self { path }
        }

        fn file_bytes(&self, relative: &str, bytes: &[u8]) -> PathBuf {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&path, bytes).expect("write fixture");
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn labels_and_success_are_conservative() {
        assert!(LaunchOutcome::Launched.is_success());
        assert!(LaunchOutcome::Switched { window: 7 }.is_success());
        assert!(!LaunchOutcome::Unverified {
            reason: "not observed".to_owned()
        }
        .is_success());
        assert!(!LaunchOutcome::Denied {
            reason: "denied".to_owned()
        }
        .is_success());
        assert!(!LaunchOutcome::NoTarget {
            reason: "missing".to_owned()
        }
        .is_success());
        assert!(!LaunchOutcome::Failed {
            reason: "shell failure".to_owned()
        }
        .is_success());
        assert_eq!(
            LaunchOutcome::Failed {
                reason: "shell failure".to_owned()
            }
            .label(),
            "failed"
        );
        assert_eq!(LaunchOutcome::Launched.label(), "launched");
        assert_eq!(LaunchOutcome::Switched { window: 7 }.label(), "switched");
        assert_eq!(
            LaunchOutcome::Unverified {
                reason: String::new()
            }
            .label(),
            "unverified"
        );
    }

    #[test]
    fn rejects_missing_and_unsupported_entries_before_launching() {
        let tree = TempTree::new("invalid");
        let missing = tree.path.join("Missing.lnk");
        let reason = resolve(missing.to_str().unwrap()).unwrap_err();
        assert!(reason.contains("refresh"));

        let text = tree.file_bytes("Readme.txt", b"");
        let reason = resolve(text.to_str().unwrap()).unwrap_err();
        assert!(reason.contains("only .lnk and .exe"));

        assert!(resolve("").is_err());
        assert!(resolve("C:\\bad\0.lnk").is_err());
    }

    #[test]
    fn a_shortcut_with_a_missing_target_is_not_launched() {
        let tree = TempTree::new("missing-target");
        let target = tree.path.join("Gone.exe");
        let link = tree.file_bytes(
            "Gone.lnk",
            &shell_link::minimal_link_fixture(&target.to_string_lossy()),
        );

        let reason = resolve(link.to_str().unwrap()).unwrap_err();
        assert!(reason.contains("no longer exists"), "{reason}");
        assert!(reason.contains("refresh"), "{reason}");

        let outcome = launch_or_switch(link.to_str().unwrap());
        assert!(matches!(outcome, LaunchOutcome::NoTarget { .. }));

        // `open_application` validates before it reaches ShellExecuteW.
        let error = open_application(link.to_str().unwrap()).unwrap_err();
        assert!(error.contains("no longer exists"), "{error}");
    }

    #[test]
    fn a_shortcut_with_an_existing_target_is_switchable() {
        let tree = TempTree::new("valid-target");
        let target = tree.file_bytes("Example.exe", b"");
        let link = tree.file_bytes(
            "Example.lnk",
            &shell_link::minimal_link_fixture(&target.to_string_lossy()),
        );

        let plan = resolve(link.to_str().unwrap()).expect("valid plan");
        assert_eq!(
            plan,
            LaunchPlan::File {
                entry: link,
                target: Some(target),
                switchable: true,
            }
        );
    }

    #[test]
    fn a_relative_shortcut_resolves_before_launch_validation() {
        let tree = TempTree::new("relative-launch");
        let target = tree.file_bytes("Riot Games/Riot Client/RiotClientServices.exe", b"");
        let link = tree.file_bytes(
            "Riot Games/Riot Client.lnk",
            &shell_link::relative_link_fixture(
                r"..\Riot Games\Riot Client\RiotClientServices.exe",
                r"C:/Riot Games/Riot Client",
            ),
        );

        let plan = resolve(link.to_str().unwrap()).expect("relative target resolves");
        assert_eq!(
            plan,
            LaunchPlan::File {
                entry: link,
                target: Some(target),
                switchable: true,
            }
        );
    }

    #[test]
    fn a_shortcut_without_link_info_is_still_openable() {
        let tree = TempTree::new("idlist-only");
        // Header-only shortcut: valid CLSID and header size, no LinkInfo.
        let mut bytes = shell_link::minimal_link_fixture(r"C:\ignored.exe");
        bytes.truncate(0x4C);
        bytes[20..24].copy_from_slice(&0_u32.to_le_bytes());
        let link = tree.file_bytes("Store.lnk", &bytes);

        let plan = resolve(link.to_str().unwrap()).expect("valid plan");
        assert!(matches!(
            plan,
            LaunchPlan::File {
                target: None,
                switchable: false,
                ..
            }
        ));
    }

    #[test]
    fn packaged_paths_require_a_valid_aumid() {
        assert_eq!(
            packaged_aumid_from_path(r"shell:AppsFolder\Vendor.App_pkg!App"),
            Some("Vendor.App_pkg!App")
        );
        assert_eq!(
            packaged_aumid_from_path(r"SHELL:APPSFOLDER\Vendor.App_pkg!App"),
            Some("Vendor.App_pkg!App")
        );
        assert_eq!(packaged_aumid_from_path(r"C:\App\app.exe"), None);

        let reason = resolve(r"shell:AppsFolder\C:\Windows\notepad.exe").unwrap_err();
        assert!(reason.contains("AUMID"));
        assert!(reason.contains("refresh"));

        assert!(matches!(
            resolve(r"shell:AppsFolder\Vendor.App_pkg!App"),
            Ok(LaunchPlan::Packaged(_))
        ));
    }

    #[test]
    fn a_direct_executable_is_prepared_for_switch_then_launch() {
        let tree = TempTree::new("direct-exe");
        let exe = tree.file_bytes("Example.exe", b"");
        assert_eq!(
            resolve(exe.to_str().unwrap()),
            Ok(LaunchPlan::File {
                entry: exe.clone(),
                target: Some(exe),
                switchable: true,
            })
        );
    }
}
