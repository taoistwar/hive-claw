//! T017G [P] [security-remediation] Search-index contract for the
//! `hivegui-nfkc-casefold-v1` normalizer + FTS5 trigram + 1-2 char
//! short-gram index. The Foundation phase already has its search
//! surface (T028) under a different normalizer scheme; this contract
//! asserts the security-remediation normalizer behaviour that
//! HiveGUI must adopt and that T022 must build into the schema and
//! migrations.
//!
//! The contract is intentionally scoped to the **public** boundary
//! owned by `hivegui::datasource::search_normalization` plus the
//! `hivegui::datasource::search_index` write/query surface. No
//! implementation exists yet, so the test compiles fail-closed: any
//! attempt to observe behaviour raises an "unimplemented" panic from
//! the future public surface.
//!
//! Behavioural assertions covered by this contract:
//!
//!   - `normalize("Test") != normalize("test")` is *not* true; the
//!     two surface identifiers must remain byte-equal in identifier
//!     uniqueness/look-up (ASCII byte-case-sensitive identity). The
//!     search index, however, must return BOTH rows when the user
//!     searches "test" because it lower-cases for matching while
//!     preserving distinct primary keys.
//!   - The normalizer is `hivegui-nfkc-casefold-v1`. It is built from
//!     `unicode-normalization = 0.1.25` (the only acceptable direct
//!     dependency for NFC) and from a pinned Unicode 17.0.0 UCD
//!     `NFKC_CF` mapping. The contract asserts both
//!     `unicode-normalization` is declared directly with the exact
//!     version/feature set and that the loaded mapping carries
//!     `UNICODE_VERSION = (17, 0, 0)`.
//!   - Provenance + per-file SHA-256 of the generated tables and
//!     the generator command are recorded in
//!     `third_party/unicode-17.0.0/PROVENANCE.md` and must be
//!     checksum-validated. A mismatch is an integrity-gate failure,
//!     not a silent fallback.
//!   - The normalization must reject: non-empty input that
//!     normalizes to empty (`empty_after_normalization`),
//!     missing/unknown normalization ID, and a Unicode version
//!     drift (data set not pinned to 17.0.0). Each failure mode
//!     must be returned through a dedicated stable error so the
//!     migration path can identify the recovery step.
//!   - After normalization, scalar-value count drives index
//!     selection: 3+ scalar values use the FTS5 trigram index, 1
//!     or 2 scalar values use a transactionally maintained
//!     short-gram index. `%`, `_`, quotes, and FTS operators in
//!     the input MUST keep literal semantics.
//!   - Multi-field hits on the same primary key MUST be deduped
//!     by `(entity, primary_key)`.
//!   - Total ordering for every list/search over the fixed
//!     fixture must be: normalized display name asc, normalized
//!     identifier/key asc, numeric primary key asc. Paging must
//!     neither drop nor duplicate rows.
//!   - Entity write + both index updates MUST occur in the same
//!     SQLite transaction. The `EXPLAIN` parser used by
//!     `storage_query_plans.rs` must recognise FTS `VIRTUAL TABLE
//!     INDEX` as an index access path, not a `SCAN`.
//!   - At startup, the running SQLite MUST be probed for FTS5
//!     trigram support; absence MUST fail-closed. There is no
//!     `LIKE`/`SCAN` fallback for the search path.

mod support;

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use hivegui::datasource::{
    search_index::{
        IndexBackend, IndexSelection, MAX_PAGE_SIZE, NormalizationIdError, SearchError, SearchHit,
        SearchIndex, SearchInput, SearchNormalizer, SearchOrdering,
    },
    search_normalization::{NormalizationFailure, NormalizationId, NormalizerProvenance},
};
use serde_json::json;
use support::TestWorkspace;

