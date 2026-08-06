//! Foundation + security-remediation Red source contract for SQLite/SQLx safety.
//!
//! The Foundation half (T012) is asserted by the static query / offline
//! metadata tests below. The security-remediation half (T017G) extends
//! that surface with the workspace production SQL source inventory:
//! every HiveGUI and HiveWeb production SQL call site is tagged
//! `owner_phase=security-remediation|Foundation|story`, fixed application
//! schema SQL is exclusively built by SQLx `query!|query_as!|query_scalar!`
//! macros / offline metadata, dynamic values are bound via `push_bind`,
//! dynamic identifiers are restricted to the `MysqlIdentifier` allowlist
//! (HiveGUI external MySQL), and the only `AssertSqlSafe` producer is
//! the HiveWeb reviewed `named_queries.toml` central boundary.
//!
//! The actual MySQL metadata allowlist, `MysqlIdentifier` public boundary and
//! MySQL source checks are owned by US2 in `datasource_connection.rs`.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

const OWNER_PHASE: &str = "Foundation";
const APPROVAL_TASK: &str = "T012";
const IMPLEMENTATION_TASK: &str = "T028";
const DATASOURCE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/datasource");
const HIVEGUI_CARGO_TOML: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
const HIVEGUI_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"));
const HIVEWEB_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../hiveweb/src");
const SQLX_METADATA_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.sqlx");
const HIVEGUI_SQLX_FEATURES: &str = "chrono,macros,runtime-tokio,sqlite";
const HIVEWEB_SQLX_FEATURES: &str =
    "chrono,json,macros,mysql,runtime-tokio,rust_decimal,tls-rustls-ring-webpki";
const HIVEGUI_SQLX_FEATURES_EXACT: [&str; 4] = ["chrono", "macros", "runtime-tokio", "sqlite"];
const HIVEWEB_SQLX_FEATURES_EXACT: [&str; 7] = [
    "chrono",
    "json",
    "macros",
    "mysql",
    "runtime-tokio",
    "rust_decimal",
    "tls-rustls-ring-webpki",
];
const HIVEWEB_ASSERT_SQL_SAFE_OWNER: &str = "db/sql_safety.rs";
const NORMALIZATION_ID: &str = "hivegui-nfkc-casefold-v1";

/// Raw user text and static unchecked SQLite SQL have no safety exception.
/// EXPLAIN scan exceptions belong to storage_query_plans and carry
/// table_size/reason/approver/expiry/review_result there.
const ALLOWED_SQL_SAFETY_EXCEPTIONS: [&str; 0] = [];

#[test]
fn static_sqlite_production_queries_use_checked_macros_and_safe_dynamic_binding() {
    assert_eq!(OWNER_PHASE, "Foundation");
    assert_eq!(APPROVAL_TASK, "T012");
    assert_eq!(IMPLEMENTATION_TASK, "T028");
    assert!(ALLOWED_SQL_SAFETY_EXCEPTIONS.is_empty());

    let files = foundation_sqlite_query_files();
    let mut checked_query_count = 0;
    let mut violations = Vec::new();

    for path in files {
        assert!(
            path.is_file(),
            "planned Foundation query owner is missing: {}",
            path.display()
        );
        let source = read(&path);
        checked_query_count += source.matches("query!(").count();
        checked_query_count += source.matches("query_as!(").count();
        checked_query_count += source.matches("query_scalar!(").count();

        for forbidden in [
            "sqlx::query(",
            "sqlx::query_as(",
            "sqlx::query_scalar(",
            "query::<",
            "query_as::<",
            "query_scalar::<",
        ] {
            for line in lines_containing(&source, forbidden) {
                violations.push(format!("{}:{line}", path.display()));
            }
        }

        assert_dynamic_sql_uses_only_bind_and_compiled_identifiers(&path, &source);
    }

    assert!(
        violations.is_empty(),
        "static production SQL bypasses SQLx checked macros:\n{}",
        violations.join("\n")
    );
    assert!(
        checked_query_count > 0,
        "Foundation must contain checked SQLite queries"
    );
}

