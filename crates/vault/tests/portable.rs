//! Portable crypto, fallback and file fixtures; runnable on Windows.

use ansa_vault::{
    AEAD_PAYLOAD_VERSION, MAX_MEMORY_SECRET_BYTES, MAX_PASSPHRASE_BYTES, MAX_PLAINTEXT_BYTES,
    PAYLOAD_VERSION, SealedPayload, Vault, VaultError, VaultMode, VaultState, VaultValidation,
};
use std::fs;

const PASSPHRASE: &[u8] = b"ansa-test-passphrase-fixture-single";
const NEW_PASSPHRASE: &[u8] = b"ansa-test-passphrase-fixture-replacement";
const SENTINEL: &[u8] = b"ansa-vault-fixture-never-plaintext-on-disk-201dc6";

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle)
}

/// Exact encoded size used by the memory-only named-secret map: the alias and
/// context-tag lengths plus the hexadecimal nonce/ciphertext/tag.
fn encoded_secret_size(alias: &str, plaintext_len: usize) -> usize {
    let context = format!("secret:{}", alias.to_ascii_lowercase());
    2 * alias.len() + context.len() + 2 * (plaintext_len + 40)
}

#[test]
fn memory_is_volatile_and_lock_destroys_old_payloads() {
    let vault = Vault::memory_only().unwrap();
    assert_eq!(vault.status().mode, VaultMode::MemoryOnly);
    assert!(!vault.status().mode.is_persistent());
    assert_eq!(vault.status().validation, VaultValidation::PortableTested);
    vault.put_secret("Example", SENTINEL).unwrap();
    vault.put_secret("example", SENTINEL).unwrap();
    assert_eq!(vault.list_secrets().unwrap(), vec!["Example"]);
    let external = vault.seal(SENTINEL, "clipboard.text").unwrap();
    assert!(!contains(&serde_json::to_vec(&external).unwrap(), SENTINEL));
    assert!(!contains(
        format!("{external:?} {vault:?}").as_bytes(),
        SENTINEL
    ));
    let secret = vault.get_secret_zeroizing("EXAMPLE").unwrap();
    assert_eq!(secret.as_slice(), SENTINEL);
    assert!(!contains(format!("{secret:?}").as_bytes(), SENTINEL));
    assert_eq!(vault.rotate().unwrap().secrets_resealed, 1);
    assert!(matches!(vault.verify_backup(), Err(VaultError::WrongMode)));
    vault.lock();
    assert_eq!(vault.status().state, VaultState::Locked);
    assert!(matches!(vault.list_secrets(), Err(VaultError::Locked)));
    assert!(matches!(
        vault.verify_payload(&external, "clipboard.text"),
        Err(VaultError::Locked)
    ));
    vault.try_unlock().unwrap();
    assert!(vault.list_secrets().unwrap().is_empty());
    assert!(matches!(
        vault.verify_payload(&external, "clipboard.text"),
        Err(VaultError::AuthenticationFailed)
    ));
}

#[test]
fn memory_quota_preserves_existing_secrets_on_rejection() {
    let vault = Vault::memory_only().unwrap();
    let large = vec![42; MAX_PLAINTEXT_BYTES];
    let payload_limit = MAX_MEMORY_SECRET_BYTES / (2 * large.len());
    // The encoded nonce/tag and alias overhead consume capacity too.
    for i in 0..payload_limit - 1 {
        vault.put_secret(&format!("item{i}"), &large).unwrap();
    }
    assert!(matches!(
        vault.put_secret("over", &large),
        Err(VaultError::LimitExceeded {
            kind: "memory-only encoded secret bytes",
            ..
        })
    ));
    assert_eq!(vault.list_secrets().unwrap().len(), payload_limit - 1);
    assert_eq!(vault.get_secret("item0").unwrap(), large);
    assert!(vault.delete_secret("item0").unwrap());
    vault.put_secret("over", &large).unwrap();
}