const NORMALIZATION_ID: &str = "hivegui-nfkc-casefold-v1";
const PROVENANCE_PATH: &str = "third_party/unicode-17.0.0/PROVENANCE.md";
const HIVEGUI_CARGO_TOML: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
const HIVEGUI_NORMALIZATION_SRC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/datasource/search_normalization.rs"
);
const HIVEGUI_SEARCH_INDEX_SRC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/datasource/search_index.rs"
);
const HIVEGUI_STORE_SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/datasource/store.rs");
const HIVEGUI_MIGRATIONS_SRC: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/src/datasource/migrations.rs");

// ---------------------------------------------------------------------------
// §T017G.1 — Normalization ID is fixed and versioned.
// ---------------------------------------------------------------------------

#[test]
fn normalization_id_is_hivegui_nfkc_casefold_v1() {
    let parsed = NormalizationId::parse(NORMALIZATION_ID)
        .expect("hivegui-nfkc-casefold-v1 is the only acceptable normalization id");
    assert_eq!(parsed.as_str(), NORMALIZATION_ID);
    assert_eq!(parsed.unicode_version(), (17, 0, 0));
    assert_eq!(parsed.algorithm(), "NFKC_CF + NFC");
}

#[test]
fn unknown_or_missing_normalization_id_is_rejected_with_stable_error() {
    for invalid in [
        "hivegui-nfkc-casefold-v0",
        "hivegui-nfkc-casefold-v2",
        "",
        "hivegui-nfc-only",
        "hivegui-host-unicode",
    ] {
        let err = NormalizationId::parse(invalid)
            .unwrap_err()
            .expect("invalid normalization id must be rejected");
        assert!(
            matches!(err, NormalizationIdError::UnknownOrMissing { .. }),
            "{invalid:?} must map to UnknownOrMissing, got {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// §T017G.2 — Direct dependency for NFC must be exactly `unicode-normalization = 0.1.25`.
// ---------------------------------------------------------------------------

#[test]
fn hivegui_declares_exact_nfc_direct_dependency_with_default_features_disabled() {
    let cargo_toml = fs::read_to_string(HIVEGUI_CARGO_TOML)
        .unwrap_or_else(|error| panic!("read {}: {error}", HIVEGUI_CARGO_TOML));
    let section = slice_dependency_block(&cargo_toml, "unicode-normalization");
    assert!(
        section.contains("version = \"=0.1.25\""),
        "HiveGUI must pin unicode-normalization to exactly 0.1.25; got {section}"
    );
    assert!(
        section.contains("default-features = false"),
        "HiveGUI must disable default features of unicode-normalization; got {section}"
    );
    assert!(
        section.contains("features = [\"std\"]"),
        "HiveGUI must enable only the `std` feature of unicode-normalization; got {section}"
    );
    assert!(
        !section.contains("features = [\"std\", \"compiled_data\"]"),
        "HiveGUI must not pull compiled_data; the pinned UCD must be the source of truth; got {section}"
    );
}

#[test]
fn normalizer_runtime_loads_unicode_17_0_0_not_host_unicode() {
    let normalizer = SearchNormalizer::load(NORMALIZATION_ID)
        .expect("normalizer loads when pinned to Unicode 17.0.0")
        .expect("normalizer is registered");
    let version = normalizer.unicode_version();
    assert_eq!(
        version,
        (17, 0, 0),
        "the runtime must operate on Unicode 17.0.0, never host Unicode"
    );
    assert!(
        !normalizer.delegates_to_host_unicode(),
        "the normalizer must not delegate to OS/locale Unicode"
    );
}

// ---------------------------------------------------------------------------
// §T017G.3 — Provenance / checksum / generator command recorded.
// ---------------------------------------------------------------------------

#[test]
fn provenance_records_canonical_source_per_file_sha256_and_generator_command() {
    let path = Path::new(PROVENANCE_PATH);
    if !path.exists() {
        // Red gate: the provenance file is not yet created. T022 must
        // generate the tables and the file in a single transaction.
        panic!(
            "missing {PROVENANCE_PATH}; T017G Red gate: the Unicode 17.0.0 provenance file must be created and recorded by T022 before any normalizer can be used"
        );
    }
    let doc = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", PROVENANCE_PATH));
    assert!(doc.contains("unicode_version = \"17.0.0\""));
    assert!(doc.contains("algorithm = \"NFKC_CF + NFC\""));
    assert!(doc.contains("generator_command = "));
    assert!(doc.contains("per_file_sha256:"));
    for required in [
        "ucd/CaseFolding.txt",
        "ucd/NormalizationCorrections.txt",
        "ucd/UnicodeData.txt",
    ] {
        assert!(
            doc.contains(required),
            "PROVENANCE.md must list the canonical UCD file {required}"
        );
    }
}

#[test]
fn pinned_tables_checksum_matches_provenance_entry() {
    let normalizer = SearchNormalizer::load(NORMALIZATION_ID)
        .expect("normalizer loads when pinned to Unicode 17.0.0")
        .expect("normalizer is registered");
    let provenance = normalizer.provenance();
    assert_provenance_complete(provenance);
    let runtime_checksum = normalizer
        .runtime_tables_sha256()
        .expect("runtime can hash its own tables");
    for entry in provenance.per_file_sha256() {
        assert_eq!(
            entry.computed, entry.expected,
            "{} checksum drift; expected {}, got {}",
            entry.relative_path, entry.expected, runtime_checksum
        );
    }
}

// ---------------------------------------------------------------------------
// §T017G.4 — NFKC_CF + NFC golden fixture.
// ---------------------------------------------------------------------------

#[test]
fn golden_fixture_nfkc_cf_plus_nfc_is_byte_stable() {
    let normalizer = SearchNormalizer::load(NORMALIZATION_ID)
        .expect("normalizer loads")
        .expect("normalizer is registered");
    let fixture = golden_fixture();
    for case in fixture.cases() {
        let observed = normalizer
            .normalize(case.input)
            .expect("golden fixture inputs never fail the normalizer");
        assert_eq!(
            observed, case.expected,
            "golden fixture {} drifted; the contract locks the byte-exact NFKC_CF+NFC output",
            case.name
        );
    }
}

#[test]
fn non_empty_input_that_normalizes_to_empty_is_rejected_with_stable_error() {
    let normalizer = SearchNormalizer::load(NORMALIZATION_ID)
        .expect("normalizer loads")
        .expect("normalizer is registered");
    for input in ["\u{200B}", "\u{FEFF}", "\u{034F}", "  \u{200B}  "] {
        let outcome = normalizer.normalize(input).unwrap_err().expect(
            "an input that has no content after NFKC_CF must surface empty_after_normalization",
        );
        assert!(
            matches!(outcome, NormalizationFailure::EmptyAfterNormalization),
            "{input:?} must map to EmptyAfterNormalization, got {outcome:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// §T017G.5 — Identifier look-up is byte-case-sensitive; search is normalized.
// ---------------------------------------------------------------------------

#[test]
fn identifier_lookup_is_byte_case_sensitive_but_search_is_case_insensitive() {
    let workspace = TestWorkspace::new().expect("workspace");
    let index = SearchIndex::open_migrated(workspace.database_path(), workspace.plugin_root())
        .expect("migrate and open search index");
    index
        .seed_for_test(&[("Test", "Test payload"), ("test", "test payload")])
        .expect("seed two distinct identifiers");

    let lookup_upper = index
        .lookup_identifier("Test")
        .expect("exact identifier look-up must succeed");
    let lookup_lower = index
        .lookup_identifier("test")
        .expect("exact identifier look-up must succeed");
    assert_ne!(
        lookup_upper.primary_key(),
        lookup_lower.primary_key(),
        "ASCII byte-case must distinguish the two rows as distinct primary keys"
    );
    assert_ne!(
        lookup_upper.payload(),
        lookup_lower.payload(),
        "ASCII byte-case must distinguish the two rows"
    );
    assert_eq!(lookup_upper.identifier(), "Test");
    assert_eq!(lookup_lower.identifier(), "test");

    let hits: Vec<SearchHit> = index
        .search(SearchInput::new("test").with_field("identifier"))
        .expect("search runs")
        .drain(..)
        .map(|row| row.hit)
        .collect();
    let keys: BTreeSet<_> = hits.iter().map(|hit| hit.primary_key()).collect();
    assert_eq!(
        keys.len(),
        2,
        "search must return BOTH `Test` and `test` (case-insensitive contains)"
    );
}

// ---------------------------------------------------------------------------
// §T017G.6 — FTS5 trigram vs 1-2 char short-gram index selection.
// ---------------------------------------------------------------------------

#[test]
fn index_selection_chooses_trigram_for_three_or_more_scalar_values() {
    let normalizer = SearchNormalizer::load(NORMALIZATION_ID)
        .expect("normalizer loads")
        .expect("normalizer is registered");
    for input in ["hello", "héllo", "abc def", "naïve"] {
        let count = normalizer
            .scalar_count(input)
            .expect("scalar count is defined for non-empty input");
        let selection = IndexSelection::for_input(input, &normalizer)
            .expect("index selection is defined for non-empty input");
        assert!(
            count >= 3,
            "{input:?} must contain at least 3 scalar values for the FTS5 trigram backend; got {count}"
        );
        assert!(
            matches!(selection.backend(), IndexBackend::Fts5Trigram),
            "{input:?} must use FTS5 trigram, got {:?}",
            selection.backend()
        );
    }
}

#[test]
fn index_selection_chooses_short_gram_for_one_or_two_scalar_values() {
    let normalizer = SearchNormalizer::load(NORMALIZATION_ID)
        .expect("normalizer loads")
        .expect("normalizer is registered");
    for input in ["a", "ab", "é", "日本"] {
        let count = normalizer
            .scalar_count(input)
            .expect("scalar count is defined for non-empty input");
        let selection = IndexSelection::for_input(input, &normalizer)
            .expect("index selection is defined for non-empty input");
        assert!(
            (1..=2).contains(&count),
            "{input:?} must be 1-2 scalar values; got {count}"
        );
        assert!(
            matches!(selection.backend(), IndexBackend::ShortGram { length } if (1..=2).contains(&length)),
            "{input:?} must use short-gram, got {:?}",
            selection.backend()
        );
    }
}

#[test]
fn fts_operators_and_sql_wildcards_keep_literal_semantics() {
    let workspace = TestWorkspace::new().expect("workspace");
    let index = SearchIndex::open_migrated(workspace.database_path(), workspace.plugin_root())
        .expect("migrate and open search index");
    index
        .seed_for_test(&[
            ("100% pure", "literal-percent"),
            ("underscore_field", "literal-underscore"),
            ("\"quoted\"", "literal-quote"),
            ("fts*", "fts-asterisk"),
            ("a:b", "fts-colon"),
        ])
        .expect("seed five rows");

    for (raw, expected_identifier) in [
        ("100%", "100% pure"),
        ("_field", "underscore_field"),
        ("\"quoted\"", "\"quoted\""),
        ("fts*", "fts*"),
        ("a:b", "a:b"),
    ] {
        let hits: Vec<SearchHit> = index
            .search(SearchInput::new(raw).with_field("identifier"))
            .expect("search runs")
            .drain(..)
            .map(|row| row.hit)
            .collect();
        let matched: BTreeSet<_> = hits
            .iter()
            .filter(|hit| hit.identifier() == expected_identifier)
            .map(|hit| hit.primary_key())
            .collect();
        assert_eq!(
            matched.len(),
            1,
            "{raw:?} must hit exactly {expected_identifier:?} literally"
        );
    }
}

// ---------------------------------------------------------------------------
// §T017G.7 — Dedup + total ordering + paging.
// ---------------------------------------------------------------------------

#[test]
fn multi_field_hits_on_same_primary_key_are_deduped() {
    let workspace = TestWorkspace::new().expect("workspace");
    let index = SearchIndex::open_migrated(workspace.database_path(), workspace.plugin_root())
        .expect("migrate and open search index");
    index
        .seed_for_test(&[("alpha beta", "row covers both fields")])
        .expect("seed one row whose payload and identifier share tokens");

    let hits: Vec<SearchHit> = index
        .search(
            SearchInput::new("alpha")
                .with_field("identifier")
                .with_field("payload"),
        )
        .expect("search runs")
        .drain(..)
        .map(|row| row.hit)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "matching the same primary key from two fields must dedup"
    );
}

#[test]
fn fixed_fixture_ordering_is_normalized_display_name_then_identifier_then_pk() {
    let workspace = TestWorkspace::new().expect("workspace");
    let index = SearchIndex::open_migrated(workspace.database_path(), workspace.plugin_root())
        .expect("migrate and open search index");
    index
        .seed_for_test(&[
            ("beta-1", "B-1"),
            ("alpha", "a-payload"),
            ("alpha-1", "a-1"),
            ("Beta", "B-uppercase"),
            ("\u{00C1}lpha", "A-acute"),
        ])
        .expect("seed the fixed fixture");

    let pages: Vec<Vec<SearchHit>> = index
        .list_pages(SearchOrdering::TotalOrder, 2)
        .expect("list pages")
        .drain(..)
        .map(|page| page.into_iter().map(|row| row.hit).collect())
        .collect();
    let flat: Vec<(String, String, i64)> = pages
        .iter()
        .flatten()
        .map(|hit| {
            (
                hit.normalized_display_name().to_owned(),
                hit.normalized_identifier().to_owned(),
                hit.primary_key(),
            )
        })
        .collect();
    let mut expected = flat.clone();
    expected.sort();
    assert_eq!(
        flat, expected,
        "pages must be sorted by (display name, identifier, primary key)"
    );
    // Dedupe across pages: primary keys must be unique.
    let pks: BTreeSet<i64> = flat.iter().map(|row| row.2).collect();
    assert_eq!(
        pks.len(),
        flat.len(),
        "paging must not duplicate primary keys"
    );
}

#[test]
fn page_size_is_bounded_by_max_page_size() {
    let workspace = TestWorkspace::new().expect("workspace");
    let index = SearchIndex::open_migrated(workspace.database_path(), workspace.plugin_root())
        .expect("migrate and open search index");
    let err = index
        .list_pages(SearchOrdering::TotalOrder, MAX_PAGE_SIZE + 1)
        .expect_err("page size above MAX_PAGE_SIZE must be rejected");
    assert!(matches!(err, SearchError::PageSizeTooLarge { .. }));
}

// ---------------------------------------------------------------------------
// §T017G.8 — Entity write + index update are one transaction.
// ---------------------------------------------------------------------------

#[test]
fn entity_write_and_index_update_occur_in_one_transaction() {
    let workspace = TestWorkspace::new().expect("workspace");
    let mut index = SearchIndex::open_migrated(workspace.database_path(), workspace.plugin_root())
        .expect("migrate and open search index");
    index
        .begin_write_for_test()
        .expect("begin transaction")
        .expect("transaction ready");
    index
        .upsert_for_test("row-1", "alpha", "alpha payload")
        .expect("write the row");
    // Simulate a crash before commit: the row must NOT be visible to
    // a fresh reader, nor must the FTS5/short-gram row be visible.
    index.rollback_for_test().expect("rollback");

    let hits: Vec<SearchHit> =
        SearchIndex::open_migrated(workspace.database_path(), workspace.plugin_root())
            .expect("reopen index")
            .search(SearchInput::new("alpha").with_field("identifier"))
            .expect("search runs")
            .drain(..)
            .map(|row| row.hit)
            .collect();
    assert!(
        hits.is_empty(),
        "a rolled-back write must leave both row and index empty"
    );
}

// ---------------------------------------------------------------------------
// §T017G.9 — EXPLAIN recognises FTS VIRTUAL TABLE INDEX; fail-closed on FTS5 absence.
// ---------------------------------------------------------------------------

#[test]
fn explain_parser_recognises_fts_virtual_table_index_as_index_access() {
    let plan = vec![
        json!({
            "id": 0,
            "parent": 0,
            "notused": 0,
            "detail": "SCAN CONSTANT ROW"
        }),
        json!({
            "id": 1,
            "parent": 0,
            "notused": 0,
            "detail": "SEARCH search_index USING VIRTUAL TABLE INDEX 1 (term=?)"
        }),
    ];
    let verdict = hivegui::datasource::query_plan::evaluate_fts_plan(
        &SearchIndexPlanExpectation::for_fts_trigram(),
        &plan,
        "2026-07-30",
    )
    .expect("verdict");
    assert!(
        verdict.is_accepted(),
        "FTS VIRTUAL TABLE INDEX must count as index access; got {:?}",
        verdict.failures()
    );
}

#[test]
fn explain_rejects_fts_table_scan_or_like_fallback() {
    let plan = vec![json!({
        "id": 1,
        "parent": 0,
        "notused": 0,
        "detail": "SCAN search_index"
    })];
    let verdict = hivegui::datasource::query_plan::evaluate_fts_plan(
        &SearchIndexPlanExpectation::for_fts_trigram(),
        &plan,
        "2026-07-30",
    )
    .expect("verdict");
    assert!(verdict.has_failure(PlanFailureKind::UnapprovedFullScan));
}

#[test]
fn startup_probes_fts5_trigram_tokenizer_and_fails_closed_when_absent() {
    let workspace = TestWorkspace::new().expect("workspace");
    let outcome = SearchIndex::open(workspace.database_path())
        .expect("open attempt")
        .expect_err("opening on a SQLite without FTS5 trigram must fail-closed");
    assert!(
        matches!(outcome, SearchError::Fts5Unavailable),
        "FTS5 trigram absence must surface Fts5Unavailable, got {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// §T017G.10 — No production LIKE / SCAN / in-memory scan for search.
// ---------------------------------------------------------------------------

#[test]
fn production_search_paths_never_use_like_scan_or_in_memory_scan() {
    for path in [
        HIVEGUI_NORMALIZATION_SRC,
        HIVEGUI_SEARCH_INDEX_SRC,
        HIVEGUI_STORE_SRC,
    ] {
        assert_no_search_regression(path);
    }
}

fn assert_no_search_regression(path: &str) {
    let source = fs::read_to_string(path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    for forbidden in [
        "WHERE identifier LIKE ",
        "WHERE name LIKE ",
        "ILIKE ",
        ".scan_to_list(",
        "in_memory_scan(",
    ] {
        assert!(
            !source.contains(forbidden),
            "{path} contains forbidden search fallback {forbidden:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// §T017G.11 — Migrations own FTS5 + short-gram; runtime Store MUST NOT.
// ---------------------------------------------------------------------------

#[test]
fn runtime_store_never_creates_fts_virtual_table_or_short_gram_index() {
    let source = fs::read_to_string(HIVEGUI_STORE_SRC)
        .unwrap_or_else(|error| panic!("read {HIVEGUI_STORE_SRC}: {error}"));
    for forbidden in [
        "CREATE VIRTUAL TABLE",
        "CREATE TABLE search_index",
        "CREATE INDEX idx_search",
    ] {
        assert!(
            !source.contains(forbidden),
            "{HIVEGUI_STORE_SRC} must not perform DDL for search; DDL belongs to {HIVEGUI_MIGRATIONS_SRC}: {forbidden}"
        );
    }
}

#[test]
fn migrations_creates_fts5_trigram_and_short_gram_indices_with_normalization_id() {
    let source = fs::read_to_string(HIVEGUI_MIGRATIONS_SRC)
        .unwrap_or_else(|error| panic!("read {HIVEGUI_MIGRATIONS_SRC}: {error}"));
    assert!(
        source.contains("CREATE VIRTUAL TABLE search_index USING fts5"),
        "{HIVEGUI_MIGRATIONS_SRC} must own the FTS5 VIRTUAL TABLE; got: {}",
        source
    );
    assert!(
        source.contains("tokenize = \"trigram\""),
        "{HIVEGUI_MIGRATIONS_SRC} must use the FTS5 trigram tokenizer"
    );
    assert!(
        source.contains("search_normalization_id") && source.contains(NORMALIZATION_ID),
        "{HIVEGUI_MIGRATIONS_SRC} must write search_normalization_id = {NORMALIZATION_ID}"
    );
    assert!(
        source.contains("CREATE TABLE short_gram_index"),
        "{HIVEGUI_MIGRATIONS_SRC} must own the short-gram index table"
    );
    assert!(
        source.contains("BEGIN") && source.contains("COMMIT"),
        "migrations must keep the entity + both index writes in one transaction"
    );
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn assert_provenance_complete(provenance: &NormalizerProvenance) {
    assert_eq!(provenance.unicode_version(), (17, 0, 0));
    assert_eq!(provenance.algorithm(), "NFKC_CF + NFC");
    assert!(!provenance.generator_command().trim().is_empty());
    assert!(!provenance.license_terms().trim().is_empty());
    assert!(!provenance.per_file_sha256().is_empty());
}

fn slice_dependency_block(cargo_toml: &str, crate_name: &str) -> String {
    let mut inside = false;
    let mut buf = String::new();
    for line in cargo_toml.lines() {
        if line.starts_with('[') {
            // Match either `[dependencies.{crate_name}]` style
            // blocks or `[dependencies]` (with the crate as an
            // inline key=value sub-entry).
            inside = line.contains(&format!(".{crate_name}]"))
                || (line.starts_with("[dependencies]") && !line.contains('.'));
        } else if inside {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    assert!(
        !buf.trim().is_empty(),
        "HiveGUI Cargo.toml must declare [{crate_name}] (inline or table)"
    );
    buf
}

struct GoldenFixture;

impl GoldenFixture {
    fn cases(&self) -> Vec<GoldenCase> {
        vec![
            GoldenCase::new("ascii-uppercase-folds", "HELLO", "hello"),
            GoldenCase::new("ascii-mixed-case-folds", "Hello", "hello"),
            GoldenCase::new("german-sharp-s-uf04", "STRASSE", "strasse"),
            GoldenCase::new("german-sharp-s-u00df", "Straße", "strasse"),
            GoldenCase::new("compatibility-ff-ufb00", "ﬀ", "ff"),
            GoldenCase::new("compatibility-fligature-ufb03", "ﬃ", "ffi"),
            GoldenCase::new("nfc-pending-jp", "がぎぐげご", "がぎぐげご"),
            GoldenCase::new("nfc-pending-vs-nfd", "à", "à"),
            GoldenCase::new("idempotent-after-roundtrip", "naïve", "naïve"),
        ]
    }
}

struct GoldenCase {
    name: &'static str,
    input: &'static str,
    expected: &'static str,
}

impl GoldenCase {
    const fn new(name: &'static str, input: &'static str, expected: &'static str) -> Self {
        Self {
            name,
            input,
            expected,
        }
    }
}

fn golden_fixture() -> GoldenFixture {
    GoldenFixture
}

struct SearchIndexPlanExpectation;

impl SearchIndexPlanExpectation {
    fn for_fts_trigram() -> hivegui::datasource::query_plan::QueryPlanRequirement {
        hivegui::datasource::query_plan::QueryPlanRequirement {
            query_id: "t017g.search.fts_trigram",
            owner_phase: "security-remediation",
            activation_task: "T017G",
            table: "search_index",
            expected_access: hivegui::datasource::query_plan::AccessExpectation::Search,
            expected_index: Some("search_index VIRTUAL TABLE INDEX"),
            filter_columns: &["term"],
            join_columns: &[],
            scan_exception: None,
        }
    }
}

use hivegui::datasource::query_plan::PlanFailureKind;

// `Path`/`PathBuf` are kept in the imports to allow direct test
// helpers (and to keep the compiler happy once the surface exists).
#[allow(dead_code)]
fn _unused_path_alias(path: &Path) -> PathBuf {
    path.to_path_buf()
}
