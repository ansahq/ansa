use crate::host_lifecycle::HostBackend;
use crate::{imp, CapabilityStatus};
use ansa_quicklinks::{
    is_safe_default_app_extension, resolve_target, OpenPolicy, OpenRequest, Opener,
    OpenerAvailability, OpenerProbe, TargetKind, MAX_RESOLVED_TARGET_BYTES,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Current native routing capabilities. Availability is a probe, not a
/// guarantee that the next particular URL or document will have a handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenerCapabilities {
    pub backend: HostBackend,
    pub browser: CapabilityStatus,
    pub file_manager: CapabilityStatus,
    pub default_application: CapabilityStatus,
}

/// Use this probe with the quicklink preview gate instead of assuming every
/// opener is available. Degraded, usable routes still require execution-time
/// validation and may produce a per-target error.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeOpenerProbe;

impl OpenerProbe for NativeOpenerProbe {
    fn probe(&self, opener: Opener) -> OpenerAvailability {
        let capabilities = imp::opener_capabilities();
        let status = match opener {
            Opener::Browser => capabilities.browser,
            Opener::FileManager => capabilities.file_manager,
            Opener::DefaultApp => capabilities.default_application,
        };
        match status {
            CapabilityStatus::Available | CapabilityStatus::Degraded { .. } => OpenerAvailability::Available,
            CapabilityStatus::PermissionRequired | CapabilityStatus::TemporarilyUnavailable | CapabilityStatus::Unsupported => {
                OpenerAvailability::Unavailable {
                    reason: format!("{} is {} on {}. Configure a desktop handler or copy the target and open it manually.",
                        opener.label(), status.label(), capabilities.backend.label()),
                }
            }
        }
    }
}

pub(super) fn execute(request: &OpenRequest, policy: OpenPolicy) -> Result<(), String> {
    match classify(request, policy)? {
        TargetKind::WebUrl => imp::open_url(&request.target),
        TargetKind::LocalPath => {
            let target = LocalTarget::capture(&request.target, request.opener, policy)?;
            match request.opener {
                Opener::FileManager => imp::reveal_in_file_manager(target.handoff_str()?),
                Opener::DefaultApp => imp::open_document(&target),
                Opener::Browser => Err("Browser does not open local paths".to_owned()),
            }
        }
    }
}

