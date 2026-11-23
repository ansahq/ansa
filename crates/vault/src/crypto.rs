use argon2::{Algorithm, Argon2, Block, Params, Version};
use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{Tag, XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use crate::{MAX_PASSPHRASE_BYTES, VaultError};

pub(crate) const KEY_BYTES: usize = 32;
pub(crate) const ID_BYTES: usize = 16;
pub(crate) const OVERHEAD: usize = 24 + 16;
pub(crate) type Key = Zeroizing<Box<[u8]>>;
pub(crate) type ProfileId = [u8; ID_BYTES];

pub(crate) fn random<const N: usize>() -> Result<[u8; N], VaultError> {
    let mut bytes = [0; N];
    fill_random(&mut bytes)?;
    Ok(bytes)
}

fn fill_random(bytes: &mut [u8]) -> Result<(), VaultError> {
    getrandom::fill(bytes).map_err(|_| VaultError::Internal {
        operation: "generating vault randomness",
    })
}

pub(crate) fn empty_key() -> Key {
    Zeroizing::new(Box::new([0; KEY_BYTES]))
}

pub(crate) fn random_key() -> Result<Key, VaultError> {
    let mut key = empty_key();
    fill_random(&mut key[..])?;
    Ok(key)
}

pub(crate) fn key_from_bytes(bytes: &[u8]) -> Option<Key> {
    if bytes.len() != KEY_BYTES {
        return None;
    }
    let mut key = empty_key();
    key.copy_from_slice(bytes);
    Some(key)
}

pub(crate) fn validate_passphrase(passphrase: &[u8]) -> Result<(), VaultError> {
    if passphrase.is_empty() || passphrase.len() > MAX_PASSPHRASE_BYTES {
        return Err(VaultError::PassphraseLength);
    }
    Ok(())
}

pub(crate) fn derive(passphrase: &[u8], salt: &ProfileId) -> Result<Key, VaultError> {
    validate_passphrase(passphrase)?;
    // Fixed format-v1 parameters; never permit on-disk data to choose KDF cost.
    let params =
        Params::new(64 * 1024, 3, 1, Some(KEY_BYTES)).map_err(|_| VaultError::Internal {
            operation: "configuring Argon2id",
        })?;
    let mut blocks = Zeroizing::new(Vec::new());
    blocks
        .try_reserve_exact(params.block_count())
        .map_err(|_| VaultError::Internal {
            operation: "allocating Argon2id workspace",
        })?;
    blocks.resize(params.block_count(), Block::default());
    let mut key = empty_key();
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into_with_memory(passphrase, salt, &mut key[..], blocks.as_mut_slice())
        .map_err(|_| VaultError::Internal {
            operation: "deriving Argon2id key",
        })?;
    Ok(key)
}

pub(crate) fn aad(domain: &[u8], id: &ProfileId, context: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(domain.len() + id.len() + context.len());
    aad.extend_from_slice(domain);
    aad.extend_from_slice(id);
    aad.extend_from_slice(context);
    aad
}

fn cipher(key: &Key) -> Result<XChaCha20Poly1305, VaultError> {
    XChaCha20Poly1305::new_from_slice(key).map_err(|_| VaultError::Internal {
        operation: "initializing XChaCha20-Poly1305 key",
    })
}

pub(crate) fn seal(key: &Key, bytes: &[u8], aad: &[u8]) -> Result<Vec<u8>, VaultError> {
    let nonce = random::<24>()?;
    let aead = cipher(key)?;
    let mut buffer = Zeroizing::new(bytes.to_vec());
    let tag = aead
        .encrypt_in_place_detached(XNonce::from_slice(&nonce), aad, buffer.as_mut_slice())
        .map_err(|_| VaultError::Internal {
            operation: "sealing XChaCha20-Poly1305 payload",
        })?;
    let mut frame = Vec::with_capacity(bytes.len() + OVERHEAD);
    frame.extend_from_slice(&nonce);
    frame.extend_from_slice(&buffer);
    frame.extend_from_slice(&tag);
    Ok(frame)
}

pub(crate) fn open(key: &Key, frame: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    if frame.len() < OVERHEAD {
        return Err(VaultError::InvalidPayload {
            reason: "AEAD frame is truncated",
        });
    }
    let (nonce, rest) = frame.split_at(24);
    let (bytes, tag) = rest.split_at(rest.len() - 16);
    let mut buffer = Zeroizing::new(bytes.to_vec());
    cipher(key)?
        .decrypt_in_place_detached(
            XNonce::from_slice(nonce),
            aad,
            buffer.as_mut_slice(),
            Tag::from_slice(tag),
        )
        .map_err(|_| VaultError::AuthenticationFailed)?;
    Ok(buffer)
}
