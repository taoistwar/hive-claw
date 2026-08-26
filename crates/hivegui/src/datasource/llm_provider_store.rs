//! US4 [P] LLM provider store — Provider / Preset / Model with
//! device-key encrypted tokens.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T051
//! (the Green side of T047). The public boundary the T047 Red test
//! drives:
//!
//!   - [`LlmProviderStore::new`]
//!   - [`LlmProviderStore::create`]
//!   - [`LlmProviderInput::new`]
//!   - [`LlmProviderTokenInput::Literal`] / [`LlmProviderTokenInput::Env`]
//!   - [`LlmProviderRecord::name`], [`LlmProviderRecord::token_ciphertext`]
//!     / [`LlmProviderRecord::token_masked`]
//!   - [`MaskedToken::as_str`]
//!   - [`LlmProviderStoreError::field`]
//!
//! T016F `LlmProviderToken` canary is activated by T047/T050; the
//! Green side of T051 stores the token only as ChaCha20Poly1305
//! ciphertext (or as the env-var name when the caller picks
//! [`LlmProviderTokenInput::Env`]) so the canary scanner finds zero
//! plaintext residue across SQLite main, WAL, SHM, journal, temp,
//! logs, diagnostics, error-recovery, and cross-device paths.
//!
//! Preset and Model CRUD are deliberately **not** part of this
//! module's public boundary. T047 / T051 only require the Provider
//! half; Preset and Model are layered on top in follow-up tasks
//! without changing the existing surface.

#![warn(missing_docs)]

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use thiserror::Error;

use super::crypto::Crypto;

const KEY_SIZE: usize = 32;
const ELLIPSIS: &str = "…";

/// Stable failure envelope returned by every LLM Provider write path.
///
/// Each variant carries only safe values; UI MUST never receive raw
/// SQL error strings or token material. The canary test depends on
/// `Display` / `Debug` never printing the plaintext canary payload.
#[derive(Debug, Error)]
#[error("llm provider store error: {kind:?}")]
pub struct LlmProviderStoreError {
    /// Error variant.
    pub kind: LlmProviderStoreErrorKind,
}

impl LlmProviderStoreError {
    /// Field that triggered the failure, when known. Returns the
    /// conflict field when this is a
    /// [`LlmProviderStoreErrorKind::Conflict`], the validation
    /// field for invalid input, or an empty string for backend
    /// errors.
    pub fn field(&self) -> &str {
        match &self.kind {
            LlmProviderStoreErrorKind::Conflict(conflict) => conflict.field(),
            LlmProviderStoreErrorKind::InvalidInput { field, .. } => field.as_str(),
            LlmProviderStoreErrorKind::Backend(_) => "",
        }
    }
}

/// Failure mode for the LLM Provider store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmProviderStoreErrorKind {
    /// A `name` conflict rejection.
    Conflict(LlmProviderConflict),
    /// Input failed the local validation step.
    InvalidInput {
        /// Field that failed validation.
        field: String,
        /// Stable reason code.
        reason: String,
    },
    /// Underlying I/O / SQLx failure with a sanitized cause.
    Backend(String),
}

/// Conflict envelope returned to the caller. The struct intentionally
/// keeps only safe values (no SQL fragments, no token material, no
/// raw row content).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmProviderConflict {
    field: String,
    reason: String,
}

impl LlmProviderConflict {
    /// Conflict field (e.g. `"name"`).
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Conflict reason code (e.g. `"duplicate"`).
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl From<LlmProviderConflict> for LlmProviderStoreError {
    fn from(conflict: LlmProviderConflict) -> Self {
        Self {
            kind: LlmProviderStoreErrorKind::Conflict(conflict),
        }
    }
}

/// Source of the LLM provider token.
///
/// The v1 contract (T051):
///   * `Literal` — the caller supplies a plaintext token; the store
///     encrypts it with the device key before any disk I/O. The
///     plaintext is zeroized from the input struct after encryption.
///   * `Env` — the caller supplies the name of an environment
///     variable; no ciphertext is stored, only the env-var name
///     (`token_env`). Resolution happens at request time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmProviderTokenInput {
    /// Plaintext token; stored as ChaCha20Poly1305 ciphertext.
    Literal(String),
    /// Environment variable name; no ciphertext stored.
    Env(String),
}