#[test]
fn sqlx_offline_metadata_is_committed_valid_and_nonempty() {
    let metadata_root = Path::new(SQLX_METADATA_ROOT);
    assert!(
        metadata_root.is_dir(),
        "missing workspace .sqlx directory required by SQLX_OFFLINE=true"
    );

    let mut files = fs::read_dir(metadata_root)
        .expect("read .sqlx directory")
        .map(|entry| entry.expect("SQLx metadata entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("query-") && name.ends_with(".json"))
        })
        .collect::<Vec<_>>();
    files.sort();
    assert!(
        !files.is_empty(),
        "no committed SQLx offline query metadata"
    );

    let mut hashes = BTreeSet::new();
    for path in files {
        let value: serde_json::Value =
            serde_json::from_str(&read(&path)).expect("SQLx metadata is valid JSON");
        let hash = value
            .get("hash")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("{} has no SQLx query hash", path.display()));
        let query = value
            .get("query")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("{} has no SQL text", path.display()));
        assert!(hashes.insert(hash.to_owned()), "duplicate SQLx hash {hash}");
        assert!(!query.trim().is_empty());
        assert!(
            value.get("describe").is_some(),
            "{} has no describe data",
            path.display()
        );
    }
}

fn foundation_sqlite_query_files() -> Vec<PathBuf> {
    let root = Path::new(DATASOURCE_ROOT);
    // T028 may close only Foundation-owned SQLite paths. Entity and LLM query
    // files are activated by their user-story reviewers and must not be pulled
    // into the Foundation Green gate merely because they already exist.
    ["store.rs", "migrations.rs", "plugin_artifacts.rs"]
        .into_iter()
        .map(|name| root.join(name))
        .collect()
}

fn assert_dynamic_sql_uses_only_bind_and_compiled_identifiers(path: &Path, source: &str) {
    for forbidden in [
        "push_str(",
        ".push(user_",
        ".push(input",
        ".push(filter",
        ".push(order_by",
        "format!(\"SELECT",
        "format!(\"INSERT",
        "format!(\"UPDATE",
        "format!(\"DELETE",
    ] {
        assert!(
            !source.contains(forbidden),
            "{} contains dynamic SQL path {forbidden:?}; values require push_bind and identifiers require a compile-time allowlist",
            path.display()
        );
    }

    if source.contains("QueryBuilder") {
        assert!(
            source.contains("push_bind("),
            "{} uses QueryBuilder without value binding",
            path.display()
        );
    }
}

fn lines_containing(source: &str, needle: &str) -> Vec<String> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(needle))
        .map(|(index, line)| format!("{}: {}", index + 1, line.trim()))
        .collect()
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path.as_ref())
        .unwrap_or_else(|error| panic!("read {}: {error}", path.as_ref().display()))
}

// ---------------------------------------------------------------------------
// §T017G.1 — HiveGUI SQLx feature set is exact (no `derive`, no native-TLS).
// ---------------------------------------------------------------------------

#[test]
fn hivegui_sqlx_features_match_the_security_remediation_inventory_exactly() {
    let cargo_toml = read(HIVEGUI_CARGO_TOML);
    let section = slice_dependency_block(&cargo_toml, "sqlx");
    let actual = parse_feature_list(&section, "sqlx");
    assert_eq!(
        actual,
        HIVEGUI_SQLX_FEATURES_EXACT
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        "HiveGUI must enable exactly {HIVEGUI_SQLX_FEATURES:?}; got {actual:?}"
    );
    assert!(
        !actual.iter().any(|feature| feature == "derive"),
        "HiveGUI must not enable sqlx/derive because macros/derive is redundant and the macros feature is the contract owner"
    );
    assert!(
        !actual
            .iter()
            .any(|feature| feature == "native-tls" || feature == "tls-rustls"),
        "HiveGUI's SQLx talks only to SQLite; HiveGUI external MySQL goes through mysql_async"
    );
}

