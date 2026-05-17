//! Library surface for the `animus-provider-claude` plugin.
//!
//! The binary entrypoint lives in `src/main.rs`. The modules below are
//! exposed so integration tests (and downstream embedders that want to wire
//! the Claude backend without spawning a subprocess) can reach the
//! `ProviderBackend` implementation directly.

pub mod backend;
pub mod config;
