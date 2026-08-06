//! Product-neutral Plugin manifest and execution policy contracts.
//!
//! This module describes validated manifests, exports, resource limits,
//! capability requirements, and cache identity. It does not open artifact paths,
//! persist Plugin records, or construct a product runtime. Hosts must validate
//! artifacts and permissions before supplying controlled WASM bytes for use.
