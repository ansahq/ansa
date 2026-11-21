//! Integration tests for the Windows DPAPI vault.
//!
//! Windows tests exercise the real operating-system backend; non-Windows tests
//! assert the explicit unsupported result and that no files are created.

#[cfg(windows)]
use ansa_vault::{
    MAX_PLAINTEXT_BYTES, MAX_SEALED_FILE_BYTES, OpenPayload, PAYLOAD_VERSION, SealedPayload,
};
use ansa_vault::{Vault, VaultError};

use std::fs;
#[cfg(windows)]
use std::path::{Path, PathBuf};

#[cfg(windows)]
fn new_vault() -> (tempfile::TempDir, Vault) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let vault = Vault::open(directory.path()).expect("open vault");
    (directory, vault)
}

#[cfg(windows)]
fn all_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).expect("read directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

#[cfg(windows)]
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(windows)]
fn vault_files_contain(root: &Path, needle: &[u8]) -> bool {
    all_files(root)
        .iter()
        .any(|path| contains(&fs::read(path).expect("read vault file"), needle))
}

#[test]
#[cfg(windows)]
fn payload_round_trip_through_json() {
    let (_directory, vault) = new_vault();
    let payload = vault.seal(b"hello secret", "clipboard.text").unwrap();
    assert_eq!(payload.version, PAYLOAD_VERSION);
    assert_eq!(payload.context_tag, "clipboard.text");
    assert!(!payload.ciphertext_hex.is_empty());

    let encoded = serde_json::to_string(&payload).unwrap();
    let decoded: SealedPayload = serde_json::from_str(&encoded).unwrap();
    assert_eq!(
        vault.open(&decoded, "clipboard.text").unwrap().as_slice(),
        b"hello secret"
    );
    assert_eq!(
        vault
            .open_payload(&decoded, "clipboard.text")
            .unwrap()
            .as_slice(),
        b"hello secret"
    );

    let empty = vault.seal(b"", "clipboard.text").unwrap();
    assert!(vault.open(&empty, "clipboard.text").unwrap().is_empty());
}

#[test]
#[cfg(windows)]
fn wrong_context_and_tampering_fail_without_plaintext() {
    let (_directory, vault) = new_vault();
    let payload = vault.seal(b"bound secret", "clipboard.text").unwrap();

    assert!(matches!(
        vault.open(&payload, "note.body"),
        Err(VaultError::WrongContext)
    ));

    // Even if the visible tag is rewritten, DPAPI's entropy binding fails.
    let mut rewritten = payload.clone();
    rewritten.context_tag = "note.body".to_owned();
    assert!(matches!(
        vault.open(&rewritten, "note.body"),
        Err(VaultError::DecryptionFailed { .. })
    ));

    let mut tampered = payload.clone();
    let mut digits = tampered.ciphertext_hex.into_bytes();
    digits[0] = if digits[0] == b'0' { b'1' } else { b'0' };
    tampered.ciphertext_hex = String::from_utf8(digits).unwrap();
    assert!(matches!(
        vault.open(&tampered, "clipboard.text"),
        Err(VaultError::DecryptionFailed { .. })
    ));

    let mut truncated = payload.clone();
    truncated.ciphertext_hex.pop();
    assert!(matches!(
        vault.open(&truncated, "clipboard.text"),
        Err(VaultError::InvalidPayload { .. })
    ));
}

#[test]
#[cfg(windows)]
fn lock_blocks_every_secret_operation_until_unlocked() {
    let (_directory, vault) = new_vault();
    vault.put_secret("token", b"value").unwrap();
    vault.put_secret("other", b"value").unwrap();
    let payload = vault.seal(b"value", "clipboard.text").unwrap();

    vault.lock();
    assert!(vault.is_locked());
    assert!(matches!(
        vault.seal(b"x", "clipboard.text"),
        Err(VaultError::Locked)
    ));
    assert!(matches!(
        vault.open(&payload, "clipboard.text"),
        Err(VaultError::Locked)
    ));
    assert!(matches!(
        vault.put_secret("new", b"x"),
        Err(VaultError::Locked)
    ));
    assert!(matches!(vault.get_secret("token"), Err(VaultError::Locked)));
    assert!(matches!(
        vault.delete_secret("token"),
        Err(VaultError::Locked)
    ));
    assert!(matches!(vault.list_secrets(), Err(VaultError::Locked)));
    assert!(matches!(vault.rotate(), Err(VaultError::Locked)));

    vault.unlock();
    assert!(!vault.is_locked());
    assert_eq!(vault.get_secret("token").unwrap().as_slice(), b"value");
    assert_eq!(
        vault.open(&payload, "clipboard.text").unwrap().as_slice(),
        b"value"
    );
}

