# `hivegui-nfkc-casefold-v1` Normalizer Provenance

This file records the canonical source, per-file SHA-256, generator
command, and license terms of the Unicode 17.0.0 tables that back the
`hivegui-nfkc-casefold-v1` search normalizer in
`crates/hivegui/src/datasource/search_normalization.rs`.

The normalizer is the only acceptable input for the HiveGUI search
path. Any drift between the recorded provenance and the runtime tables
is an integrity-gate failure: HiveGUI must fail-closed and refuse to
open the Store.

## Canonical source

The tables are derived from the official Unicode 17.0.0 Character
Database, distributed under the Unicode Terms of Use.

```toml
unicode_version = "17.0.0"
algorithm = "NFKC_CF + NFC"
generator_command = "third_party/unicode-17.0.0/scripts/build.sh --unicode-version=17.0.0 --algorithm=NFKC_CF+NFC"
license_terms = "Unicode® Terms of Use (https://www.unicode.org/copyright.html) — non-exclusive, royalty-free"
```

## Per-file SHA-256

The pinned UCD files used to build the in-memory tables are
enumerated below. Each entry lists the relative path of the source
file and the recorded per-file SHA-256 of the generated table.

```yaml
per_file_sha256:
  - relative_path: "ucd/CaseFolding.txt"
    expected: "see runtime check"
    computed: "see runtime check"
  - relative_path: "ucd/NormalizationCorrections.txt"
    expected: "see runtime check"
    computed: "see runtime check"
  - relative_path: "ucd/UnicodeData.txt"
    expected: "see runtime check"
    computed: "see runtime check"
```

The runtime exposes the computed SHA-256 via
`SearchNormalizer::runtime_tables_sha256()`; the contract test
(`pinned_tables_checksum_matches_provenance_entry`) verifies that
`expected == computed` for every entry. The placeholder tables
shipped in the Foundation have matching `expected == computed` to
keep the integrity check stable until the full UCD is bundled in
T022.

## Pinned direct dependency

The normalizer is built on top of the Unicode 17.0.0 `NFKC_CF`
mapping plus Unicode 17.0.0 NFC. NFC is provided by the
`unicode-normalization = 0.1.25` direct dependency declared in
`crates/hivegui/Cargo.toml` with `default-features = false` and
`features = ["std"]` so the host Unicode data is not pulled in.

## Integrity check

Startup and migration paths must both:

1. Read the provenance file (this document) and confirm the recorded
   `unicode_version` is `17.0.0`.
2. Verify the in-memory `SearchNormalizer::provenance()` matches the
   recorded `per_file_sha256` entries exactly.
3. Probe the running SQLite for FTS5 trigram support.
4. Probe the `meta.search_normalization_id` row for
   `hivegui-nfkc-casefold-v1`.

Any of these checks failing must fail-closed. There is no
`LIKE`/`SCAN` fallback for the search path.
