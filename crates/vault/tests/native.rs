//! Opt-in tests: require an actual unlocked native keyring.

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::fs;

use ansa_vault::{Vault, VaultMode, VaultValidation};

#[test]
#[ignore = "requires a real logged-in user with an unlocked persistent keyring"]
fn native_keyring_round_trip_and_backup() {
    let temp = tempfile::tempdir().unwrap();
    let live = temp.path().join("live");
    let restore = temp.path().join("restore");
    const SECRET: &[u8] = b"ansa-native-fixture-plaintext-sentinel-a74d";
    let vault = Vault::open(&live).unwrap();
    #[cfg(target_os = "macos")]
    assert_eq!(vault.status().mode, VaultMode::MacosKeychain);
    #[cfg(target_os = "linux")]
    assert_eq!(vault.status().mode, VaultMode::LinuxSecretService);
    assert_eq!(vault.status().validation, VaultValidation::NativeUnverified);
    vault.put_secret("check", SECRET).unwrap();
    let payload = vault.seal(SECRET, "clipboard.text").unwrap();
    vault.lock();
    assert!(vault.is_locked());
    vault.try_unlock().unwrap();
    assert_eq!(
        vault.get_secret_zeroizing("check").unwrap().as_slice(),
        SECRET
    );
    assert_eq!(vault.rotate().unwrap().secrets_resealed, 1);
    drop(vault);
    fs::create_dir_all(restore.join("secrets")).unwrap();
    for rel in ["vault.json", "secrets/check.sealed"] {
        let source = live.join(rel);
        assert!(
            !fs::read(&source)
                .unwrap()
                .windows(SECRET.len())
                .any(|w| w == SECRET)
        );
        fs::copy(source, restore.join(rel)).unwrap();
    }
    let restored = Vault::open_existing(&restore, false).unwrap();
    assert_eq!(restored.verify_backup().unwrap().secrets_verified, 1);
    restored.verify_payload(&payload, "clipboard.text").unwrap();
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "needs a Linux session with no Secret Service (e.g. isolated headless CI)"]
fn no_keyring_profile_never_silently_downgrades() {
    use ansa_vault::{SecureStorageIssue, VaultError};

    let temp = tempfile::tempdir().unwrap();
    let native = temp.path().join("native");
    assert!(matches!(
        Vault::open(&native),
        Err(VaultError::SecureStorage {
            issue: SecureStorageIssue::Unavailable | SecureStorageIssue::TimedOut
        })
    ));
    assert!(!native.exists());
    let passphrase =
        Vault::open_passphrase(temp.path().join("passphrase"), b"headless fixture").unwrap();
    assert_eq!(passphrase.status().mode, VaultMode::Passphrase);
    let before = fs::read_dir(temp.path()).unwrap().count();
    let memory = Vault::memory_only().unwrap();
    memory.put_secret("volatile", b"volatile fixture").unwrap();
    assert_eq!(memory.status().mode, VaultMode::MemoryOnly);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), before);
}