#[test]
fn transactional_replacement_preserves_entries_on_quota_failure() {
    let vault = Vault::memory_only().unwrap();
    vault.put_secret("keep", b"kept value").unwrap();
    vault.put_secret("victim", b"victim value").unwrap();

    // Fill the volatile vault to within 4 KiB of the encoded byte limit so the
    // small existing rows are known to fit but a 128 KiB replacement cannot.
    let mut used = encoded_secret_size("keep", 10) + encoded_secret_size("victim", 12);
    let large = vec![b'f'; MAX_PLAINTEXT_BYTES];
    for index in 0..7 {
        let alias = format!("unrelated{index}");
        vault.put_secret(&alias, &large).unwrap();
        used += encoded_secret_size(&alias, large.len());
    }
    let last_alias = "unrelated7";
    let remaining = MAX_MEMORY_SECRET_BYTES - 4096 - used;
    let fixed = 2 * last_alias.len() + format!("secret:{last_alias}").len();
    let last_len = (remaining - fixed) / 2 - 40;
    assert!(last_len <= MAX_PLAINTEXT_BYTES);
    vault.put_secret(last_alias, &vec![b'f'; last_len]).unwrap();

    assert!(matches!(
        vault.put_secret_replacing("replacement", &vec![b'r'; 128 * 1024], &["victim"]),
        Err(VaultError::LimitExceeded {
            kind: "memory-only encoded secret bytes",
            ..
        })
    ));
    // The failed transaction deleted nothing and installed nothing.
    assert_eq!(vault.get_secret("keep").unwrap(), b"kept value");
    assert_eq!(vault.get_secret("victim").unwrap(), b"victim value");
    assert!(matches!(
        vault.get_secret("replacement"),
        Err(VaultError::SecretNotFound)
    ));
    assert_eq!(vault.list_secrets().unwrap().len(), 10);

    // Invalid removal aliases fail closed too, before any mutation.
    assert!(matches!(
        vault.put_secret_replacing("replacement", b"small", &["bad alias"]),
        Err(VaultError::InvalidAlias { .. })
    ));
    assert!(matches!(
        vault.put_secret_replacing("bad alias", b"small", &["victim"]),
        Err(VaultError::InvalidAlias { .. })
    ));
    assert!(vault.get_secret("victim").is_ok());

    // A replacement that fits commits both halves together: the victim is
    // evicted and the new payload is visible.
    vault
        .put_secret_replacing("replacement", b"small", &["victim"])
        .unwrap();
    assert!(matches!(
        vault.get_secret("victim"),
        Err(VaultError::SecretNotFound)
    ));
    assert_eq!(vault.get_secret("replacement").unwrap(), b"small");
    assert_eq!(vault.get_secret("keep").unwrap(), b"kept value");
    let aliases = vault.list_secrets().unwrap();
    assert!(aliases.contains(&"replacement".to_owned()));
    assert!(!aliases.contains(&"victim".to_owned()));
}

#[test]
fn transactional_replacement_is_rejected_for_file_backed_vaults() {
    let temp = tempfile::tempdir().unwrap();
    let vault = Vault::open_passphrase(temp.path(), PASSPHRASE).unwrap();
    vault.put_secret("kept", SENTINEL).unwrap();
    assert!(matches!(
        vault.put_secret_replacing("replacement", b"new", &["kept"]),
        Err(VaultError::WrongMode)
    ));
    assert_eq!(vault.get_secret("kept").unwrap(), SENTINEL);
    assert!(matches!(
        vault.get_secret("replacement"),
        Err(VaultError::SecretNotFound)
    ));
}

