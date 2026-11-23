//! Freedesktop Secret Service with a negotiated encrypted D-Bus session.

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use async_io::Timer;
use futures_lite::future;
use secret_service::{EncryptionType, Error, SecretService};
use zeroize::Zeroizing;

use crate::SecureStorageIssue as Issue;
use crate::crypto::{self, Key};
use crate::portable::NativeKeyStore;

pub(crate) struct LinuxKeyStore;

const LABEL: &str = "Ansa Vault";
const OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
const PROMPT_TIMEOUT: Duration = Duration::from_secs(30);

fn attributes(id: &str) -> HashMap<&str, &str> {
    HashMap::from([("application", "ansa-vault"), ("profile-id", id)])
}

fn deadline<T>(
    duration: Duration,
    operation: impl Future<Output = Result<T, Issue>>,
) -> Result<T, Issue> {
    future::block_on(future::race(operation, async move {
        Timer::after(duration).await;
        Err(Issue::TimedOut)
    }))
}

impl NativeKeyStore for LinuxKeyStore {
    fn add(&self, id: &str, key: &Key) -> Result<(), Issue> {
        deadline(OPERATION_TIMEOUT, async {
            let service = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(map_error)?;
            // Never choose an arbitrary, transient or session-only collection.
            let collection = service.get_default_collection().await.map_err(map_error)?;
            match service.get_collection_by_alias("session").await {
                Ok(session) if collection.collection_path == session.collection_path => {
                    return Err(Issue::Unavailable);
                }
                Ok(_) | Err(Error::NoResult) => {}
                Err(error) => return Err(map_error(error)),
            }
            // Provisioning is not a routine read: it checks that the persistent
            // default collection is already unlocked. In secret-service 5.2 this
            // call only tests the lock state; it does not itself prompt.
            collection.ensure_unlocked().await.map_err(map_error)?;
            // Residual prompt path: after an unlocked report the provider may
            // still require authorization and return a D-Bus prompt for
            // CreateItem. secret-service 5.2 executes that prompt internally
            // (collection.rs `create_item`) and exposes no noninteractive
            // create variant. The five-second deadline bounds the wait, but
            // this crate cannot suppress that provider prompt.
            collection
                .create_item(
                    LABEL,
                    attributes(id),
                    key,
                    false,
                    "application/octet-stream",
                )
                .await
                .map_err(map_error)?;
            Ok(())
        })
    }

    fn load(&self, id: &str, interactive: bool) -> Result<Key, Issue> {
        deadline(
            if interactive {
                PROMPT_TIMEOUT
            } else {
                OPERATION_TIMEOUT
            },
            async {
                let service = SecretService::connect(EncryptionType::Dh)
                    .await
                    .map_err(map_error)?;
                let found = service
                    .search_items(attributes(id))
                    .await
                    .map_err(map_error)?;
                let item = match (found.unlocked.as_slice(), found.locked.as_slice()) {
                    ([item], []) => item,
                    // Only explicit interactive access asks the provider to
                    // unlock the item. Routine loads report Locked without
                    // calling an unlock or prompt API.
                    ([], [item]) if interactive => {
                        item.unlock().await.map_err(map_error)?;
                        item
                    }
                    ([], [_]) => return Err(Issue::Locked),
                    ([], []) => return Err(Issue::KeyMissing),
                    _ => return Err(Issue::InvalidKey),
                };
                let bytes = Zeroizing::new(item.get_secret().await.map_err(map_error)?);
                crypto::key_from_bytes(&bytes).ok_or(Issue::InvalidKey)
            },
        )
    }
}

fn map_error(error: Error) -> Issue {
    match error {
        Error::Locked => Issue::Locked,
        Error::Prompt => Issue::AccessDenied,
        Error::PromptDisconnected => Issue::Unavailable,
        Error::Crypto(_) => Issue::InvalidKey,
        Error::NoResult | Error::Unavailable => Issue::Unavailable,
        Error::Zbus(zbus::Error::MethodError(name, _, _)) => match name.as_str() {
            "org.freedesktop.Secret.Error.IsLocked" => Issue::Locked,
            "org.freedesktop.Secret.Error.NoSuchObject"
            | "org.freedesktop.DBus.Error.UnknownObject" => Issue::KeyMissing,
            "org.freedesktop.DBus.Error.AccessDenied" | "org.freedesktop.DBus.Error.AuthFailed" => {
                Issue::AccessDenied
            }
            "org.freedesktop.DBus.Error.NoReply" | "org.freedesktop.DBus.Error.Timeout" => {
                Issue::TimedOut
            }
            _ => Issue::Unavailable,
        },
        Error::ZbusFdo(zbus::fdo::Error::AccessDenied(_) | zbus::fdo::Error::AuthFailed(_)) => {
            Issue::AccessDenied
        }
        _ => Issue::Unavailable,
    }
}