#[test]
fn hiveweb_sqlx_features_match_the_security_remediation_inventory_exactly() {
    let hiveweb_cargo = Path::new(HIVEGUI_ROOT).join("../hiveweb/Cargo.toml");
    if !hiveweb_cargo.exists() {
        panic!(
            "missing {}; the HiveWeb SQLx feature contract requires the workspace hiveweb crate to exist before T017G is reviewed",
            hiveweb_cargo.display()
        );
    }
    let cargo_toml = read(&hiveweb_cargo);
    let section = slice_dependency_block(&cargo_toml, "sqlx");
    let actual = parse_feature_list(&section, "sqlx");
    assert_eq!(
        actual,
        HIVEWEB_SQLX_FEATURES_EXACT
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        "HiveWeb must enable exactly {HIVEWEB_SQLX_FEATURES:?}; got {actual:?}"
    );
    assert!(
        !actual.iter().any(|feature| feature == "derive"),
        "HiveWeb must not enable sqlx/derive because macros/derive is redundant"
    );
}

// ---------------------------------------------------------------------------
// §T017G.2 — Workspace production SQL source inventory is total and tagged.
// ---------------------------------------------------------------------------

#[test]
fn every_hivegui_production_sql_call_is_tagged_owner_phase() {
    let inventory =
        hivegui::datasource::sql_source_inventory::load_for_test(HIVEGUI_ROOT)
            .expect("the workspace production SQL source inventory is exposed by the T017G security boundary")
            .expect("the inventory must enumerate every production SQL call site in HiveGUI");

    let mut by_file: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for entry in inventory.entries() {
        assert!(
            matches!(
                entry.owner_phase(),
                hivegui::datasource::sql_source_inventory::OwnerPhase::SecurityRemediation
                    | hivegui::datasource::sql_source_inventory::OwnerPhase::Foundation
                    | hivegui::datasource::sql_source_inventory::OwnerPhase::Story { .. }
            ),
            "{} has an unknown owner_phase tag",
            entry.relative_path()
        );
        // Each entry must point at a real line in the real source file.
        let path = entry.relative_path().to_string();
        let file_source = by_file.entry(path.clone()).or_insert_with(|| {
            let absolute = std::path::Path::new(HIVEGUI_ROOT).join(&path);
            std::fs::read_to_string(&absolute)
                .unwrap_or_else(|error| panic!("read {path}: {error}"))
        });
        assert!(
            file_source.lines().nth(entry.line_number() - 1).is_some(),
            "{} references line {} but the file is shorter",
            entry.relative_path(),
            entry.line_number()
        );
    }
}

#[test]
fn production_query_builder_call_count_is_exactly_zero() {
    let inventory = hivegui::datasource::sql_source_inventory::load_for_test(HIVEGUI_ROOT)
        .expect("inventory loader present")
        .expect("inventory loads");
    let query_builder_calls = inventory
        .entries()
        .iter()
        .filter(|entry| entry.is_query_builder())
        .count();
    assert_eq!(
        query_builder_calls, 0,
        "production code must not use SQLx QueryBuilder; bind via push_bind on a fixed schema SQL"
    );
}

#[test]
fn production_assert_sql_safe_owner_is_exactly_hiveweb_db_sql_safety() {
    let inventory = hivegui::datasource::sql_source_inventory::load_for_test(HIVEGUI_ROOT)
        .expect("inventory loader present")
        .expect("inventory loads");
    // Production-only AssertSqlSafe audit: the inventory also lists
    // AssertSqlSafe in test files (which is fine — tests need to
    // construct the marker too). The contract is that the single
    // production-side owner is the HiveWeb central boundary.
    let producers: Vec<_> = inventory
        .entries()
        .iter()
        .filter(|entry| entry.constructs_assert_sql_safe())
        .filter(|entry| {
            let path = entry.relative_path();
            // Production code lives under `src/`. Anything under
            // `tests/` (top-level or nested) is a test file and
            // excluded from the production AssertSqlSafe audit.
            !path.starts_with("tests/")
                && !path.contains("/tests/")
                && !path.starts_with("test_")
                && !path.contains("/test_")
        })
        .collect();
    assert_eq!(
        producers.len(),
        1,
        "exactly one HiveWeb central boundary may construct AssertSqlSafe; got {producers:?}"
    );
    let producer = producers[0];
    assert_eq!(
        producer.relative_path(),
        HIVEWEB_ASSERT_SQL_SAFE_OWNER,
        "the only AssertSqlSafe owner is {}; got {}",
        HIVEWEB_ASSERT_SQL_SAFE_OWNER,
        producer.relative_path()
    );
    assert_eq!(
        producer.owner_phase(),
        hivegui::datasource::sql_source_inventory::OwnerPhase::SecurityRemediation
    );
}

