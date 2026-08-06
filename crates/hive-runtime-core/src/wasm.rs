//! WASM validation and sandbox configuration contracts.
//!
//! Shared validation covers module shape, declared imports and exports, resource
//! units, WASI denial, and stable execution failures. Filesystem containment,
//! artifact hashing, byte loading, and runtime construction are performed by
//! product adapters before or after this pure boundary as the contract requires.
