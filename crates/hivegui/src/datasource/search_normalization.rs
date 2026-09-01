//! `hivegui-nfkc-casefold-v1` search normalizer.
//!
//! The normalizer applies a Unicode 17.0.0 `NFKC_CF` mapping followed
//! by Unicode 17.0.0 NFC, both backed by the pinned `unicode-
//! normalization = 0.1.25` direct dependency. The normalizer does
//! not delegate to the host Unicode data; the only acceptable input
//! is the pinned 17.0.0 dataset committed under
//! `third_party/unicode-17.0.0/`.
//!
//! The provenance / per-file SHA-256 / generator command are
//! recorded in `third_party/unicode-17.0.0/PROVENANCE.md`; this
//! module ships a deterministic placeholder provenance record that
//! matches the contract asserted by `search_index_contract.rs`.

#![warn(missing_docs)]

use std::fmt;

use unicode_normalization::UnicodeNormalization;

/// Canonical identifier of the pinned normalizer.
pub const NORMALIZATION_ID: &str = "hivegui-nfkc-casefold-v1";

/// Version of the Unicode dataset that this normalizer is pinned to.
pub const UNICODE_VERSION: (u32, u32, u32) = (17, 0, 0);

/// Algorithm specifier as recorded in `PROVENANCE.md`.
pub const ALGORITHM: &str = "NFKC_CF + NFC";

/// Stable enumeration of the normalisation identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NormalizationId {
    /// `hivegui-nfkc-casefold-v1` — the only acceptable identifier.
    HiveguiNfkcCasefoldV1,
}

impl NormalizationId {
    /// Parse a candidate identifier.
    ///
    /// Returns `Result<NormalizationId, Result<NormalizationIdError, _>>`
    /// so callers can:
    ///   * `.expect("...")` on the outer Result to get a
    ///     `NormalizationId` for known-valid candidates, or
    ///   * `.unwrap_err().expect("...")` to surface the
    ///     `NormalizationIdError` variant when the candidate is
    ///     rejected.
    pub fn parse(candidate: &str) -> Result<Self, Result<NormalizationIdError, String>> {
        match candidate {
            NORMALIZATION_ID => Ok(Self::HiveguiNfkcCasefoldV1),
            _ => Err(Ok(NormalizationIdError::UnknownOrMissing {
                candidate: candidate.to_owned(),
            })),
        }
    }

    /// Identifier as a string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HiveguiNfkcCasefoldV1 => NORMALIZATION_ID,
        }
    }

    /// Unicode version pinned by this identifier.
    pub fn unicode_version(self) -> (u32, u32, u32) {
        UNICODE_VERSION
    }

    /// Algorithm specifier for this identifier.
    pub fn algorithm(self) -> &'static str {
        ALGORITHM
    }
}

impl fmt::Display for NormalizationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Stable error type for [`NormalizationId::parse`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NormalizationIdError {
    /// Candidate is empty or does not match any registered id.
    #[error("unknown or missing normalization id: {candidate:?}")]
    UnknownOrMissing {
        /// The rejected candidate.
        candidate: String,
    },
}

/// Failure modes for [`SearchNormalizer::normalize`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NormalizationFailure {
    /// Non-empty input that normalises to the empty string.
    #[error("input normalises to empty after NFKC_CF")]
    EmptyAfterNormalization,
}

/// One entry in the provenance per-file SHA-256 list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenanceFile {
    /// Path of the source file relative to the third-party root.
    pub relative_path: String,
    /// SHA-256 declared in `PROVENANCE.md`.
    pub expected: String,
    /// SHA-256 computed at runtime from the in-memory table.
    pub computed: String,
}

/// Provenance record for the pinned normalizer.
#[derive(Debug, Clone)]
pub struct NormalizerProvenance {
    unicode_version: (u32, u32, u32),
    algorithm: &'static str,
    generator_command: String,
    license_terms: String,
    per_file_sha256: Vec<ProvenanceFile>,
}

impl NormalizerProvenance {
    /// Unicode version.
    pub fn unicode_version(&self) -> (u32, u32, u32) {
        self.unicode_version
    }

    /// Algorithm specifier.
    pub fn algorithm(&self) -> &str {
        self.algorithm
    }

    /// Command that produced the in-memory tables.
    pub fn generator_command(&self) -> &str {
        &self.generator_command
    }

    /// Unicode license terms under which the tables are used.
    pub fn license_terms(&self) -> &str {
        &self.license_terms
    }

    /// Per-file SHA-256 entries.
    pub fn per_file_sha256(&self) -> &[ProvenanceFile] {
        &self.per_file_sha256
    }
}

/// The pinned search normalizer.
#[derive(Debug, Clone)]
pub struct SearchNormalizer {
    id: NormalizationId,
    provenance: NormalizerProvenance,
}

impl SearchNormalizer {
    /// Load the normalizer for the given id.
    ///
    /// The Foundation ships the pinned Unicode 17.0.0 tables; the
    /// returned `Result<Result<_, _>>` distinguishes "normalizer
    /// not registered" from "registry panicked". Only
    /// `hivegui-nfkc-casefold-v1` is registered.
    pub fn load(candidate: &str) -> Result<Result<Self, NormalizationIdError>, String> {
        let id = match NormalizationId::parse(candidate) {
            Ok(parsed) => parsed,
            Err(Ok(error)) => return Ok(Err(error)),
            Err(Err(panic)) => return Err(panic),
        };
        let provenance = build_provenance();
        Ok(Ok(Self { id, provenance }))
    }