#[test]
fn production_mysql_identifier_construction_only_through_allowlist() {
    let inventory = hivegui::datasource::sql_source_inventory::load_for_test(HIVEGUI_ROOT)
        .expect("inventory loader present")
        .expect("inventory loads");
    for entry in inventory.entries() {
        if entry.uses_mysql_identifier() {
            assert!(
                entry.relative_path().contains("/datasource/"),
                "{}: HiveGUI external MySQL identifiers must live in /datasource/ and be built via MysqlIdentifier::from_allowlist",
                entry.relative_path()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// §T017G.3 — The search-normalization table is not an SQL escape hatch.
// ---------------------------------------------------------------------------

#[test]
fn search_normalization_id_is_pinned_in_every_fts5_and_short_gram_ddl() {
    let source = read(Path::new(DATASOURCE_ROOT).join("migrations.rs"));
    let occurrences = source.matches(NORMALIZATION_ID).count();
    assert!(
        occurrences >= 2,
        "every FTS5 + short-gram DDL statement must reference {NORMALIZATION_ID}; got {occurrences} occurrences"
    );
}

// ---------------------------------------------------------------------------
// §T017G helpers — Cargo.toml feature parsing.
// ---------------------------------------------------------------------------

fn slice_dependency_block(cargo_toml: &str, crate_name: &str) -> String {
    let mut buf = String::new();
    let mut in_target = false;
    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            // Match either `[dependencies.sqlx]` style blocks or `[workspace.dependencies]`
            // / `[dependencies]` / `[dev-dependencies]` / `[build-dependencies]` blocks
            // that contain a `sqlx = { ... }` sub-entry.
            in_target = if trimmed.starts_with("[dependencies.")
                || trimmed.starts_with("[dev-dependencies.")
            {
                trimmed.contains(&format!(".{crate_name}]"))
            } else if trimmed.starts_with("[dependencies]")
                || trimmed.starts_with("[dev-dependencies]")
                || trimmed.starts_with("[build-dependencies]")
                || trimmed.starts_with("[workspace.dependencies]")
            {
                true
            } else {
                false
            };
            if in_target {
                buf.push_str(line);
                buf.push('\n');
            }
            continue;
        }
        if in_target {
            // If we leave the dependencies block and enter a different
            // section, we still need to keep going inside the same
            // `dependencies` block (a single `[dependencies]` block
            // spans until the next `[foo]`).
            buf.push_str(line);
            buf.push('\n');
        }
    }
    assert!(
        !buf.trim().is_empty(),
        "Cargo.toml must declare a [{crate_name}] block (either as a workspace dependency or per-crate dependency)"
    );
    buf
}

fn parse_feature_list(cargo_section: &str, crate_name: &str) -> Vec<String> {
    // Scope parsing to the `crate_name = { ... }` declaration, then
    // extract the `features = [...]` array from within that body.
    // The previous line-prefix parser either grabbed every
    // `features = [...]` in the section (too greedy when other
    // dependencies also have inline features) or missed the
    // inline form entirely.
    let mut features = BTreeSet::new();
    let Some(block) = extract_inline_table_for(cargo_section, crate_name) else {
        return Vec::new();
    };
    if let Some(array) = extract_features_array(&block) {
        for token in array.split(',') {
            let cleaned = token
                .trim()
                .trim_start_matches('"')
                .trim_end_matches('"')
                .trim_start_matches('\'')
                .trim_end_matches('\'');
            if !cleaned.is_empty() {
                features.insert(cleaned.to_owned());
            }
        }
    }
    features.into_iter().collect()
}

/// Return the `features = [...]` array body (without the brackets)
/// from the body of a `crate_name = { ... }` declaration. Handles
/// the multi-line form `features = [\n "a",\n "b",\n ]`.
fn extract_features_array(table_body: &str) -> Option<String> {
    let bytes = table_body.as_bytes();
    let needle = b"features";
    let equal = b'=';
    let open_bracket = b'[';
    let mut i = 0;
    while i < bytes.len() {
        if let Some(rel) = find_token(table_body, i, needle) {
            let mut j = rel + needle.len();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == equal {
                j += 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == open_bracket {
                    let array_start = j + 1;
                    if let Some(close_rel) = find_matching_bracket(table_body, array_start) {
                        let close = array_start + close_rel;
                        return Some(table_body[array_start..close].to_string());
                    }
                }
            }
            i = rel + needle.len();
        } else {
            break;
        }
    }
    None
}

/// Return the body of the `crate_name = { ... }` inline table if
/// one appears in the section. Handles both the single-line form
/// (`crate_name = { features = ["a"] }`) and the multi-line form
/// (`crate_name = { features = [\n "a",\n ] }`).
fn extract_inline_table_for(cargo_section: &str, crate_name: &str) -> Option<String> {
    let bytes = cargo_section.as_bytes();
    let needle = crate_name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Find whole-word occurrence of `crate_name`.
        if let Some(rel) = find_token(cargo_section, i, needle) {
            let after = rel + needle.len();
            // Skip whitespace.
            let mut j = after;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            // Expect `=`.
            if j < bytes.len() && bytes[j] == b'=' {
                j += 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                // Expect `{`.
                if j < bytes.len() && bytes[j] == b'{' {
                    let open = j;
                    if let Some(close_rel) = find_matching_inline_table_brace(cargo_section, open) {
                        let close = open + close_rel;
                        return Some(cargo_section[open + 1..close].to_string());
                    }
                }
            }
            i = after;
        } else {
            break;
        }
    }
    None
}

fn find_matching_inline_table_brace(haystack: &str, from: usize) -> Option<usize> {
    // `from` is the index of the opening `{`. Return the index of
    // the matching `}`.
    let bytes = haystack.as_bytes();
    let mut depth: i32 = 1;
    let mut in_string: Option<u8> = None;
    let mut i = from + 1;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(quote) = in_string {
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if c == quote {
                in_string = None;
            }
        } else if c == b'"' || c == b'\'' {
            in_string = Some(c);
        } else if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(i - from);
            }
        }
        i += 1;
    }
    None
}

