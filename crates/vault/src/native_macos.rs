//! Local file-based user Keychain, using Security.framework generic passwords.

use std::ptr;
use std::sync::Arc;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::CFData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};
use security_framework_sys::base::SecKeychainRef;
use security_framework_sys::item::*;
use security_framework_sys::keychain::{SecKeychainCopyDomainDefault, SecPreferencesDomain};
use security_framework_sys::keychain_item::{SecItemAdd, SecItemCopyMatching};

use crate::SecureStorageIssue as Issue;
use crate::crypto::{self, Key};
use crate::portable::NativeKeyStore;

pub(crate) struct MacKeyStore;

const SERVICE: &str = "dev.ansa.vault.v1";
const UNLOCKED: u32 = 1; // kSecUnlockStateStatus

struct KeyData(Key);

impl AsRef<[u8]> for KeyData {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecUseAuthenticationUIFail: CFStringRef;
    fn SecKeychainGetStatus(keychain: SecKeychainRef, status: *mut u32) -> i32;
}

/// Keychain access policy for one operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KeychainAccess {
    /// Refuse to query until `SecKeychainGetStatus` reports the default
    /// Keychain unlocked. This is the routine, no-UI path.
    require_unlocked: bool,
}

/// Only an explicit interactive read skips the unlocked precheck so
/// Security.framework can unlock or prompt for the default Keychain. Routine
/// loads and provisioning keep the precheck and fail closed without UI.
fn keychain_access(interactive: bool) -> KeychainAccess {
    KeychainAccess {
        require_unlocked: !interactive,
    }
}

fn default_keychain(access: KeychainAccess) -> Result<CFType, Issue> {
    let mut keychain = ptr::null_mut();
    // SAFETY: valid out pointer, selecting the user's Keychain domain.
    status(unsafe { SecKeychainCopyDomainDefault(SecPreferencesDomain::User, &mut keychain) })?;
    if keychain.is_null() {
        return Err(Issue::Unavailable);
    }
    // SAFETY: the Copy call returned an owned CF keychain reference.
    let owned = unsafe { CFType::wrap_under_create_rule(keychain.cast()) };
    if access.require_unlocked {
        let mut flags = 0;
        // SAFETY: `owned` keeps the keychain live and `flags` is correctly typed.
        status(unsafe { SecKeychainGetStatus(keychain, &mut flags) })?;
        if flags & UNLOCKED == 0 {
            return Err(Issue::Locked);
        }
    }
    Ok(owned)
}

fn attributes(id: &str) -> Vec<(CFString, CFType)> {
    // SAFETY: Security.framework provides live constant CFStrings. Wrap with
    // the get rule because the program does not own the globals.
    unsafe {
        vec![
            (
                CFString::wrap_under_get_rule(kSecClass),
                CFType::wrap_under_get_rule(kSecClassGenericPassword.cast()),
            ),
            (
                CFString::wrap_under_get_rule(kSecAttrService),
                CFString::new(SERVICE).as_CFType(),
            ),
            (
                CFString::wrap_under_get_rule(kSecAttrAccount),
                CFString::new(id).as_CFType(),
            ),
        ]
    }
}

impl NativeKeyStore for MacKeyStore {
    fn add(&self, id: &str, key: &Key) -> Result<(), Issue> {
        // Provisioning is routine (not an explicit interactive unlock) and
        // still requires the default Keychain to report unlocked.
        let keychain = default_keychain(keychain_access(false))?;
        // Supply CFData from zeroizing Rust-owned storage rather than creating
        // a second, unzeroized application-side key buffer in CoreFoundation.
        let value = crypto::key_from_bytes(key).ok_or(Issue::InvalidKey)?;
        let data = CFData::from_arc(Arc::new(KeyData(value)));
        let mut attributes = attributes(id);
        // SAFETY: Security.framework constants are valid CFString references;
        // the dictionary retains the Keychain and the CFData during the call.
        unsafe {
            attributes.push((CFString::wrap_under_get_rule(kSecUseKeychain), keychain));
            attributes.push((
                CFString::wrap_under_get_rule(kSecValueData),
                data.as_CFType(),
            ));
            attributes.push((
                CFString::wrap_under_get_rule(kSecUseAuthenticationUI),
                CFType::wrap_under_get_rule(kSecUseAuthenticationUIFail.cast()),
            ));
        }
        let query = CFDictionary::from_CFType_pairs(&attributes);
        // SAFETY: a live attributes dictionary; no returned object requested.
        status(unsafe { SecItemAdd(query.as_concrete_TypeRef(), ptr::null_mut()) })
    }