fn classify(request: &OpenRequest, policy: OpenPolicy) -> Result<TargetKind, String> {
    if request.target.len() > MAX_RESOLVED_TARGET_BYTES {
        return Err("Open target exceeds the quicklink size limit".to_owned());
    }
    resolve_target(&request.target, request.opener, policy).map_err(|error| error.to_string())
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(super) fn web_url(url: &str) -> Result<(), String> {
    if url.len() > MAX_RESOLVED_TARGET_BYTES {
        return Err("URL exceeds the quicklink size limit".to_owned());
    }
    resolve_target(url, Opener::Browser, OpenPolicy::web_only())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn local_path(target: &str, opener: Opener, policy: OpenPolicy) -> Result<String, String> {
    LocalTarget::capture(target, opener, policy)?
        .handoff_str()
        .map(str::to_owned)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObjectKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// A metadata snapshot of the exact filesystem object a policy decision refers
/// to. Unix includes the inode identity and change time so an in-place rewrite
/// is visible even when length and mtime are preserved.
#[derive(Clone, Debug)]
struct ObjectStamp {
    len: u64,
    modified: Option<SystemTime>,
    kind: ObjectKind,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
    #[cfg(unix)]
    mode: u32,
}

impl PartialEq for ObjectStamp {
    fn eq(&self, other: &Self) -> bool {
        let same =
            self.len == other.len && self.modified == other.modified && self.kind == other.kind;
        #[cfg(unix)]
        let same = same && self.identity == other.identity && self.mode == other.mode;
        same
    }
}

impl Eq for ObjectStamp {}

impl ObjectStamp {
    fn new(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        let file_type = metadata.file_type();
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            kind: if file_type.is_symlink() {
                ObjectKind::Symlink
            } else if file_type.is_file() {
                ObjectKind::File
            } else if file_type.is_dir() {
                ObjectKind::Directory
            } else {
                ObjectKind::Other
            },
            #[cfg(unix)]
            identity: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
            #[cfg(unix)]
            mode: metadata.permissions().mode(),
        }
    }

    #[cfg(target_os = "linux")]
    fn is_directory(&self) -> bool {
        self.kind == ObjectKind::Directory
    }
}

/// A policy-checked local target whose exact object can be revalidated around
/// native helper calls. `handoff` is the path actually sent to the opener; for
/// reveal it is the originally selected pathname, while document opening uses
/// the resolved canonical object. `input` remains the originally selected
/// pathname used to revalidate a symlink or directory entry.
#[derive(Clone, Debug)]
pub(super) struct LocalTarget {
    input: PathBuf,
    handoff: PathBuf,
    canonical_stamp: ObjectStamp,
    input_stamp: ObjectStamp,
    link_target: Option<PathBuf>,
    opener: Opener,
    policy: OpenPolicy,
}

impl PartialEq for LocalTarget {
    fn eq(&self, other: &Self) -> bool {
        self.input == other.input
            && self.handoff == other.handoff
            && self.canonical_stamp == other.canonical_stamp
            && self.input_stamp == other.input_stamp
            && self.link_target == other.link_target
            && self.opener == other.opener
            && self.policy == other.policy
    }
}

impl Eq for LocalTarget {}

impl LocalTarget {
    pub(super) fn capture(
        target: &str,
        opener: Opener,
        policy: OpenPolicy,
    ) -> Result<Self, String> {
        let input = Path::new(target);
        if !input.is_absolute() {
            return Err("Target must be absolute on this operating system".to_owned());
        }
        let canonical = fs::canonicalize(input)
            .map_err(|error| format!("Local target could not be resolved: {error}"))?;
        let canonical_metadata = fs::symlink_metadata(&canonical)
            .map_err(|error| format!("Local target is no longer accessible: {error}"))?;
        if canonical_metadata.file_type().is_symlink() {
            // std::fs::canonicalize should have resolved every link. A final
            // symlink here means the path was replaced during resolution.
            return Err("Local target changed while it was being resolved; retry".to_owned());
        }
        let metadata = fs::metadata(&canonical)
            .map_err(|error| format!("Local target is no longer accessible: {error}"))?;
        let canonical_str = checked_canonical(&canonical)?;
        // Never broaden the path-kind policy through a symlink. On Windows std
        // canonicalize adds a synthetic verbatim prefix; checked_canonical
        // removes it for policy evaluation rather than treating it as user
        // input. Re-check the resolved canonical path against the caller's
        // policy without broadening it: FileManager is used only to validate
        // path-kind opt-ins (local/network/device); safe_document below applies
        // a stricter document allowlist even when the policy opted into
        // executables or unknown types.
        resolve_target(canonical_str, Opener::FileManager, policy)
            .map_err(|error| error.to_string())?;
        if !metadata.is_file() && !metadata.is_dir() {
            return Err("Only regular files and directories may be sent to an opener".to_owned());
        }
        if opener == Opener::DefaultApp {
            safe_document(&canonical, &metadata)?;
        }
        let input_metadata = fs::symlink_metadata(input)
            .map_err(|error| format!("Local target could not be inspected: {error}"))?;
        let link_target = if input_metadata.file_type().is_symlink() {
            Some(
                fs::read_link(input)
                    .map_err(|error| format!("Local target link could not be read: {error}"))?,
            )
        } else {
            None
        };
        // Reveal, unlike open, is harmless on an alias. Use the originally
        // selected path so the file manager presents the selected directory
        // entry. Document opening hands off the resolved object.
        let handoff = if opener == Opener::FileManager {
            input.to_path_buf()
        } else {
            canonical.clone()
        };
        Ok(Self {
            input: input.to_path_buf(),
            handoff,
            canonical_stamp: ObjectStamp::new(&canonical_metadata),
            input_stamp: ObjectStamp::new(&input_metadata),
            link_target,
            opener,
            policy,
        })
    }

    /// Re-resolves and re-applies policy to the exact handoff path, then
    /// requires every captured identity field to match. Callers use this
    /// immediately before a native handoff that ran after the first check.
    #[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
    pub(super) fn revalidate(&self) -> Result<(), String> {
        // Revalidate the originally selected pathname, not only the canonical
        // handoff string. A retargeted symlink must fail even though the old
        // canonical target still exists.
        let input = self.input.to_str().ok_or("Resolved path is not UTF-8")?;
        let current = Self::capture(input, self.opener, self.policy)?;
        if current == *self {
            Ok(())
        } else {
            Err(
                "Local target changed after policy validation. Refresh the target and try again"
                    .to_owned(),
            )
        }
    }

    pub(super) fn handoff(&self) -> &Path {
        &self.handoff
    }

    pub(super) fn handoff_str(&self) -> Result<&str, String> {
        self.handoff()
            .to_str()
            .ok_or_else(|| "Resolved path is not UTF-8".to_owned())
    }

    #[cfg(target_os = "linux")]
    pub(super) fn is_directory(&self) -> bool {
        self.canonical_stamp.is_directory()
    }
}

#[cfg(windows)]
fn checked_canonical(path: &Path) -> Result<&str, String> {
    let path = path.to_str().ok_or("Resolved path is not UTF-8")?;
    let Some(rest) = path.strip_prefix(r"\\?\") else {
        // std::fs::canonicalize normally returns a verbatim path. If it did
        // not, any remaining UNC/device prefix is not sent to Explorer.
        if path.starts_with(r"\\") || path.starts_with("//") {
            return Err(
                "Resolved network or device paths are not supported by this opener".to_owned(),
            );
        }
        return Ok(path);
    };
    if rest.get(..4).is_some_and(|prefix| {
        prefix.eq_ignore_ascii_case("UNC\\") || prefix.eq_ignore_ascii_case("UNC/")
    }) || rest.starts_with(r"\\")
        || rest.starts_with("//")
    {
        return Err("Resolved network paths are not supported by this opener".to_owned());
    }
    Ok(rest)
}

#[cfg(not(windows))]
fn checked_canonical(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "Resolved path is not UTF-8".to_owned())
}

fn safe_document(path: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    if metadata.is_dir() {
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                [
                    "app",
                    "bundle",
                    "plugin",
                    "pkg",
                    "mpkg",
                    "prefpane",
                    "appex",
                    "framework",
                    "workflow",
                ]
                .iter()
                .any(|kind| ext.eq_ignore_ascii_case(kind))
            })
        {
            return Err(
                "Application packages cannot be opened as documents; reveal the path instead"
                    .to_owned(),
            );
        }
        return Ok(());
    }
    if !path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(is_safe_default_app_extension)
    {
        return Err(
            "Local file is not an allowlisted document type; reveal the path instead".to_owned(),
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 != 0 {
            return Err(
                "A file marked executable cannot be opened as a document; reveal the path instead"
                    .to_owned(),
            );
        }
    }
    Ok(())
}