#[test]
fn aead_envelope_is_bounded_and_authenticates_context_and_profile() {
    let first = Vault::memory_only().unwrap();
    let second = Vault::memory_only().unwrap();
    let sealed = first.seal(SENTINEL, "clipboard.text").unwrap();
    assert_eq!(sealed.version, AEAD_PAYLOAD_VERSION);
    assert_ne!(sealed, first.seal(SENTINEL, "clipboard.text").unwrap());
    let encoded = serde_json::to_vec(&sealed).unwrap();
    let decoded: SealedPayload = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, sealed);
    assert_eq!(
        first.open_payload(&decoded, "clipboard.text").unwrap(),
        SENTINEL
    );
    assert!(matches!(
        second.verify_payload(&decoded, "clipboard.text"),
        Err(VaultError::AuthenticationFailed)
    ));
    assert!(matches!(
        first.verify_payload(&decoded, "other"),
        Err(VaultError::WrongContext)
    ));
    let mut replaced = decoded.clone();
    replaced.context_tag = "other".into();
    assert!(matches!(
        first.verify_payload(&replaced, "other"),
        Err(VaultError::AuthenticationFailed)
    ));
    let mut changed = decoded.clone();
    let first_byte = if changed.ciphertext_hex.starts_with('a') {
        "b"
    } else {
        "a"
    };
    changed.ciphertext_hex.replace_range(..1, first_byte);
    assert!(matches!(
        first.verify_payload(&changed, "clipboard.text"),
        Err(VaultError::AuthenticationFailed)
    ));
    changed.version = PAYLOAD_VERSION;
    assert!(matches!(
        first.verify_payload(&changed, "clipboard.text"),
        Err(VaultError::UnsupportedPayloadVersion {
            found: PAYLOAD_VERSION,
            supported: AEAD_PAYLOAD_VERSION
        })
    ));
    let oversized = SealedPayload {
        version: AEAD_PAYLOAD_VERSION,
        context_tag: "clipboard.text".into(),
        ciphertext_hex: "0".repeat(2 * (MAX_PLAINTEXT_BYTES + 41)),
    };
    assert!(matches!(
        first.verify_payload(&oversized, "clipboard.text"),
        Err(VaultError::LimitExceeded { .. })
    ));
    assert!(matches!(
        first.seal(&vec![0; MAX_PLAINTEXT_BYTES + 1], "clipboard.text"),
        Err(VaultError::LimitExceeded { .. })
    ));
}

#[test]
fn passphrase_lock_and_wrong_password_never_modify_ciphertext() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let vault = Vault::open_passphrase(root, PASSPHRASE).unwrap();
    assert_eq!(vault.status().mode, VaultMode::Passphrase);
    assert!(vault.status().mode.is_persistent());
    vault.put_secret("check", SENTINEL).unwrap();
    let payload = vault.seal(SENTINEL, "external").unwrap();
    let path = root.join("secrets/check.sealed");
    let metadata = root.join("vault.json");
    let encrypted = fs::read(&path).unwrap();
    let header = fs::read(&metadata).unwrap();
    for output in [&encrypted, &header, &serde_json::to_vec(&payload).unwrap()] {
        assert!(!contains(output, SENTINEL));
        assert!(!contains(output, PASSPHRASE));
    }
    vault.lock();
    assert!(matches!(
        vault.try_unlock(),
        Err(VaultError::PassphraseRequired)
    ));
    vault.unlock(); // Legacy compatibility method does not bypass passphrase auth.
    assert!(matches!(
        vault.open_payload(&payload, "external"),
        Err(VaultError::Locked)
    ));
    assert!(matches!(
        vault.unlock_with_passphrase(b"wrong"),
        Err(VaultError::InvalidPassphrase)
    ));
    assert!(vault.is_locked());
    assert!(matches!(
        Vault::open_passphrase(root, b"wrong"),
        Err(VaultError::InvalidPassphrase)
    ));
    assert_eq!(fs::read(&path).unwrap(), encrypted);
    assert_eq!(fs::read(&metadata).unwrap(), header);
    vault.unlock_with_passphrase(PASSPHRASE).unwrap();
    assert_eq!(vault.get_secret("check").unwrap(), SENTINEL);
    assert_eq!(vault.open_payload(&payload, "external").unwrap(), SENTINEL);
}

#[test]
fn missing_metadata_and_wrong_mode_never_create_a_replacement_key() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    assert!(matches!(
        Vault::open_existing_passphrase(root, PASSPHRASE),
        Err(VaultError::MetadataMissing)
    ));
    assert!(matches!(
        Vault::open_passphrase(root, b""),
        Err(VaultError::PassphraseLength)
    ));
    assert!(matches!(
        Vault::open_passphrase(root, &vec![0; MAX_PASSPHRASE_BYTES + 1]),
        Err(VaultError::PassphraseLength)
    ));
    assert_eq!(fs::read_dir(root).unwrap().count(), 0);

    let metadata = root.join("vault.json");
    let dpapi_metadata = br#"{"format":1,"backend":"windows-dpapi-v1"}"#;
    fs::write(&metadata, dpapi_metadata).unwrap();
    assert!(matches!(
        Vault::open_passphrase(root, PASSPHRASE),
        Err(VaultError::WrongMode)
    ));
    assert_eq!(fs::read(&metadata).unwrap(), dpapi_metadata);
    fs::remove_file(&metadata).unwrap();
    fs::create_dir(root.join("secrets")).unwrap();
    let encrypted = root.join("secrets/old.sealed");
    // Format fixture representing ciphertext the caller already has. No KDF
    // or key generation should occur before missing metadata is rejected.
    let fixture = br#"{"version":2,"context_tag":"secret:old","ciphertext_hex":"00"}"#;
    fs::write(&encrypted, fixture).unwrap();
    assert!(matches!(
        Vault::open_passphrase(root, PASSPHRASE),
        Err(VaultError::MetadataMissing)
    ));
    assert!(matches!(
        Vault::open_existing_passphrase(root, PASSPHRASE),
        Err(VaultError::MetadataMissing)
    ));
    assert!(!metadata.exists());
    assert_eq!(fs::read(encrypted).unwrap(), fixture);
}

