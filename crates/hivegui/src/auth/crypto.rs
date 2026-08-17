//! Cryptographic primitives for the local master-password authentication
//! (T-AUTH-5 / FR-049).
//!
//! - Argon2id (m=64MiB, t=3, p=1) for KEK derivation. 5s injected
//!   deadline → `DerivationTimeout` + fail-closed.
//! - ChaCha20Poly1305 (RFC 8439) for wrapping the device key material.
//! - `kek_verifier` = `ChaCha20Poly1305(kek, nonce, [0; 0])` — empty plaintext
//!   ciphertext proves the KEK without exposing it.
//! - All 32-byte secrets wrapped in `Zeroizing<[u8; 32]>`.

use std::time::{Duration, Instant};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::rngs::OsRng;
use rand::{RngCore, SeedableRng};
use thiserror::Error;
use zeroize::Zeroize;
/// Errors raised by the local master-password crypto primitives.
#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("kek derivation failed: {0}")]
    /// Underlying Argon2id derivation failure.
    DerivationFailed(String),
    #[error("kek derivation exceeded {0:?} deadline")]
    /// KEK derivation exceeded the fail-closed deadline.
    DerivationTimeout(Duration),
    #[error("kek verifier mismatch (collapse to InvalidPassword)")]
    /// KEK verifier did not decrypt; collapses "wrong key" with "tampered blob".
    KekVerifierMismatch,
    #[error("AES-GCM wrap/unwrap failed: {0}")]
    /// AEAD wrap/unwrap failure.
    AeadFailed(String),
    #[error("invalid file layout: {0}")]
    /// On-disk keystore blob had an unexpected layout.
    InvalidLayout(String),
}

/// 32-byte secret. On drop, `bytes` is zeroed via `Zeroize` (the
/// `Zeroizing<[u8; 32]>` wrapper forces the optimizer to actually run
/// the drop code, which is the same guarantee `zeroize` gives).
/// 32-byte secret buffer that is wiped on drop via the `Zeroizing` wrapper.
#[derive(Clone)]
pub struct Secret32 {
    bytes: zeroize::Zeroizing<[u8; 32]>,
}

impl Secret32 {
    /// Generate a fresh `Secret32` filled with 32 bytes drawn from the
    /// OS CSPRNG. The internal `Zeroizing` wrapper drops the bytes
    /// deterministically when the value is no longer reachable.
    pub fn random() -> Self {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self {
            bytes: zeroize::Zeroizing::new(bytes),
        }
    }

    /// Wrap a 32-byte array as a `Secret32`. The internal
    /// `Zeroizing` ensures the buffer is wiped on drop.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self {
            bytes: zeroize::Zeroizing::new(bytes),
        }
    }

    /// Borrow the underlying 32 secret bytes. The reference is bound
    /// to the lifetime of `self`; callers must not clone it.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    /// Return an 8-hex-char fingerprint of the secret, suitable for
    /// log redaction (the rest of the key is not derivable from this
    /// value). Used by `Debug` and by restore-tamper tests.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.bytes.as_slice());
        let digest = hasher.finalize();
        hex::encode(&digest[..8])
    }

    /// Explicit zeroize (in addition to the Drop impl).
    pub fn zeroize(&mut self) {
        *self.bytes = [0u8; 32];
    }
}

impl std::fmt::Debug for Secret32 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Secret32(<redacted, fp={}>)", self.fingerprint())
    }
}

/// `Argon2id` parameters per OWASP Password Storage Cheat Sheet (2025).
pub const ARGON2_M_KIB: u32 = 64 * 1024;
/// Argon2id time cost (number of iterations).
pub const ARGON2_T: u32 = 3;
/// Argon2id parallelism factor (lanes).
pub const ARGON2_P: u32 = 1;
/// Argon2id salt length in bytes.
pub const ARGON2_SALT_LEN: usize = 16;
/// Argon2id derived-key length in bytes.
pub const ARGON2_OUTPUT_LEN: usize = 32;

/// Hard deadline for KEK derivation. Production desktops complete in
/// ~1-3s; the 5s ceiling tolerates slower hardware while keeping fail-closed
/// behavior observable. Injected clock makes this testable.
pub const KEK_DERIVATION_DEADLINE: Duration = Duration::from_secs(5);

/// Derive a 32-byte KEK from `password` and `salt` using Argon2id at
/// the module parameters. The 5-second deadline (`KEK_DERIVATION_DEADLINE`)
/// is enforced against `clock`; a slow derivation zeroizes the local
/// output buffer and returns `CryptoError::DerivationTimeout`.
pub fn derive_kek(password: &str, salt: &[u8], clock: &dyn Clock) -> Result<Secret32, CryptoError> {
    let params = Params::new(ARGON2_M_KIB, ARGON2_T, ARGON2_P, Some(ARGON2_OUTPUT_LEN))
        .map_err(|err| CryptoError::DerivationFailed(err.to_string()))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let start = clock.now();
    let mut output = [0u8; ARGON2_OUTPUT_LEN];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut output)
        .map_err(|err| CryptoError::DerivationFailed(err.to_string()))?;
    if clock.now().saturating_duration_since(start) > KEK_DERIVATION_DEADLINE {
        output.zeroize();
        return Err(CryptoError::DerivationTimeout(KEK_DERIVATION_DEADLINE));
    }
    Ok(Secret32::from_bytes(output))
}