#[test]
#[cfg(windows)]
fn alias_traversal_and_windows_device_names_are_rejected() {
    let (directory, vault) = new_vault();
    let long = "a".repeat(129);
    let bad_aliases = [
        "",
        ".",
        "..",
        "../escape",
        "..\\escape",
        "a/b",
        "a\\b",
        "a b",
        "bad\0alias",
        "caf\u{e9}",
        "CON",
        "nul.txt",
        "COM1",
        "lpt9.log",
        "trailing.",
        long.as_str(),
    ];

    for alias in bad_aliases {
        assert!(
            matches!(
                vault.put_secret(alias, b"value"),
                Err(VaultError::InvalidAlias { .. })
            ),
            "alias {alias:?} should be rejected"
        );
        assert!(
            matches!(
                vault.get_secret(alias),
                Err(VaultError::InvalidAlias { .. })
            ),
            "alias {alias:?} should be rejected on read"
        );
        assert!(
            matches!(
                vault.delete_secret(alias),
                Err(VaultError::InvalidAlias { .. })
            ),
            "alias {alias:?} should be rejected on delete"
        );
    }

    assert!(vault.list_secrets().unwrap().is_empty());
    assert!(
        !directory
            .path()
            .parent()
            .unwrap()
            .join("escape.sealed")
            .exists()
    );
    assert!(!directory.path().join("escape.sealed").exists());
}

#[test]
#[cfg(windows)]
fn secrets_survive_reopen() {
    let directory = tempfile::tempdir().unwrap();
    {
        let vault = Vault::open(directory.path()).unwrap();
        vault.put_secret("persistent", b"survives").unwrap();
    }
    {
        let vault = Vault::open(directory.path()).unwrap();
        assert_eq!(
            vault.get_secret("persistent").unwrap().as_slice(),
            b"survives"
        );
    }
}

#[test]
#[cfg(windows)]
fn secrets_survive_process_independent_reopen() {
    let directory = tempfile::tempdir().unwrap();
    {
        let vault = Vault::open(directory.path()).unwrap();
        vault
            .put_secret("cross-process", b"child-value")
            .expect("write secret before spawning the child");
    }

    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_reads_secret_from_env"])
        .env("ANSA_VAULT_CHILD_DIR", directory.path())
        .env("ANSA_VAULT_CHILD_ALIAS", "cross-process")
        .env("ANSA_VAULT_CHILD_EXPECT", "child-value")
        .status()
        .expect("spawn child test process");
    assert!(status.success(), "child process could not read the vault");
}

/// Child half of [`secrets_survive_process_independent_reopen`]. When the
/// environment variables are absent this test is a no-op in the normal suite.
#[test]
#[cfg(windows)]
fn child_reads_secret_from_env() {
    let Some(directory) = std::env::var_os("ANSA_VAULT_CHILD_DIR") else {
        return;
    };
    let alias = std::env::var("ANSA_VAULT_CHILD_ALIAS").unwrap();
    let expected = std::env::var("ANSA_VAULT_CHILD_EXPECT").unwrap();
    let vault = Vault::open(directory).unwrap();
    assert_eq!(
        vault.get_secret(&alias).unwrap().as_slice(),
        expected.as_bytes()
    );
}

#[test]
#[cfg(windows)]
fn rotation_reseals_every_secret_without_losing_data() {
    let (directory, vault) = new_vault();
    vault.put_secret("alpha", b"alpha-value").unwrap();
    vault.put_secret("beta", b"beta-value").unwrap();
    let before_alpha = fs::read(directory.path().join("secrets").join("alpha.sealed")).unwrap();
    let before_beta = fs::read(directory.path().join("secrets").join("beta.sealed")).unwrap();

    let report = vault.rotate().unwrap();
    assert_eq!(report.secrets_resealed, 2);
    assert!(!report.os_key_rotation_performed);

    assert_eq!(
        vault.get_secret("alpha").unwrap().as_slice(),
        b"alpha-value"
    );
    assert_eq!(vault.get_secret("beta").unwrap().as_slice(), b"beta-value");
    let after_alpha = fs::read(directory.path().join("secrets").join("alpha.sealed")).unwrap();
    let after_beta = fs::read(directory.path().join("secrets").join("beta.sealed")).unwrap();
    assert_ne!(before_alpha, after_alpha);
    assert_ne!(before_beta, after_beta);
}