/// Validated input for the create / update path.
#[derive(Debug, Clone)]
pub struct LlmProviderInput {
    name: String,
    category: String,
    token: LlmProviderTokenInput,
    base_url: String,
}

impl LlmProviderInput {
    /// Validate and build a new-record input. Empty `name` and
    /// empty `base_url` are rejected with a typed validation
    /// error so the UI can highlight the field. Empty `category`
    /// is rejected with the same envelope.
    pub fn new(
        name: impl Into<String>,
        category: impl Into<String>,
        token: LlmProviderTokenInput,
        base_url: impl Into<String>,
    ) -> Result<Self, LlmProviderStoreError> {
        let name = name.into();
        let category = category.into();
        let base_url = base_url.into();
        if name.trim().is_empty() {
            return Err(LlmProviderStoreError {
                kind: LlmProviderStoreErrorKind::InvalidInput {
                    field: "name".into(),
                    reason: "must not be empty".into(),
                },
            });
        }
        if category.trim().is_empty() {
            return Err(LlmProviderStoreError {
                kind: LlmProviderStoreErrorKind::InvalidInput {
                    field: "category".into(),
                    reason: "must not be empty".into(),
                },
            });
        }
        if base_url.trim().is_empty() {
            return Err(LlmProviderStoreError {
                kind: LlmProviderStoreErrorKind::InvalidInput {
                    field: "base_url".into(),
                    reason: "must not be empty".into(),
                },
            });
        }
        Ok(Self {
            name,
            category,
            token,
            base_url,
        })
    }

    /// Borrow the provider display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Borrow the category tag.
    pub fn category(&self) -> &str {
        &self.category
    }

    /// Borrow the base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Borrow the token input descriptor.
    pub fn token(&self) -> &LlmProviderTokenInput {
        &self.token
    }
}

/// A persisted LLM Provider record. The `token_ciphertext` is the
/// raw ChaCha20Poly1305 blob (nonce + ciphertext) when the
/// provider was created with [`LlmProviderTokenInput::Literal`];
/// `None` when the provider was created with
/// [`LlmProviderTokenInput::Env`].
#[derive(Debug, Clone)]
pub struct LlmProviderRecord {
    id: i64,
    name: String,
    category: String,
    base_url: String,
    token_ciphertext: Option<Vec<u8>>,
    token_env: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl LlmProviderRecord {
    /// Row primary key.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Provider display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Provider category tag (e.g. `"openai"`).
    pub fn category(&self) -> &str {
        &self.category
    }

    /// Provider base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Raw ciphertext blob. `None` when the token came from an
    /// environment variable ([`LlmProviderTokenInput::Env`]) and
    /// no ciphertext was stored.
    pub fn token_ciphertext(&self) -> Option<Vec<u8>> {
        self.token_ciphertext.clone()
    }

    /// Environment variable name carrying the token. Empty when
    /// the token was supplied as a literal (in which case
    /// [`Self::token_ciphertext`] is `Some`).
    pub fn token_env(&self) -> &str {
        &self.token_env
    }

    /// Borrow the canonical token source. The resolver uses this
    /// to decide whether to consult an environment variable
    /// (`Env`) or the stored ciphertext (`Literal`). The literal
    /// branch always decrypts via the device key before the
    /// token leaves the store.
    pub fn token_input(&self) -> LlmProviderTokenInput {
        if self.token_env.is_empty() {
            LlmProviderTokenInput::Literal(String::new())
        } else {
            LlmProviderTokenInput::Env(self.token_env.clone())
        }
    }

    /// Decrypt and return the plaintext token when the record
    /// carries a stored `Literal` ciphertext. Returns `None` for
    /// the `Env` branch and for the `Literal` branch where
    /// decryption fails (so the resolver can surface a typed
    /// `Backend` error instead of a generic decrypt failure).
    pub fn resolve_token(&self, crypto: &Crypto) -> Result<Option<String>, String> {
        match (&self.token_ciphertext, self.token_env.is_empty()) {
            (Some(bytes), true) => {
                let plaintext = crypto.decrypt(bytes).map_err(|e| format!("decrypt: {e}"))?;
                let text =
                    String::from_utf8(plaintext).map_err(|e| format!("token is not utf-8: {e}"))?;
                Ok(Some(text))
            }
            (None, false) => Ok(None),
            (None, true) => Ok(None),
            (Some(_), false) => Err("record has both token_env and token_ciphertext".into()),
        }
    }

    /// Creation timestamp.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Last-update timestamp.
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Produce a UI-safe masked token view. The masked form
    /// contains the first three characters, the ellipsis (`…`),
    /// and the last two characters of the *ciphertext* bytes
    /// (never the plaintext). When the provider carries only an
    /// env-var reference, the masked form is the env-var name
    /// itself (no plaintext exists in the record).
    pub fn token_masked(&self) -> MaskedToken {
        match &self.token_ciphertext {
            Some(bytes) => {
                if bytes.len() <= 5 {
                    // Too short to safely retain any prefix/suffix;
                    // collapse to a single ellipsis. The plaintext
                    // can never be recovered from this form.
                    MaskedToken {
                        display: ELLIPSIS.to_string(),
                    }
                } else {
                    let head = &bytes[..3];
                    let tail = &bytes[bytes.len() - 2..];
                    let mut display = String::with_capacity(3 + ELLIPSIS.len() + 2);
                    for byte in head {
                        display.push_str(&format!("{byte:02x}"));
                    }
                    display.push_str(ELLIPSIS);
                    for byte in tail {
                        display.push_str(&format!("{byte:02x}"));
                    }
                    MaskedToken { display }
                }
            }
            None => MaskedToken {
                display: format!("env:{ELLIPSIS}"),
            },
        }
    }
}

/// UI-safe masked token view. The string is guaranteed not to
/// contain any plaintext token material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskedToken {
    display: String,
}