/// `kek_verifier`: ChaCha20Poly1305-encrypt the constant `[0u8; 0]` with the
/// KEK as key and a deterministic nonce. A successful decrypt proves the
/// KEK is correct *without revealing* the KEK. This is the public test
/// for "is the entered password right?" before unwrapping the device key.
pub const KEK_VERIFIER_PLAINTEXT: [u8; 0] = [];

/// Wrap the empty `KEK_VERIFIER_PLAINTEXT` with `kek` and return
/// `nonce (12) || ciphertext (16)` = 28 bytes. A subsequent
/// `unwrap_kek_verifier` success proves the holder of `kek` is the
/// correct KEK without revealing it.
pub fn wrap_kek_verifier(kek: &Secret32) -> Result<Vec<u8>, CryptoError> {
    let cipher = ChaCha20Poly1305::new(kek.as_bytes().into());
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), &KEK_VERIFIER_PLAINTEXT[..])
        .map_err(|err| CryptoError::AeadFailed(err.to_string()))?;
    let mut out = Vec::with_capacity(12 + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Verify `kek` against the wrapped verifier `blob`. Returns
/// `CryptoError::KekVerifierMismatch` on AEAD failure (collapses
/// "wrong key" with "tampered blob" so the unlock path does not leak
/// which case occurred).
pub fn unwrap_kek_verifier(kek: &Secret32, blob: &[u8]) -> Result<(), CryptoError> {
    if blob.len() < 12 {
        return Err(CryptoError::InvalidLayout("kek_verifier too short".into()));
    }
    let (nonce, ciphertext) = blob.split_at(12);
    let cipher = ChaCha20Poly1305::new(kek.as_bytes().into());
    let _plaintext = cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| CryptoError::KekVerifierMismatch)?;
    Ok(())
}

/// Wrap a `Secret32` with KEK using AES-256-GCM. Returns
/// `nonce (12) || ciphertext (32 + 16)` = 60 bytes.
pub fn wrap_with_kek(kek: &Secret32, plaintext: &Secret32) -> Result<Vec<u8>, CryptoError> {
    let cipher = ChaCha20Poly1305::new(kek.as_bytes().into());
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes().as_slice())
        .map_err(|err| CryptoError::AeadFailed(err.to_string()))?;
    let mut out = Vec::with_capacity(12 + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Unwrap a previously `wrap_with_kek`'d `Secret32`. The blob layout
/// is `nonce (12) || ciphertext (32) || tag (16)`. Returns
/// `KekVerifierMismatch` on AEAD failure and `InvalidLayout` if the
/// blob is too short or the plaintext length is not 32 bytes.
pub fn unwrap_with_kek(kek: &Secret32, blob: &[u8]) -> Result<Secret32, CryptoError> {
    if blob.len() < 12 + 16 {
        return Err(CryptoError::InvalidLayout("wrap blob too short".into()));
    }
    let (nonce, ciphertext) = blob.split_at(12);
    let cipher = ChaCha20Poly1305::new(kek.as_bytes().into());
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| CryptoError::KekVerifierMismatch)?;
    if plaintext.len() != 32 {
        return Err(CryptoError::InvalidLayout(format!(
            "expected 32 bytes, got {}",
            plaintext.len()
        )));
    }
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&plaintext);
    Ok(Secret32::from_bytes(bytes))
}

/// Deterministic, injectable clock. Production uses
/// `SystemClock::now()`; tests inject `TestClock` for 5s deadline + 15min
/// idle scenarios.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Return the current instant.
    fn now(&self) -> Instant;
}

/// Production clock backed by the real monotonic instant.
pub struct SystemClock;

impl std::fmt::Debug for SystemClock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "SystemClock")
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Injectable test clock whose instant can be advanced manually.
pub struct TestClock {
    inner: parking_lot::Mutex<Instant>,
}

impl std::fmt::Debug for TestClock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TestClock").finish()
    }
}

impl TestClock {
    /// Construct a `TestClock` anchored to the current real instant.
    /// Tests then call `advance` to simulate idle windows.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            inner: parking_lot::Mutex::new(Instant::now()),
        }
    }

    /// Advance the injected clock by `by`. Test-only helper; production
    /// paths use `SystemClock` which cannot be advanced.
    pub fn advance(&self, by: Duration) {
        let mut guard = self.inner.lock();
        *guard = guard.checked_add(by).expect("clock overflow");
    }
}

impl Clock for TestClock {
    fn now(&self) -> Instant {
        *self.inner.lock()
    }
}

/// Slow-clock simulator: every `now()` call advances the clock by
/// 6s to force `DerivationTimeout`.
/// Test clock that advances 6s on every `now()` call, forcing `DerivationTimeout`.
pub struct SlowClock {
    inner: parking_lot::Mutex<Instant>,
}

impl std::fmt::Debug for SlowClock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("SlowClock").finish()
    }
}

impl SlowClock {
    /// Construct a slow clock that reports `now() + 6s` on every call.
    /// Used by tests asserting the 5-second KEK derivation deadline.
    pub fn after_6s() -> Self {
        Self {
            inner: parking_lot::Mutex::new(Instant::now()),
        }
    }
}

impl Clock for SlowClock {
    fn now(&self) -> Instant {
        let mut guard = self.inner.lock();
        *guard = guard
            .checked_add(Duration::from_secs(6))
            .expect("slow clock overflow");
        *guard
    }
}

/// Generate `n` random bytes from the OS RNG.
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n];
    OsRng.fill_bytes(&mut out);
    out
}

/// Deterministic RNG for tests. Not used in production paths.
pub fn seeded_rng(seed: u64) -> rand::rngs::StdRng {
    rand::rngs::StdRng::seed_from_u64(seed)
}