#[test]
#[cfg(windows)]
fn plaintext_sentinel_never_reaches_vault_files_or_debug_output() {
    const SENTINEL: &str = "ANSA-VAULT-PLAINTEXT-SENTINEL-7d2a1f";
    let (directory, vault) = new_vault();
    vault.put_secret("sentinel", SENTINEL.as_bytes()).unwrap();
    let payload = vault.seal(SENTINEL.as_bytes(), "clipboard.text").unwrap();

    assert!(!format!("{payload:?}").contains(SENTINEL));
    assert!(!format!("{vault:?}").contains(SENTINEL));
    assert!(!serde_json::to_string(&payload).unwrap().contains(SENTINEL));

    let error = vault.get_secret("missing").unwrap_err();
    assert!(!format!("{error}").contains(SENTINEL));
    assert!(!format!("{error:?}").contains(SENTINEL));

    assert!(
        !vault_files_contain(directory.path(), SENTINEL.as_bytes()),
        "plaintext sentinel was found in a vault file"
    );
}

#[test]
#[cfg(windows)]
fn duplicate_alias_replaces_atomically_and_case_insensitively() {
    let (directory, vault) = new_vault();
    vault.put_secret("alias", b"first-value").unwrap();
    vault.put_secret("alias", b"second-value").unwrap();

    assert_eq!(
        vault.list_secrets().unwrap(),
        vec!["alias".to_owned()],
        "replacement must not create a second file"
    );
    assert_eq!(
        vault.get_secret("alias").unwrap().as_slice(),
        b"second-value"
    );
    assert!(
        !vault_files_contain(directory.path(), b"first-value"),
        "replaced plaintext must not remain in the vault"
    );

    vault.put_secret("Alias", b"third-value").unwrap();
    assert_eq!(vault.list_secrets().unwrap(), vec!["alias".to_owned()]);
    assert_eq!(
        vault.get_secret("alias").unwrap().as_slice(),
        b"third-value"
    );
    assert_eq!(
        vault.get_secret("ALIAS").unwrap().as_slice(),
        b"third-value"
    );
}

#[test]
#[cfg(windows)]
fn delete_clears_ciphertext_and_is_idempotent() {
    let (directory, vault) = new_vault();
    vault.put_secret("remove-me", b"remove-value").unwrap();
    let path = directory.path().join("secrets").join("remove-me.sealed");
    assert!(path.exists());

    assert!(vault.delete_secret("remove-me").unwrap());
    assert!(!path.exists());
    assert!(!vault.delete_secret("remove-me").unwrap());
    assert!(matches!(
        vault.get_secret("remove-me"),
        Err(VaultError::SecretNotFound)
    ));
    assert!(vault.list_secrets().unwrap().is_empty());
}

#[test]
#[cfg(windows)]
fn bounds_are_enforced_on_write_and_read() {
    let (directory, vault) = new_vault();
    let too_large = vec![0u8; MAX_PLAINTEXT_BYTES + 1];
    assert!(matches!(
        vault.seal(&too_large, "clipboard.text"),
        Err(VaultError::LimitExceeded { .. })
    ));
    assert!(matches!(
        vault.put_secret("too-big", &too_large),
        Err(VaultError::LimitExceeded { .. })
    ));

    let oversized = directory.path().join("secrets").join("oversized.sealed");
    fs::write(&oversized, vec![b'a'; MAX_SEALED_FILE_BYTES + 1]).unwrap();
    assert!(matches!(
        vault.get_secret("oversized"),
        Err(VaultError::LimitExceeded { .. })
    ));

    fs::write(&oversized, b"not a sealed payload").unwrap();
    assert!(matches!(
        vault.get_secret("oversized"),
        Err(VaultError::InvalidPayload { .. })
    ));
}

#[test]
#[cfg(windows)]
fn metadata_is_non_secret_and_newer_formats_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let vault = Vault::open(directory.path()).unwrap();
    drop(vault);

    let metadata = fs::read_to_string(directory.path().join("vault.json")).unwrap();
    assert!(metadata.contains("windows-dpapi-v1"));
    assert!(metadata.contains("\"format\":1"));

    fs::write(
        directory.path().join("vault.json"),
        br#"{"format":99,"backend":"windows-dpapi-v1"}"#,
    )
    .unwrap();
    assert!(matches!(
        Vault::open(directory.path()),
        Err(VaultError::UnsupportedVaultFormat {
            found: 99,
            supported: 1
        })
    ));

    fs::write(directory.path().join("vault.json"), b"{not json").unwrap();
    assert!(matches!(
        Vault::open(directory.path()),
        Err(VaultError::InvalidVault { .. })
    ));
}

#[test]
#[cfg(not(windows))]
fn non_windows_open_is_explicitly_unsupported_and_writes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let result = Vault::open(directory.path());
    match result {
        Err(VaultError::Unsupported { platform, .. }) => {
            assert_eq!(platform, std::env::consts::OS);
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
    assert_eq!(
        fs::read_dir(directory.path()).unwrap().count(),
        0,
        "no vault files may be created on an unsupported platform"
    );
}
