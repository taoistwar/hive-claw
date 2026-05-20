//! Programmatic facade for running nanobot as a Rust library.
//!
//! Port of the Python `nanobot.nanobot` module. Exposes a small, stable
//! surface so downstream code can spin up an agent without reaching into
//! the internals of `agent`, `providers`, `bus`, and `session`.

pub mod nanobot;

pub use nanobot::{Nanobot, NanobotError, RunResult};