#[test]
fn passphrase_change_and_backup_preserve_payloads_and_verify_ciphertext() {
    let temp = tempfile::tempdir().unwrap();
    let live = temp.path().join("live");
    let backup = temp.path().join("backup");
    let vault = Vault::open_passphrase(&live, PASSPHRASE).unwrap();
    vault.put_secret("check", SENTINEL).unwrap();
    let payload = vault.seal(SENTINEL, "external").unwrap();
    let encrypted = fs::read(live.join("secrets/check.sealed")).unwrap();
    fs::create_dir_all(backup.join("secrets")).unwrap();
    fs::copy(live.join("vault.json"), backup.join("vault.json")).unwrap();
    fs::copy(
        live.join("secrets/check.sealed"),
        backup.join("secrets/check.sealed"),
    )
    .unwrap();

    assert!(matches!(
        vault.change_passphrase(b"wrong old passphrase", NEW_PASSPHRASE),
        Err(VaultError::InvalidPassphrase)
    ));
    assert_eq!(
        fs::read(live.join("vault.json")).unwrap(),
        fs::read(backup.join("vault.json")).unwrap()
    );
    assert_eq!(
        fs::read(live.join("secrets/check.sealed")).unwrap(),
        encrypted
    );
    assert_eq!(vault.get_secret("check").unwrap(), SENTINEL);
    assert!(matches!(
        vault.change_passphrase(PASSPHRASE, b""),
        Err(VaultError::PassphraseLength)
    ));
    assert!(matches!(
        vault.change_passphrase(PASSPHRASE, &vec![0; MAX_PASSPHRASE_BYTES + 1]),
        Err(VaultError::PassphraseLength)
    ));
    assert_eq!(
        fs::read(live.join("vault.json")).unwrap(),
        fs::read(backup.join("vault.json")).unwrap()
    );

    vault.change_passphrase(PASSPHRASE, NEW_PASSPHRASE).unwrap();
    assert_eq!(
        fs::read(live.join("secrets/check.sealed")).unwrap(),
        encrypted
    );
    assert_eq!(vault.verify_backup().unwrap().secrets_verified, 1);
    assert_eq!(vault.get_secret("check").unwrap(), SENTINEL);
    vault.lock();
    assert!(matches!(
        vault.unlock_with_passphrase(PASSPHRASE),
        Err(VaultError::InvalidPassphrase)
    ));
    assert!(vault.is_locked());
    vault.unlock_with_passphrase(NEW_PASSPHRASE).unwrap();
    assert_eq!(vault.get_secret("check").unwrap(), SENTINEL);
    assert!(matches!(
        Vault::open_existing_passphrase(&live, PASSPHRASE),
        Err(VaultError::InvalidPassphrase)
    ));
    let reopened = Vault::open_existing_passphrase(&live, NEW_PASSPHRASE).unwrap();
    reopened.verify_payload(&payload, "external").unwrap();
    assert_eq!(reopened.get_secret("check").unwrap(), SENTINEL);
    assert!(!contains(
        &fs::read(live.join("vault.json")).unwrap(),
        NEW_PASSPHRASE
    ));

    let restored = Vault::open_existing_passphrase(&backup, PASSPHRASE).unwrap();
    assert_eq!(restored.verify_backup().unwrap().secrets_verified, 1);
    restored.verify_payload(&payload, "external").unwrap();
    fs::write(backup.join("secrets/check.sealed"), b"truncated").unwrap();
    assert!(matches!(
        restored.verify_backup(),
        Err(VaultError::InvalidPayload { .. })
    ));
}