    fn load(&self, id: &str, interactive: bool) -> Result<Key, Issue> {
        // A routine load prechecks the lock state and keeps
        // `kSecUseAuthenticationUIFail`, so it never shows UI. An explicit
        // interactive load skips the precheck and lets the item query attempt
        // recovery (including a system Keychain-unlock prompt) where the OS
        // permits it. Only an interactive `AccessDenied` plus a confirmed
        // still-locked status remaps to `Locked`; other failures keep their
        // original mapping.
        let keychain = default_keychain(keychain_access(interactive))?;
        let keychain_ref = keychain.as_CFTypeRef() as SecKeychainRef;
        let keychains = CFArray::from_CFTypes(&[keychain]);
        let mut attributes = attributes(id);
        // SAFETY: Security.framework constants are valid CFString references.
        unsafe {
            attributes.push((
                CFString::wrap_under_get_rule(kSecMatchSearchList),
                keychains.as_CFType(),
            ));
            attributes.push((
                CFString::wrap_under_get_rule(kSecReturnData),
                CFBoolean::true_value().as_CFType(),
            ));
            if !interactive {
                attributes.push((
                    CFString::wrap_under_get_rule(kSecUseAuthenticationUI),
                    CFType::wrap_under_get_rule(kSecUseAuthenticationUIFail.cast()),
                ));
            }
        }
        let query = CFDictionary::from_CFType_pairs(&attributes);
        let mut result = ptr::null();
        // SAFETY: live query and valid out pointer for an owned CF reference.
        let code = unsafe { SecItemCopyMatching(query.as_concrete_TypeRef(), &mut result) };
        if let Err(issue) = status(code) {
            // An interactive query can fail with errSecInteractionNotAllowed
            // when the Keychain stayed locked after the prompt. Recheck the
            // lock state so the caller receives `Locked` rather than a generic
            // access denial. `Locked` is only a state report; the caller/UI
            // supplies any system-unlock instruction.
            return Err(interactive_issue(
                interactive,
                issue,
                keychain_locked(keychain_ref),
            ));
        }
        if result.is_null() {
            return Err(Issue::InvalidKey);
        }
        // SAFETY: CopyMatching returned ownership. Verify the type before
        // treating the result as CFData.
        let object = unsafe { CFType::wrap_under_create_rule(result) };
        let data = object.downcast::<CFData>().ok_or(Issue::InvalidKey)?;
        crypto::key_from_bytes(data.bytes()).ok_or(Issue::InvalidKey)
    }
}

/// Map an interactive query failure to `Locked` when the default Keychain is
/// still locked. A failed status recheck leaves the original issue unchanged.
fn interactive_issue(interactive: bool, issue: Issue, keychain_locked: Option<bool>) -> Issue {
    if interactive && issue == Issue::AccessDenied && keychain_locked == Some(true) {
        Issue::Locked
    } else {
        issue
    }
}

/// Best-effort lock-state recheck after an item query. `None` means the status
/// call itself failed, in which case the original query error is reported.
fn keychain_locked(keychain: SecKeychainRef) -> Option<bool> {
    let mut flags = 0;
    // SAFETY: the caller passes a reference to a live default Keychain it
    // keeps retained for the duration of the call.
    status(unsafe { SecKeychainGetStatus(keychain, &mut flags) })
        .ok()
        .map(|()| flags & UNLOCKED == 0)
}

fn status(code: i32) -> Result<(), Issue> {
    // See Security.framework/SecBase.h. Do not retain OS messages or objects.
    match code {
        0 => Ok(()),
        -25300 => Err(Issue::KeyMissing), // errSecItemNotFound
        -25293 | -128 | -25308 | -61 => Err(Issue::AccessDenied), // auth / cancel / UI / write permissions
        -25299 | -26275 => Err(Issue::InvalidKey),                // duplicate item / decode failed
        _ => Err(Issue::Unavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::{interactive_issue, keychain_access};
    use crate::SecureStorageIssue as Issue;

    #[test]
    fn only_explicit_interactive_access_skips_the_unlocked_precheck() {
        // Pure policy check over the access plan. It neither links nor calls
        // Security.framework, so it cannot attest runtime prompt behavior.
        assert!(keychain_access(false).require_unlocked);
        assert!(!keychain_access(true).require_unlocked);
    }

    #[test]
    fn an_interactive_denial_maps_to_locked_only_while_locked() {
        // Pure mapping test over a supplied lock-state observation; it does not
        // call Security.framework.
        assert_eq!(
            interactive_issue(true, Issue::AccessDenied, Some(true)),
            Issue::Locked
        );
        assert_eq!(
            interactive_issue(true, Issue::AccessDenied, Some(false)),
            Issue::AccessDenied
        );
        assert_eq!(
            interactive_issue(false, Issue::AccessDenied, Some(true)),
            Issue::AccessDenied
        );
        assert_eq!(
            interactive_issue(true, Issue::KeyMissing, Some(true)),
            Issue::KeyMissing
        );
        assert_eq!(
            interactive_issue(true, Issue::AccessDenied, None),
            Issue::AccessDenied
        );
    }
}
