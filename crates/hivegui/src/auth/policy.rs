//! Password strength policy (T-AUTH-1, FR-049).
//!
//! - Minimum 12 characters; >= 3 character classes (lower / upper / digit / symbol).
//! - Reject the same as the OS's `pw_passwd` for the current uid
//!   (injectable for tests via `set_os_pw_passwd_for_test`).
//! - Reject the empty string, all-whitespace, all-same-char, etc.

use std::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasswordPolicy;

impl PasswordPolicy {
    /// Return the default policy. Currently a zero-sized struct; the
    /// function exists so callers don't write `PasswordPolicy` and
    /// reserve a future hook for policy reload.
    pub fn load_default() -> Self {
        Self
    }

    /// Evaluate a candidate password. Returns a `PasswordRejection` that
    /// is either `Accepted` (with a brief reason) or one of the typed
    /// rejection variants. `is_blocking()` collapses all blocking
    /// variants to true.
    pub fn evaluate(&self, password: &str) -> PasswordRejection {
        if password.is_empty() {
            return PasswordRejection::TooShort { min: 12 };
        }
        if password.len() < 12 {
            return PasswordRejection::TooShort { min: 12 };
        }
        let classes = [
            password.chars().any(|c| c.is_ascii_lowercase()),
            password.chars().any(|c| c.is_ascii_uppercase()),
            password.chars().any(|c| c.is_ascii_digit()),
            password.chars().any(|c| !c.is_alphanumeric()),
        ];
        if classes.iter().filter(|c| **c).count() < 3 {
            return PasswordRejection::InsufficientCharacterClasses {
                required: 3,
                actual: classes.iter().filter(|c| **c).count() as u8,
            };
        }
        let mut chars = password.chars();
        let first = chars.next().unwrap();
        if chars.all(|c| c == first) {
            return PasswordRejection::AllSameChar;
        }
        if password.trim().is_empty() {
            return PasswordRejection::WhitespaceOnly;
        }
        if let Some(os_pw) = os_pw_passwd() {
            if password == os_pw {
                return PasswordRejection::OsPwPasswdOverlap;
            }
        }
        PasswordRejection::Accepted {
            length: password.len(),
        }
    }

    /// Legacy shortcut: returns `true` iff the policy considers the
    /// password strong.
    pub fn is_strong(password: &str) -> bool {
        matches!(
            PasswordPolicy.evaluate(password),
            PasswordRejection::Accepted { .. }
        )
    }

    /// Inject the OS's `pw_passwd` value for the current uid. Production
    /// code calls this with `getpwuid(getuid()).pw_passwd`; tests
    /// inject a known string and assert the policy rejects the same
    /// string.
    pub fn set_os_pw_passwd_for_test(&self, value: &str) {
        *OS_PW_PASSWD.write().expect("os pw lock poisoned") = Some(value.to_string());
    }
}

static OS_PW_PASSWD: RwLock<Option<String>> = RwLock::new(None);

fn os_pw_passwd() -> Option<String> {
    OS_PW_PASSWD.read().expect("os pw lock poisoned").clone()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordRejection {
    Accepted { length: usize },
    TooShort { min: usize },
    InsufficientCharacterClasses { required: u8, actual: u8 },
    AllSameChar,
    WhitespaceOnly,
    OsPwPasswdOverlap,
}

impl PasswordRejection {
    /// Returns `true` iff this rejection should block the password
    /// from being accepted. The single `false` value is `Accepted`.
    pub fn is_blocking(&self) -> bool {
        !matches!(self, PasswordRejection::Accepted { .. })
    }
}