#[test]
fn locked_handle_rejects_a_restored_older_passphrase_header_until_reopened() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("vault");
    let vault = Vault::open_passphrase(&root, PASSPHRASE).unwrap();
    vault.put_secret("check", SENTINEL).unwrap();
    let sealed_path = root.join("secrets/check.sealed");
    let ciphertext = fs::read(&sealed_path).unwrap();

    // Header A is the one this handle opened.
    let opened_header = fs::read(root.join("vault.json")).unwrap();
    vault.change_passphrase(PASSPHRASE, NEW_PASSPHRASE).unwrap();
    let changed_header = fs::read(root.join("vault.json")).unwrap();
    assert_ne!(opened_header, changed_header);

    // Simulate restoring a backup snapshot while the process still holds the
    // handle that changed the passphrase. The public profile ID is unchanged.
    vault.lock();
    fs::write(root.join("vault.json"), &opened_header).unwrap();
    assert!(matches!(
        vault.unlock_with_passphrase(PASSPHRASE),
        Err(VaultError::InvalidVault { .. })
    ));
    assert!(vault.is_locked());
    assert_eq!(vault.status().state, VaultState::Locked);
    assert!(matches!(vault.get_secret("check"), Err(VaultError::Locked)));
    assert!(matches!(
        vault.change_passphrase(PASSPHRASE, NEW_PASSPHRASE),
        Err(VaultError::Locked)
    ));
    // The failed attempt must not overwrite the restored header.
    assert_eq!(fs::read(root.join("vault.json")).unwrap(), opened_header);
    assert_eq!(fs::read(&sealed_path).unwrap(), ciphertext);

    // Explicitly reopening is how a restored snapshot is adopted.
    let reopened = Vault::open_existing_passphrase(&root, PASSPHRASE).unwrap();
    assert_eq!(reopened.get_secret("check").unwrap(), SENTINEL);

    // Restoring the handle's own header recovers the original handle.
    fs::write(root.join("vault.json"), &changed_header).unwrap();
    vault.unlock_with_passphrase(NEW_PASSPHRASE).unwrap();
    assert_eq!(vault.get_secret("check").unwrap(), SENTINEL);
}

#[test]
fn interrupted_rotation_leaves_old_or_resealed_ciphertext_and_ignores_temps() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let vault = Vault::open_passphrase(root, PASSPHRASE).unwrap();
    for alias in ["alpha", "middle", "omega"] {
        vault.put_secret(alias, SENTINEL).unwrap();
    }
    let alpha = root.join("secrets/alpha.sealed");
    let middle = root.join("secrets/middle.sealed");
    let omega = root.join("secrets/omega.sealed");
    let old_alpha = fs::read(&alpha).unwrap();
    let old_middle = fs::read(&middle).unwrap();
    let old_omega = fs::read(&omega).unwrap();
    fs::write(root.join("secrets/.tmp-interrupted"), &old_middle[..8]).unwrap();
    fs::write(&middle, &old_middle[..8]).unwrap(); // Deterministic failure partway through rotate.
    assert!(matches!(
        vault.rotate(),
        Err(VaultError::InvalidPayload { .. })
    ));
    assert_ne!(fs::read(&alpha).unwrap(), old_alpha);
    assert_eq!(fs::read(&middle).unwrap(), &old_middle[..8]);
    assert_eq!(fs::read(&omega).unwrap(), old_omega);
    assert_eq!(vault.get_secret("alpha").unwrap(), SENTINEL);
    assert_eq!(vault.get_secret("omega").unwrap(), SENTINEL);
    fs::write(&middle, old_middle).unwrap();
    assert_eq!(vault.rotate().unwrap().secrets_resealed, 3);
    assert_eq!(vault.verify_backup().unwrap().secrets_verified, 3);
    for entry in fs::read_dir(root.join("secrets")).unwrap() {
        assert!(!contains(
            &fs::read(entry.unwrap().path()).unwrap(),
            SENTINEL
        ));
    }
}

