# Provenance

- Upstream package: `aws-sdk-s3 1.141.0`
- Upstream repository: <https://github.com/awslabs/aws-sdk-rust>
- Upstream revision: `edce1e88c8803e14865addfb83b8531c014e7f6d`
- crates.io archive SHA-256: `d9f9420d3a2467eed22ed3635ca653653162c386a0b0f65c78189f9bd3c1379e`
- Local patch: change only the generated manifest dependency from `lru ^0.16.3`
  to `lru 0.18.2`.

The dependency update removes RUSTSEC-2026-0253. The affected API used by the
S3 Express identity cache (`LruCache::new`, `cap`, `len`, and
`get_or_insert_mut`) is unchanged in `lru 0.18.2`. No generated AWS SDK Rust
source is modified.