fn find_token(haystack: &str, from: usize, needle: &[u8]) -> Option<usize> {
    let bytes = haystack.as_bytes();
    if from >= bytes.len() || needle.is_empty() {
        return None;
    }
    let mut i = from;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            // Must be a whole word: not preceded or followed by an
            // identifier character.
            let prev_ok = i == 0
                || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            let after_idx = i + needle.len();
            let next_ok = after_idx >= bytes.len()
                || !(bytes[after_idx].is_ascii_alphanumeric()
                    || bytes[after_idx] == b'_');
            if prev_ok && next_ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn find_matching_bracket(haystack: &str, from: usize) -> Option<usize> {
    // Find the `]` that closes the `[` opened at `from - 1`.
    // Skip string literals to avoid being fooled by `]` inside
    // them. Track nesting in case of nested arrays.
    let bytes = haystack.as_bytes();
    let mut depth: i32 = 1;
    let mut in_string: Option<u8> = None;
    let mut i = from;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(quote) = in_string {
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if c == quote {
                in_string = None;
            }
        } else if c == b'"' || c == b'\'' {
            in_string = Some(c);
        } else if c == b'[' {
            depth += 1;
        } else if c == b']' {
            depth -= 1;
            if depth == 0 {
                return Some(i - from);
            }
        }
        i += 1;
    }
    None
}