#[test]
fn passphrase_header_is_bound_to_the_open_handle() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("vault");
    let other_root = temp.path().join("other");
    let header_path = root.join("vault.json");
    let sealed_path = root.join("secrets/check.sealed");
    let vault = Vault::open_passphrase(&root, PASSPHRASE).unwrap();
    vault.put_secret("check", SENTINEL).unwrap();
    let payload = vault.seal(SENTINEL, "external").unwrap();
    let original_header = fs::read(&header_path).unwrap();
    let original_ciphertext = fs::read(&sealed_path).unwrap();

    let other = Vault::open_passphrase(&other_root, b"other passphrase fixture").unwrap();
    drop(other);
    let other_header = fs::read(other_root.join("vault.json")).unwrap();

    fs::remove_file(&header_path).unwrap();
    assert!(matches!(
        vault.get_secret("check"),
        Err(VaultError::MetadataMissing)
    ));
    assert!(matches!(
        vault.put_secret("check", b"replacement"),
        Err(VaultError::MetadataMissing)
    ));
    assert!(matches!(
        vault.verify_payload(&payload, "external"),
        Err(VaultError::MetadataMissing)
    ));
    assert!(matches!(
        vault.verify_backup(),
        Err(VaultError::MetadataMissing)
    ));
    assert!(matches!(
        vault.try_unlock(),
        Err(VaultError::MetadataMissing)
    ));
    assert_eq!(fs::read(&sealed_path).unwrap(), original_ciphertext);

    fs::write(&header_path, &original_header).unwrap();
    assert_eq!(vault.get_secret("check").unwrap(), SENTINEL);
    assert_eq!(vault.verify_backup().unwrap().secrets_verified, 1);

    fs::write(&header_path, &other_header).unwrap();
    assert!(matches!(
        vault.get_secret("check"),
        Err(VaultError::InvalidVault { .. })
    ));
    assert!(matches!(
        vault.put_secret("check", b"replacement"),
        Err(VaultError::InvalidVault { .. })
    ));
    assert!(matches!(
        vault.rotate(),
        Err(VaultError::InvalidVault { .. })
    ));
    assert!(matches!(
        vault.verify_backup(),
        Err(VaultError::InvalidVault { .. })
    ));
    assert!(matches!(
        vault.try_unlock(),
        Err(VaultError::InvalidVault { .. })
    ));
    assert!(matches!(
        vault.change_passphrase(PASSPHRASE, NEW_PASSPHRASE),
        Err(VaultError::InvalidVault { .. })
    ));
    assert_eq!(fs::read(&sealed_path).unwrap(), original_ciphertext);

    fs::write(&header_path, original_header).unwrap();
    vault.lock();
    assert!(matches!(
        vault.unlock_with_passphrase(b"other passphrase fixture"),
        Err(VaultError::InvalidPassphrase)
    ));
    assert!(vault.is_locked());
    vault.unlock_with_passphrase(PASSPHRASE).unwrap();
    assert_eq!(vault.get_secret("check").unwrap(), SENTINEL);
    assert_eq!(vault.verify_backup().unwrap().secrets_verified, 1);
}

#[test]
fn passphrase_profiles_are_not_interchangeable() {
    let temp = tempfile::tempdir().unwrap();
    let first_root = temp.path().join("first");
    let second_root = temp.path().join("second");
    let first = Vault::open_passphrase(&first_root, PASSPHRASE).unwrap();
    let second = Vault::open_passphrase(&second_root, PASSPHRASE).unwrap();
    let payload = first.seal(SENTINEL, "external").unwrap();
    assert!(matches!(
        second.verify_payload(&payload, "external"),
        Err(VaultError::AuthenticationFailed)
    ));

    first.put_secret("check", SENTINEL).unwrap();
    fs::copy(
        first_root.join("secrets/check.sealed"),
        second_root.join("secrets/check.sealed"),
    )
    .unwrap();
    assert!(matches!(
        second.get_secret("check"),
        Err(VaultError::AuthenticationFailed)
    ));
    assert_eq!(first.get_secret("check").unwrap(), SENTINEL);
}