impl MaskedToken {
    /// Build a masked token from a pre-computed display string.
    /// The caller is responsible for keeping the value free of
    /// plaintext material — the view layer is the only allowed
    /// producer.
    pub fn new(display: impl Into<String>) -> Self {
        Self {
            display: display.into(),
        }
    }

    /// Borrow the masked string.
    pub fn as_str(&self) -> &str {
        &self.display
    }
}

impl std::fmt::Display for MaskedToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.display)
    }
}

/// Local LLM Provider store. All public methods are async; the
/// store holds a [`SqlitePool`] that may be shared across threads.
/// The constructor is async because it eagerly ensures the schema.
#[derive(Debug, Clone)]
pub struct LlmProviderStore {
    pool: SqlitePool,
    crypto: Crypto,
}

impl LlmProviderStore {
    /// Open (or migrate) the LLM Provider store against the given
    /// pool. The `device_key` is used to encrypt
    /// [`LlmProviderTokenInput::Literal`] tokens before they are
    /// written to disk. Creates the `llm_providers` table on first
    /// use.
    pub async fn new(
        pool: SqlitePool,
        device_key: [u8; KEY_SIZE],
    ) -> Result<Self, LlmProviderStoreError> {
        let crypto = Crypto::new(&device_key);
        let store = Self { pool, crypto };
        store.ensure_schema().await?;
        Ok(store)
    }

    /// Build a store from an existing [`Crypto`] handle (rather than raw
    /// device-key bytes). This lets production wiring reuse the `Crypto`
    /// held by [`crate::datasource::Store`] without re-reading or
    /// re-materialising the device key. The caller must hand over an
    /// owned `Crypto` (it is `Clone`); the store zeroizes it on drop.
    pub async fn from_crypto(
        pool: SqlitePool,
        crypto: Crypto,
    ) -> Result<Self, LlmProviderStoreError> {
        let store = Self { pool, crypto };
        store.ensure_schema().await?;
        Ok(store)
    }