/// Validate raw reveal calls, which are also used for catalog application
/// paths. They are not allowed to fall through to a default application.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn reveal_path(target: &str) -> Result<PathBuf, String> {
    let policy = OpenPolicy::with_local_paths();
    let request = OpenRequest {
        target: target.to_owned(),
        opener: Opener::FileManager,
    };
    classify(&request, policy)?;
    local_path(target, request.opener, policy).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_constructible_requests_must_pass_policy_again() {
        for target in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "ftp://example.org",
            "https://",
            "https://example.org/%00",
            "https://user:secret@example.org",
            "https://example.org/a\nb",
        ] {
            assert!(web_url(target).is_err(), "{target:?}");
        }
        assert!(web_url("hTTpS://example.org/path?q=a%20b").is_ok());
        let request = OpenRequest {
            target: "/tmp/file.txt".to_owned(),
            opener: Opener::DefaultApp,
        };
        assert!(classify(&request, OpenPolicy::web_only()).is_err());
        assert!(classify(&request, OpenPolicy::with_local_paths()).is_ok());
        assert!(classify(
            &OpenRequest {
                opener: Opener::FileManager,
                target: "//host/path".to_owned()
            },
            OpenPolicy::with_local_paths()
        )
        .is_err());
        let invalid = OpenRequest {
            opener: Opener::Browser,
            target: "javascript:alert(1)".to_owned(),
        };
        assert!(execute(&invalid, OpenPolicy::with_local_paths()).is_err());
    }

    #[test]
    fn local_default_uses_actual_file_type_and_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = OpenPolicy::with_local_paths();
        let doc = tmp.path().join("document.TXT");
        let file = tmp.path().join("extensionless");
        let dir = tmp.path().join("directory");
        fs::write(&doc, b"safe").unwrap();
        fs::write(&file, b"unsafe").unwrap();
        fs::create_dir(&dir).unwrap();
        assert!(local_path(doc.to_str().unwrap(), Opener::DefaultApp, policy).is_ok());
        assert!(local_path(file.to_str().unwrap(), Opener::FileManager, policy).is_ok());
        assert!(local_path(
            file.to_str().unwrap(),
            Opener::DefaultApp,
            policy.with_executables()
        )
        .is_err());
        assert!(local_path(dir.to_str().unwrap(), Opener::DefaultApp, policy).is_ok());
    }

    #[test]
    fn captured_document_rejects_in_place_or_missing_replacement_before_handoff() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = OpenPolicy::with_local_paths();
        let doc = tmp.path().join("document.txt");
        fs::write(&doc, b"original").unwrap();
        let raw = doc.to_str().unwrap();
        let captured = LocalTarget::capture(raw, Opener::DefaultApp, policy).unwrap();
        assert!(captured.revalidate().is_ok());

        // Same pathname, different object contents.
        fs::write(&doc, b"replacement content").unwrap();
        assert!(captured.revalidate().is_err());

        // The pathname no longer resolves to the captured object at all.
        fs::remove_file(&doc).unwrap();
        assert!(captured.revalidate().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn captured_document_rejects_symlink_retarget_or_disallowed_link() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let policy = OpenPolicy::with_local_paths();
        let approved = tmp.path().join("approved.txt");
        let other = tmp.path().join("other.txt");
        let unsafe_file = tmp.path().join("no-extension");
        fs::write(&approved, b"approved").unwrap();
        fs::write(&other, b"other").unwrap();
        fs::write(&unsafe_file, b"unsafe").unwrap();
        let captured =
            LocalTarget::capture(approved.to_str().unwrap(), Opener::DefaultApp, policy).unwrap();
        assert!(captured.revalidate().is_ok());

        // Replacing the approved pathname with a symlink to another allowlisted
        // file still changes the exact object.
        fs::remove_file(&approved).unwrap();
        symlink(&other, &approved).unwrap();
        assert!(captured.revalidate().is_err());

        // A symlink to a file the document policy rejects also fails during
        // revalidation, before any native handoff.
        fs::remove_file(&approved).unwrap();
        symlink(&unsafe_file, &approved).unwrap();
        assert!(captured.revalidate().is_err());
        assert!(
            LocalTarget::capture(approved.to_str().unwrap(), Opener::DefaultApp, policy).is_err()
        );

        // Reveal captures the selected pathname and notices a retarget even
        // though the canonical target remains an allowed local path.
        let reveal =
            LocalTarget::capture(approved.to_str().unwrap(), Opener::FileManager, policy).unwrap();
        assert!(reveal.revalidate().is_ok());
        fs::remove_file(&approved).unwrap();
        symlink(&other, &approved).unwrap();
        assert!(reveal.revalidate().is_err());
    }

    #[test]
    fn execute_rechecks_policy_and_file_kind_before_native_handoff() {
        let tmp = tempfile::tempdir().unwrap();
        let extensionless = tmp.path().join("extensionless");
        fs::write(&extensionless, b"not a document type").unwrap();
        let target = extensionless.to_str().unwrap().to_owned();

        // A caller can construct OpenRequest directly, so execute must repeat
        // the policy decision and the host-side kind check.
        assert!(execute(
            &OpenRequest {
                target: target.clone(),
                opener: Opener::FileManager,
            },
            OpenPolicy::web_only(),
        )
        .is_err());
        assert!(execute(
            &OpenRequest {
                target: target.clone(),
                opener: Opener::DefaultApp,
            },
            OpenPolicy::with_local_paths(),
        )
        .is_err());
        assert!(execute(
            &OpenRequest {
                target,
                opener: Opener::Browser,
            },
            OpenPolicy::with_local_paths(),
        )
        .is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_canonical_rejects_unc_and_strips_verbatim_prefix() {
        assert!(checked_canonical(Path::new(r"\\?\UNC\server\share\file.txt")).is_err());
        assert!(checked_canonical(Path::new(r"\\?\unc\server\share\file.txt")).is_err());
        assert!(checked_canonical(Path::new(r"\\?\UNC/server/share/file.txt")).is_err());
        assert!(checked_canonical(Path::new(r"\\server\share\file.txt")).is_err());
        assert!(checked_canonical(Path::new("//server/share/file.txt")).is_err());
        assert_eq!(
            checked_canonical(Path::new(r"\\?\C:\Temp\file.txt")),
            Ok(r"C:\Temp\file.txt")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cannot_bypass_resolved_extension_or_package_policy() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let executable = tmp.path().join("program.sh");
        let shortcut = tmp.path().join("document.txt");
        let bundle = tmp.path().join("Example.app");
        let directory = tmp.path().join("folder");
        fs::write(&executable, b"script").unwrap();
        fs::create_dir(&bundle).unwrap();
        symlink(executable, &shortcut).unwrap();
        symlink(bundle, &directory).unwrap();
        let policy = OpenPolicy::with_local_paths();
        for path in [shortcut, directory] {
            let raw = path.to_str().unwrap();
            assert_eq!(
                local_path(raw, Opener::FileManager, policy),
                Ok(raw.to_owned())
            );
            assert!(local_path(raw, Opener::DefaultApp, policy).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn executable_file_can_only_be_revealed_never_opened_as_a_document() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("script.txt");
        fs::write(&script, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let policy = OpenPolicy::with_local_paths().with_executables();
        let raw = script.to_str().unwrap();
        assert_eq!(
            local_path(raw, Opener::FileManager, policy),
            Ok(raw.to_owned())
        );
        assert!(local_path(raw, Opener::DefaultApp, policy).is_err());
    }
}