#[cfg(unix)]
#[test]
fn unix_permissions_and_symlinks_fail_closed() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("vault");
    let vault = Vault::open_passphrase(&root, PASSPHRASE).unwrap();
    vault.put_secret("check", SENTINEL).unwrap();

    for path in [root.clone(), root.join("secrets")] {
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode & 0o077,
            0,
            "{path:?} must not be group/other accessible"
        );
        assert_ne!(mode & 0o700, 0);
    }
    for path in [root.join("vault.json"), root.join("secrets/check.sealed")] {
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode & 0o077,
            0,
            "{path:?} must not be group/other accessible"
        );
        assert_ne!(mode & 0o600, 0);
    }

    let header = root.join("vault.json");
    let real_header = root.join("vault.json.real");
    fs::rename(&header, &real_header).unwrap();
    symlink(&real_header, &header).unwrap();
    assert!(matches!(
        vault.get_secret("check"),
        Err(VaultError::InvalidVault { .. })
    ));
    assert!(matches!(
        vault.verify_backup(),
        Err(VaultError::InvalidVault { .. })
    ));
    fs::remove_file(&header).unwrap();
    fs::rename(&real_header, &header).unwrap();
    assert_eq!(vault.get_secret("check").unwrap(), SENTINEL);

    symlink(
        root.join("secrets/check.sealed"),
        root.join("secrets/link.sealed"),
    )
    .unwrap();
    assert!(matches!(
        vault.get_secret("link"),
        Err(VaultError::SecretNotFound)
    ));
    assert_eq!(vault.list_secrets().unwrap(), vec!["check"]);
    assert_eq!(vault.verify_backup().unwrap().secrets_verified, 1);
}

#[cfg(unix)]
#[test]
fn ambiguous_case_variant_aliases_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let vault = Vault::open_passphrase(root, PASSPHRASE).unwrap();
    vault.put_secret("Token", SENTINEL).unwrap();
    let upper_path = root.join("secrets/Token.sealed");
    let lower_path = root.join("secrets/token.sealed");

    if directory_is_case_insensitive(root) {
        // On case-insensitive APFS the two names are one directory entry, so
        // creating a second physical file is impossible and there is no
        // ambiguity to test. Assert the single-entry behavior instead.
        assert!(lower_path.exists());
        assert_eq!(
            fs::read(&upper_path).unwrap(),
            fs::read(&lower_path).unwrap()
        );
        assert_eq!(vault.list_secrets().unwrap(), vec!["Token"]);
        assert_eq!(vault.verify_backup().unwrap().secrets_verified, 1);
        assert_eq!(vault.get_secret("Token").unwrap(), SENTINEL);
        assert_eq!(vault.get_secret("token").unwrap(), SENTINEL);
        assert_eq!(vault.get_secret("TOKEN").unwrap(), SENTINEL);

        // A case-variant write lands on the one entry instead of duplicating
        // the alias, and the stored alias spelling is preserved.
        vault.put_secret("token", b"replacement").unwrap();
        assert_eq!(vault.get_secret("Token").unwrap(), b"replacement");
        assert_eq!(vault.list_secrets().unwrap(), vec!["Token"]);
        assert_eq!(vault.verify_backup().unwrap().secrets_verified, 1);
        assert!(vault.delete_secret("TOKEN").unwrap());
        assert!(!upper_path.exists());
        assert!(vault.list_secrets().unwrap().is_empty());
        return;
    }

    fs::copy(&upper_path, &lower_path).unwrap();

    assert_eq!(vault.list_secrets().unwrap(), vec!["Token", "token"]);
    assert_eq!(vault.verify_backup().unwrap().secrets_verified, 2);
    assert_eq!(vault.get_secret("Token").unwrap(), SENTINEL);
    assert_eq!(vault.get_secret("token").unwrap(), SENTINEL);
    assert!(matches!(
        vault.get_secret("TOKEN"),
        Err(VaultError::InvalidVault { .. })
    ));
    assert!(matches!(
        vault.put_secret("TOKEN", b"replacement"),
        Err(VaultError::InvalidVault { .. })
    ));
    assert!(matches!(
        vault.delete_secret("TOKEN"),
        Err(VaultError::InvalidVault { .. })
    ));

    assert!(vault.delete_secret("token").unwrap());
    assert!(!lower_path.exists());
    assert!(upper_path.exists());
    assert_eq!(vault.get_secret("TOKEN").unwrap(), SENTINEL);
    assert!(vault.delete_secret("Token").unwrap());
    assert!(!upper_path.exists());
}

#[cfg(unix)]
fn directory_is_case_insensitive(directory: &std::path::Path) -> bool {
    let lower = directory.join(".ansa-case-probe");
    let upper = directory.join(".ANSA-CASE-PROBE");
    fs::write(&lower, b"probe").unwrap();
    // A case-sensitive filesystem does not resolve the differently cased
    // spelling; a case-insensitive one resolves it to the same entry.
    let insensitive = fs::metadata(&upper).is_ok();
    if insensitive {
        assert_eq!(fs::read(&upper).unwrap(), b"probe");
    }
    fs::remove_file(&lower).unwrap();
    insensitive
}
