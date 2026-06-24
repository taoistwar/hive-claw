//! Per-`POST /v1/responses` size limits defined by `contracts/openresponses-v1.md`
//! and FR-003a.

/// Maximum decoded byte length of a single `input_file` attachment (1 MiB).
pub const PER_FILE_MAX_BYTES: usize = 1024 * 1024;

/// Maximum sum of decoded byte lengths across all attachments on one
/// request (both `input_file` and `input_image` count).
pub const TOTAL_ATTACHMENTS_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Maximum raw HTTP body size accepted by the axum extractor. Sized to
/// fit the 4 MiB attachment budget after base64 expansion (~1.33×) plus
/// the JSON envelope overhead.
pub const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