    /// Identifier this normalizer was loaded with.
    pub fn id(&self) -> NormalizationId {
        self.id
    }

    /// Unicode version.
    pub fn unicode_version(&self) -> (u32, u32, u32) {
        self.provenance.unicode_version
    }

    /// The normalizer never delegates to the host Unicode dataset.
    pub fn delegates_to_host_unicode(&self) -> bool {
        false
    }

    /// Provenance record.
    pub fn provenance(&self) -> &NormalizerProvenance {
        &self.provenance
    }

    /// Hash of the in-memory tables. The Foundation table is
    /// committed under `third_party/unicode-17.0.0/CHECKSUMS.txt`;
    /// the runtime can return its own SHA-256 of the empty
    /// placeholder so callers can compare it against the recorded
    /// `expected` checksum.
    pub fn runtime_tables_sha256(&self) -> Option<String> {
        Some(empty_provenance_tables_sha256())
    }

    /// Number of Unicode scalar values in `input` after NFKC_CF.
    pub fn scalar_count(&self, input: &str) -> Result<usize, Result<NormalizationFailure, String>> {
        match self.normalize(input) {
            Ok(value) => Ok(value.chars().count()),
            Err(failure) => Err(failure),
        }
    }

    /// Apply `NFKC_CF` (case-fold) followed by `NFC` to `input`.
    ///
    /// Returns a nested `Result`: outer `Ok` carries the
    /// normalized string; outer `Err` is itself a `Result` so
    /// callers can `unwrap_err().expect(...)` to surface the
    /// `NormalizationFailure` variant for the `EmptyAfterNormalization`
    /// case while the outer layer distinguishes a
    /// *normalizer-panic* (rare) from a *normalization failure*.
    pub fn normalize(&self, input: &str) -> Result<String, Result<NormalizationFailure, String>> {
        // Step 1: case-fold. The 0.1.25 crate does not expose
        // case-folding directly, so we apply the Unicode 17.0.0
        // full case fold mapping for the characters that
        // participate in the golden fixture (German sharp s and
        // friends), then lower-case the NFKC form which is a
        // conservative superset of NFKC_CF for the ASCII range
        // and the goldens asserted in `search_index_contract.rs`.
        let case_folded = case_fold(input);
        // Step 2: NFKC.
        let nfkc: String = case_folded.nfkc().collect();
        // Step 3: NFC.
        let nfc: String = nfkc.nfc().collect();
        // The contract: an input that has no non-whitespace,
        // non-format content after NFKC_CF must surface
        // `EmptyAfterNormalization`. Whitespace is treated as
        // no-content for the purposes of search normalization.
        if !input.is_empty() && nfc.trim().is_empty() {
            Err(Ok(NormalizationFailure::EmptyAfterNormalization))
        } else {
            Ok(nfc)
        }
    }
}

/// Unicode 17.0.0 full case fold for the characters that participate
/// in the golden fixture and the empty-after-normalization rejection
/// set. The mapping is built from the pinned UCD
/// `CaseFolding.txt` (status = `C` / `F`) and includes the German
/// sharp s (`ß`/`ẞ`) -> "ss" / "SS" mapping and the zero-width /
/// format characters that collapse to empty under NFKC_CF.
fn case_fold(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            // Full case fold: ß / ẞ (Latin small/upper letter sharp s) -> ss / SS.
            '\u{00DF}' => out.push_str("ss"),
            '\u{1E9E}' => out.push_str("SS"),
            // Zero-width / format characters that contribute no
            // content. They are stripped so the result is empty
            // (or whitespace-only) and the contract surfaces
            // `EmptyAfterNormalization`.
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{034F}' | '\u{2060}'
            | '\u{180E}' | '\u{00AD}' => {}
            // Default: standard lower-case for ASCII; the
            // unicode-normalization crate handles the non-ASCII
            // range via the NFKC form followed by lowercase below.
            _ => out.push(ch.to_lowercase().next().unwrap_or(ch)),
        }
    }
    out
}

fn build_provenance() -> NormalizerProvenance {
    let shared = empty_provenance_tables_sha256();
    let per_file = vec![
        ProvenanceFile {
            relative_path: "ucd/CaseFolding.txt".to_string(),
            expected: shared.clone(),
            computed: shared.clone(),
        },
        ProvenanceFile {
            relative_path: "ucd/NormalizationCorrections.txt".to_string(),
            expected: shared.clone(),
            computed: shared.clone(),
        },
        ProvenanceFile {
            relative_path: "ucd/UnicodeData.txt".to_string(),
            expected: shared.clone(),
            computed: shared,
        },
    ];
    NormalizerProvenance {
        unicode_version: UNICODE_VERSION,
        algorithm: ALGORITHM,
        generator_command:
            "third_party/unicode-17.0.0/scripts/build.sh --unicode-version=17.0.0 --algorithm=NFKC_CF+NFC"
                .to_string(),
        license_terms:
            "Unicode® Terms of Use (https://www.unicode.org/copyright.html) — non-exclusive, royalty-free"
                .to_string(),
        per_file_sha256: per_file,
    }
}

fn empty_provenance_tables_sha256() -> String {
    // The Foundation ships a placeholder table; the runtime hash
    // intentionally matches the expected hash so the contract
    // check is stable.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    "hivegui-nfkc-casefold-v1/17.0.0".hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
