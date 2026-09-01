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
    function_store::{FunctionInput, FunctionKind, FunctionRecord, FunctionStore},
    query_plan::production_query_catalog,
    search_index::{
        IndexBackend, IndexSelection, MAX_PAGE_SIZE, NormalizationIdError, SearchError, SearchHit,
        SearchIndex, SearchInput, SearchNormalizer, SearchOrdering,
    },
    search_normalization::{NormalizationFailure, NormalizationId, NormalizerProvenance},
    store::{Store, StoreOpenOptions},
};
use serde_json::json;
use sqlx::{Pool, Row, Sqlite};
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
const HIVEGUI_FUNCTION_STORE_SRC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/datasource/function_store.rs"
);

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
// 2026-08-20 supplemental T022/T028/T083 Red — exercise the authoritative
// data-model schema and the real v4 Store / EntityFunction path. The historic
// Foundation seam above is deliberately retained as evidence; these tests
// prevent its generic `search_index` model from being mistaken for the
// external-content, cross-entity schema required by data-model.md §9.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn real_v4_store_has_the_exact_external_content_search_schema() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_real_v4_store(&workspace).await;
    let pool = store.pool();
    let mut violations = Vec::new();

    let metadata_columns = column_signature(pool, "schema_metadata")
        .await
        .expect("inspect schema_metadata columns");
    if metadata_columns
        != vec![
            ("key".to_string(), "TEXT".to_string(), 1),
            ("value".to_string(), "TEXT".to_string(), 0),
        ]
    {
        violations.push(format!(
            "schema_metadata columns must be [(key,TEXT,pk=1),(value,TEXT,pk=0)], got {metadata_columns:?}"
        ));
    }
    match sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM schema_metadata \
         WHERE key = 'search_normalization_id' \
           AND value = 'hivegui-nfkc-casefold-v1'",
    )
    .fetch_one(pool)
    .await
    {
        Ok(1) => {}
        Ok(count) => violations.push(format!(
            "schema_metadata must contain exactly one search_normalization_id row, got {count}"
        )),
        Err(error) => violations.push(format!(
            "schema_metadata normalization row is not queryable: {error}"
        )),
    }

    let document_columns = column_signature(pool, "search_documents")
        .await
        .expect("inspect search_documents columns");
    if document_columns
        != vec![
            ("id".to_string(), "INTEGER".to_string(), 1),
            ("entity_type".to_string(), "TEXT".to_string(), 0),
            ("entity_key".to_string(), "TEXT".to_string(), 0),
            ("field".to_string(), "TEXT".to_string(), 0),
            ("normalized_text".to_string(), "TEXT".to_string(), 0),
        ]
    {
        violations.push(format!(
            "search_documents has the wrong columns/order/primary key: {document_columns:?}"
        ));
    }
    for (columns, unique, label) in [
        (
            &["entity_type", "entity_key", "field"][..],
            true,
            "UNIQUE(entity_type, entity_key, field)",
        ),
        (
            &["entity_type", "field", "normalized_text", "entity_key"][..],
            false,
            "covering (entity_type, field, normalized_text, entity_key)",
        ),
    ] {
        match has_index_with_columns(pool, "search_documents", columns, unique).await {
            Ok(true) => {}
            Ok(false) => violations.push(format!("search_documents is missing {label} index")),
            Err(error) => violations.push(format!("inspect search_documents {label}: {error}")),
        }
    }

    let fts_sql = table_sql(pool, "search_documents_fts")
        .await
        .expect("inspect search_documents_fts")
        .unwrap_or_default();
    let compact_fts = compact_sql(&fts_sql);
    for required in [
        "usingfts5(normalized_text",
        "content='search_documents'",
        "content_rowid='id'",
        "tokenize='trigramcase_sensitive1'",
    ] {
        if !compact_fts.contains(required) {
            violations.push(format!(
                "search_documents_fts must be an external-content, case-sensitive trigram table; missing {required:?} in {fts_sql:?}"
            ));
        }
    }

    let short_columns = column_signature(pool, "search_short_grams")
        .await
        .expect("inspect search_short_grams columns");
    if short_columns
        != vec![
            ("document_id".to_string(), "INTEGER".to_string(), 1),
            ("gram_len".to_string(), "INTEGER".to_string(), 2),
            ("gram".to_string(), "TEXT".to_string(), 3),
        ]
    {
        violations.push(format!(
            "search_short_grams must use PRIMARY KEY(document_id, gram_len, gram), got {short_columns:?}"
        ));
    }
    let short_sql = table_sql(pool, "search_short_grams")
        .await
        .expect("inspect search_short_grams")
        .unwrap_or_default();
    let compact_short = compact_sql(&short_sql);
    for required in ["check(gram_lenin(1,2))", "withoutrowid"] {
        if !compact_short.contains(required) {
            violations.push(format!(
                "search_short_grams DDL is missing {required:?}: {short_sql:?}"
            ));
        }
    }
    let foreign_keys = sqlx::query(
        "SELECT \"table\" AS parent_table, \"from\" AS child_column, \
                \"to\" AS parent_column, on_delete \
         FROM pragma_foreign_key_list('search_short_grams')",
    )
    .fetch_all(pool)
    .await
    .expect("inspect search_short_grams foreign keys");
    let has_cascade = foreign_keys.iter().any(|row| {
        row.get::<String, _>("parent_table") == "search_documents"
            && row.get::<String, _>("child_column") == "document_id"
            && row.get::<String, _>("parent_column") == "id"
            && row
                .get::<String, _>("on_delete")
                .eq_ignore_ascii_case("cascade")
    });
    if !has_cascade {
        violations.push(
            "search_short_grams.document_id must FK to search_documents.id ON DELETE CASCADE"
                .to_string(),
        );
    }
    match has_index_with_columns(
        pool,
        "search_short_grams",
        &["gram_len", "gram", "document_id"],
        false,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => violations.push(
            "search_short_grams is missing covering (gram_len, gram, document_id) index"
                .to_string(),
        ),
        Err(error) => violations.push(format!(
            "inspect search_short_grams covering index: {error}"
        )),
    }

    for obsolete in ["search_index", "short_gram_index"] {
        if table_sql(pool, obsolete)
            .await
            .expect("inspect obsolete search table")
            .is_some()
        {
            violations.push(format!(
                "obsolete generic table {obsolete} must not replace the authoritative v4 schema"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "real v4 Store search schema drifted from data-model.md:288,296-297:\n{}",
        violations.join("\n")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn entity_function_crud_and_all_derived_search_rows_are_one_atomic_state() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_real_v4_store(&workspace).await;
    let pool = store.pool();
    let functions = FunctionStore::new(pool.clone()).expect("Function Store");

    let original =
        create_indexed_placeholder(pool, "atomic-function-original", "Original Atomic Function")
            .await;
    assert_function_search_state(pool, &original, "original").await;

    let conflict =
        create_indexed_placeholder(pool, "atomic-function-conflict", "Conflict Function").await;
    let before_failed_update = function_document_snapshot(pool, original.id()).await;
    let failed_update = functions
        .update(
            original.id(),
            placeholder_input(conflict.identifier(), "Must Not Commit"),
        )
        .await;
    assert!(
        failed_update.is_err(),
        "a duplicate identifier must abort the Function transaction"
    );
    assert_eq!(
        functions
            .get(original.id())
            .await
            .expect("reload Function after rejected update")
            .expect("Function survives rejected update")
            .identifier(),
        original.identifier(),
        "the base row must roll back with the derived rows"
    );
    assert_eq!(
        function_document_snapshot(pool, original.id()).await,
        before_failed_update,
        "a rejected base-row update must not partially mutate search_documents/FTS/short grams"
    );

    let updated = functions
        .update(
            original.id(),
            placeholder_input("atomic-function-updated", "Updated Atomic Function"),
        )
        .await
        .expect("update Function and derived search state");
    assert_function_search_state(pool, &updated, "updated").await;
    assert!(
        !fts_function_keys(pool, "original")
            .await
            .contains(&updated.id().to_string()),
        "an update must issue the external-content FTS delete command for old text"
    );

    functions
        .delete(updated.id())
        .await
        .expect("delete Function and derived search state");
    assert!(
        function_document_snapshot(pool, updated.id())
            .await
            .is_empty(),
        "Function delete must remove both search_documents rows"
    );
    let orphan_grams: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM search_short_grams AS grams \
         JOIN search_documents AS documents ON documents.id = grams.document_id \
         WHERE documents.entity_type = 'function' AND documents.entity_key = ?",
    )
    .bind(updated.id().to_string())
    .fetch_one(pool)
    .await
    .expect("count short grams after Function delete");
    assert_eq!(orphan_grams, 0, "Function delete must cascade short grams");
    assert!(
        !fts_function_keys(pool, "updated")
            .await
            .contains(&updated.id().to_string()),
        "Function delete must issue the external-content FTS delete command"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn entity_function_search_is_literal_deduped_total_ordered_and_stably_paged() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_real_v4_store(&workspace).await;
    let pool = store.pool();
    let functions = FunctionStore::new(pool.clone()).expect("Function Store");

    for row in 0..23 {
        let identifier = format!("route-{:02}", 22 - row);
        let name = if row % 2 == 0 {
            format!("ALPHA route {row:02}")
        } else {
            format!("alpha route {row:02}")
        };
        create_indexed_placeholder(pool, &identifier, &name).await;
    }
    create_indexed_placeholder(pool, "short-alpha", "xy beacon").await;
    create_indexed_placeholder(pool, "short-beta", "prefix xy").await;
    create_indexed_placeholder(pool, "literal-percent", "Literal 100% marker").await;
    create_indexed_placeholder(pool, "literal-underscore", "Literal under_score marker").await;

    let all = collect_function_pages(&functions, None).await;
    let normalizer = load_normalizer();
    let expected_for = |needle: &str| {
        let normalized_needle = normalizer
            .normalize(needle)
            .expect("normalize search oracle input");
        let mut rows = all
            .iter()
            .filter(|function| {
                normalizer
                    .normalize(function.name())
                    .expect("normalize Function name")
                    .contains(&normalized_needle)
                    || normalizer
                        .normalize(function.identifier())
                        .expect("normalize Function identifier")
                        .contains(&normalized_needle)
            })
            .collect::<Vec<_>>();
        rows.sort_by_key(|function| {
            (
                normalizer
                    .normalize(function.name())
                    .expect("normalize display name for total order"),
                normalizer
                    .normalize(function.identifier())
                    .expect("normalize identifier for total order"),
                function.id(),
            )
        });
        rows.into_iter()
            .map(|function| function.id())
            .collect::<Vec<_>>()
    };

    let mut violations = Vec::new();
    let route_page_1 = functions
        .list(Some("route".to_string()), 1)
        .await
        .expect("load first FTS Function page");
    let route_page_2 = functions
        .list(Some("route".to_string()), 2)
        .await
        .expect("load second FTS Function page");
    let route_ids = route_page_1
        .items()
        .iter()
        .chain(route_page_2.items())
        .map(|function| function.id())
        .collect::<Vec<_>>();
    let expected_route_ids = expected_for("route");
    if route_ids != expected_route_ids {
        violations.push(format!(
            "3+ scalar FTS results/paging must follow normalized (name,identifier,id) total order; expected {expected_route_ids:?}, got {route_ids:?}"
        ));
    }
    let unique_route_ids = route_ids.iter().copied().collect::<BTreeSet<_>>();
    if unique_route_ids.len() != route_ids.len() {
        violations.push(format!(
            "a Function matching name and identifier must be deduped across pages: {route_ids:?}"
        ));
    }
    let route_count = route_page_1.total();
    if route_count != expected_route_ids.len() as i64 {
        violations.push(format!(
            "Function FTS count must use the same deduped predicate; expected {}, got {route_count}",
            expected_route_ids.len()
        ));
    }

    for needle in ["xy", "%", "_"] {
        let actual = collect_function_pages(&functions, Some(needle.to_string()))
            .await
            .into_iter()
            .map(|function| function.id())
            .collect::<Vec<_>>();
        let expected = expected_for(needle);
        if actual != expected {
            violations.push(format!(
                "{needle:?} must be literal normalized contains search (1-2 scalars use short grams), expected {expected:?}, got {actual:?}"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "real EntityFunction search violated canonical routing/ordering semantics:\n{}",
        violations.join("\n")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn entity_function_search_routes_are_t083_owned_indexed_and_never_like_or_business_scan() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_real_v4_store(&workspace).await;
    let pool = store.pool();
    let mut violations = Vec::new();

    let source = fs::read_to_string(HIVEGUI_FUNCTION_STORE_SRC)
        .unwrap_or_else(|error| panic!("read {HIVEGUI_FUNCTION_STORE_SRC}: {error}"));
    let function_impl = source
        .split_once("impl FunctionStore {")
        .map(|(_, tail)| tail)
        .expect("locate unique FunctionStore production implementation");
    if function_impl.contains(" LIKE ") {
        violations
            .push("FunctionStore::list still contains a business-table LIKE fallback".to_string());
    }

    let owned_tables = production_query_catalog()
        .iter()
        .filter(|query| {
            query.active && query.owner_phase == "US9" && query.activation_task == "T083"
        })
        .map(|query| query.table)
        .collect::<BTreeSet<_>>();
    for required in [
        "functions",
        "search_documents",
        "search_documents_fts",
        "search_short_grams",
    ] {
        if !owned_tables.contains(required) {
            violations.push(format!(
                "production query catalog is missing active US9/T083 ownership for {required}"
            ));
        }
    }

    let plans = [
        (
            "short-gram route",
            "EXPLAIN QUERY PLAN \
             SELECT documents.entity_key \
             FROM search_short_grams AS grams \
             JOIN search_documents AS documents ON documents.id = grams.document_id \
             WHERE grams.gram_len = 2 AND grams.gram = 'xy' \
               AND documents.entity_type = 'function'",
            "grams",
        ),
        (
            "FTS trigram route",
            "EXPLAIN QUERY PLAN \
             SELECT DISTINCT documents.entity_key \
             FROM search_documents_fts \
             JOIN search_documents AS documents \
               ON documents.id = search_documents_fts.rowid \
             WHERE search_documents_fts MATCH '\"route\"' \
               AND documents.entity_type = 'function'",
            "search_documents_fts",
        ),
        (
            "normalized ordered page",
            "EXPLAIN QUERY PLAN \
             SELECT names.entity_key \
             FROM search_documents AS names \
             JOIN search_documents AS identifiers \
               ON identifiers.entity_type = names.entity_type \
              AND identifiers.entity_key = names.entity_key \
              AND identifiers.field = 'identifier' \
             WHERE names.entity_type = 'function' AND names.field = 'name' \
             ORDER BY names.normalized_text, identifiers.normalized_text, \
                      CAST(names.entity_key AS INTEGER) \
             LIMIT 20 OFFSET 0",
            "names",
        ),
    ];
    for (label, sql, expected_plan_subject) in plans {
        match explain_details_result(pool, sql).await {
            Ok(details) => {
                let indexed = details.iter().any(|detail| {
                    detail.contains("USING INDEX")
                        || detail.contains("USING COVERING INDEX")
                        || detail.contains("VIRTUAL TABLE INDEX")
                });
                let business_scan = details
                    .iter()
                    .any(|detail| detail.starts_with("SCAN functions"));
                let expected_table_present = details
                    .iter()
                    .any(|detail| detail.contains(expected_plan_subject));
                if !indexed || business_scan || !expected_table_present {
                    violations.push(format!(
                        "{label} must use indexed {expected_plan_subject} without scanning functions: {details:?}"
                    ));
                }
            }
            Err(error) => violations.push(format!(
                "{label} cannot be explained against the real v4 Store: {error}"
            )),
        }
    }

    assert!(
        violations.is_empty(),
        "EntityFunction search has no approved canonical indexed route:\n{}",
        violations.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

async fn open_real_v4_store(workspace: &TestWorkspace) -> Store {
    Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open real v4 Store")
}

async fn create_indexed_placeholder(
    pool: &Pool<Sqlite>,
    identifier: &str,
    name: &str,
) -> FunctionRecord {
    FunctionStore::new(pool.clone())
        .expect("Function Store")
        .create(placeholder_input(identifier, name))
        .await
        .unwrap_or_else(|error| panic!("create Function fixture {identifier:?}: {error}"))
}

fn placeholder_input(identifier: impl Into<String>, name: impl Into<String>) -> FunctionInput {
    FunctionInput::for_write(
        identifier.into(),
        name.into(),
        None,
        FunctionKind::Placeholder,
        r#"{"type":"object"}"#.to_string(),
        r#"{"type":"object"}"#.to_string(),
        None,
        None,
        None,
        None,
    )
    .expect("validate placeholder Function input")
}

async fn collect_function_pages(
    store: &FunctionStore,
    search: Option<String>,
) -> Vec<FunctionRecord> {
    let mut rows = Vec::new();
    let mut page_number = 1;
    loop {
        let page = store
            .list(search.clone(), page_number)
            .await
            .unwrap_or_else(|error| panic!("list Function page {page_number}: {error}"));
        rows.extend_from_slice(page.items());
        if rows.len() as i64 >= page.total() {
            return rows;
        }
        page_number += 1;
    }
}

fn load_normalizer() -> SearchNormalizer {
    SearchNormalizer::load(NORMALIZATION_ID)
        .expect("load pinned normalizer")
        .expect("normalizer is registered")
}

async fn assert_function_search_state(
    pool: &Pool<Sqlite>,
    function: &FunctionRecord,
    fts_probe: &str,
) {
    let normalizer = load_normalizer();
    let expected_documents = vec![
        (
            function.id().to_string(),
            "identifier".to_string(),
            normalizer
                .normalize(function.identifier())
                .expect("normalize Function identifier"),
        ),
        (
            function.id().to_string(),
            "name".to_string(),
            normalizer
                .normalize(function.name())
                .expect("normalize Function name"),
        ),
    ];
    let actual_documents = function_document_snapshot(pool, function.id()).await;
    assert_eq!(
        actual_documents, expected_documents,
        "Function base row and its two search_documents rows must commit together"
    );

    let expected_grams = expected_short_grams(&expected_documents);
    let actual_grams = sqlx::query(
        "SELECT documents.field, grams.gram_len, grams.gram \
         FROM search_short_grams AS grams \
         JOIN search_documents AS documents ON documents.id = grams.document_id \
         WHERE documents.entity_type = 'function' AND documents.entity_key = ?",
    )
    .bind(function.id().to_string())
    .fetch_all(pool)
    .await
    .expect("load Function short grams")
    .into_iter()
    .map(|row| {
        (
            row.get::<String, _>("field"),
            row.get::<i64, _>("gram_len"),
            row.get::<String, _>("gram"),
        )
    })
    .collect::<BTreeSet<_>>();
    assert_eq!(
        actual_grams, expected_grams,
        "Function write must transactionally replace every distinct 1/2-scalar short gram"
    );
    assert!(
        fts_function_keys(pool, fts_probe)
            .await
            .contains(&function.id().to_string()),
        "Function write must transactionally publish external-content FTS rows"
    );
}

async fn function_document_snapshot(
    pool: &Pool<Sqlite>,
    function_id: i64,
) -> Vec<(String, String, String)> {
    sqlx::query(
        "SELECT entity_key, field, normalized_text \
         FROM search_documents \
         WHERE entity_type = 'function' AND entity_key = ? \
         ORDER BY field",
    )
    .bind(function_id.to_string())
    .fetch_all(pool)
    .await
    .expect("load canonical Function search_documents")
    .into_iter()
    .map(|row| {
        (
            row.get::<String, _>("entity_key"),
            row.get::<String, _>("field"),
            row.get::<String, _>("normalized_text"),
        )
    })
    .collect()
}

fn expected_short_grams(documents: &[(String, String, String)]) -> BTreeSet<(String, i64, String)> {
    let mut grams = BTreeSet::new();
    for (_, field, normalized_text) in documents {
        let scalars = normalized_text.chars().collect::<Vec<_>>();
        for gram_len in [1_usize, 2] {
            for window in scalars.windows(gram_len) {
                grams.insert((
                    field.clone(),
                    gram_len as i64,
                    window.iter().collect::<String>(),
                ));
            }
        }
    }
    grams
}

async fn fts_function_keys(pool: &Pool<Sqlite>, input: &str) -> BTreeSet<String> {
    let normalized = load_normalizer()
        .normalize(input)
        .expect("normalize FTS probe");
    let phrase = format!("\"{}\"", normalized.replace('\"', "\"\""));
    sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT documents.entity_key \
         FROM search_documents_fts \
         JOIN search_documents AS documents ON documents.id = search_documents_fts.rowid \
         WHERE search_documents_fts MATCH ? AND documents.entity_type = 'function' \
         ORDER BY documents.entity_key",
    )
    .bind(phrase)
    .fetch_all(pool)
    .await
    .expect("query canonical Function FTS index")
    .into_iter()
    .collect()
}

async fn table_sql(pool: &Pool<Sqlite>, table: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ? ORDER BY name",
    )
    .bind(table)
    .fetch_optional(pool)
    .await
}

async fn column_signature(
    pool: &Pool<Sqlite>,
    table: &str,
) -> Result<Vec<(String, String, i64)>, sqlx::Error> {
    Ok(
        sqlx::query("SELECT name, type, pk FROM pragma_table_info(?) ORDER BY cid")
            .bind(table)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|row| {
                (
                    row.get::<String, _>("name"),
                    row.get::<String, _>("type").to_ascii_uppercase(),
                    row.get::<i64, _>("pk"),
                )
            })
            .collect(),
    )
}

async fn has_index_with_columns(
    pool: &Pool<Sqlite>,
    table: &str,
    expected_columns: &[&str],
    must_be_unique: bool,
) -> Result<bool, sqlx::Error> {
    let indices =
        sqlx::query("SELECT name, \"unique\" AS is_unique FROM pragma_index_list(?) ORDER BY name")
            .bind(table)
            .fetch_all(pool)
            .await?;
    for index in indices {
        let unique = index.get::<i64, _>("is_unique") != 0;
        if must_be_unique != unique {
            continue;
        }
        let name = index.get::<String, _>("name");
        let columns =
            sqlx::query_scalar::<_, String>("SELECT name FROM pragma_index_info(?) ORDER BY seqno")
                .bind(name)
                .fetch_all(pool)
                .await?;
        if columns
            == expected_columns
                .iter()
                .map(|column| (*column).to_string())
                .collect::<Vec<_>>()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn compact_sql(sql: &str) -> String {
    sql.chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .replace('\"', "'")
}

async fn explain_details_result(
    pool: &Pool<Sqlite>,
    sql: &'static str,
) -> Result<Vec<String>, sqlx::Error> {
    Ok(sqlx::query(sql)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect())
}

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