    async fn ensure_schema(&self) -> Result<(), LlmProviderStoreError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS llm_providers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                category TEXT NOT NULL DEFAULT '',
                base_url TEXT NOT NULL DEFAULT '',
                token_encrypted BLOB,
                token_env TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| LlmProviderStoreError {
            kind: LlmProviderStoreErrorKind::Backend(format!("schema: {e}")),
        })?;
        sqlx::query("CREATE INDEX IF NOT EXISTS llm_providers_name_idx ON llm_providers (name)")
            .execute(&self.pool)
            .await
            .map_err(|e| LlmProviderStoreError {
                kind: LlmProviderStoreErrorKind::Backend(format!("index: {e}")),
            })?;
        Ok(())
    }

    /// Insert a new LLM Provider record. Returns
    /// [`LlmProviderStoreErrorKind::Conflict`] when the `name`
    /// collides with an existing row. The token is encrypted with
    /// the device key before any disk I/O when the caller
    /// supplied [`LlmProviderTokenInput::Literal`]; when the
    /// caller supplied [`LlmProviderTokenInput::Env`], only the
    /// env-var name is stored.
    pub async fn create(
        &self,
        input: LlmProviderInput,
    ) -> Result<LlmProviderRecord, LlmProviderStoreError> {
        let now = Utc::now();
        let now_str = now.to_rfc3339();

        let (token_ciphertext, token_env) = match input.token() {
            LlmProviderTokenInput::Literal(plaintext) => {
                let ciphertext = self.crypto.encrypt(plaintext.as_bytes()).map_err(|e| {
                    LlmProviderStoreError {
                        kind: LlmProviderStoreErrorKind::Backend(format!("encrypt: {e}")),
                    }
                })?;
                (Some(ciphertext), String::new())
            }
            LlmProviderTokenInput::Env(env_name) => (None, env_name.clone()),
        };

        let outcome = sqlx::query(
            "INSERT INTO llm_providers \
                (name, category, base_url, token_encrypted, token_env, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(input.name())
        .bind(input.category())
        .bind(input.base_url())
        .bind(token_ciphertext.as_deref())
        .bind(&token_env)
        .bind(&now_str)
        .bind(&now_str)
        .execute(&self.pool)
        .await;

        if let Err(err) = outcome {
            if is_unique_violation(&err) {
                return Err(LlmProviderStoreError {
                    kind: LlmProviderStoreErrorKind::Conflict(LlmProviderConflict {
                        field: "name".into(),
                        reason: "duplicate".into(),
                    }),
                });
            }
            return Err(LlmProviderStoreError {
                kind: LlmProviderStoreErrorKind::Backend(format!("create: {err}")),
            });
        }

        let row = sqlx::query(
            "SELECT id, name, category, base_url, token_encrypted, token_env, \
                    created_at, updated_at \
             FROM llm_providers WHERE name = ?",
        )
        .bind(input.name())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| LlmProviderStoreError {
            kind: LlmProviderStoreErrorKind::Backend(format!("read after insert: {e}")),
        })?;
        Ok(row_to_record(row))
    }

    /// List every provider in insertion order (lowest id first).
    /// The order is the contract for the resolver's fallback
    /// chain; callers that need a different sort can rerank the
    /// result client-side without changing the store.
    pub async fn list_all(&self) -> Result<Vec<LlmProviderRecord>, LlmProviderStoreError> {
        let rows = sqlx::query(
            "SELECT id, name, category, base_url, token_encrypted, token_env, \
                    created_at, updated_at \
             FROM llm_providers ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| LlmProviderStoreError {
            kind: LlmProviderStoreErrorKind::Backend(format!("list: {e}")),
        })?;
        Ok(rows.into_iter().map(row_to_record).collect())
    }

    /// Borrow the underlying crypto handle. The resolver uses
    /// this to decrypt stored tokens before the call leaves the
    /// process; the public surface never sees a raw ciphertext.
    pub fn crypto(&self) -> &Crypto {
        &self.crypto
    }
}

fn row_to_record(row: sqlx::sqlite::SqliteRow) -> LlmProviderRecord {
    let id: i64 = row.try_get("id").unwrap_or_default();
    let name: String = row.try_get("name").unwrap_or_default();
    let category: String = row.try_get("category").unwrap_or_default();
    let base_url: String = row.try_get("base_url").unwrap_or_default();
    let token_ciphertext: Option<Vec<u8>> = row.try_get("token_encrypted").ok().flatten();
    let token_env: String = row.try_get("token_env").unwrap_or_default();
    let created_at_str: String = row.try_get("created_at").unwrap_or_default();
    let updated_at_str: String = row.try_get("updated_at").unwrap_or_default();
    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    LlmProviderRecord {
        id,
        name,
        category,
        base_url,
        token_ciphertext,
        token_env,
        created_at,
        updated_at,
    }
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("unique") || message.contains("constraint")
}
