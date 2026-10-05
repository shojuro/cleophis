//! Cloud module — the ONLY code in Cleophis allowed to touch the network,
//! with one exception: `inference.rs::healthy()` polling 127.0.0.1.
//! The chat hot path must never route through this module.

pub mod auth;
pub mod commands;
pub mod config;
pub mod download;
pub mod error;
pub mod rest;
/// The platform seam for persisted secrets (refresh token, offline-auth
/// verifiers). Phase 3.1 split these out of `store` because their storage is
/// platform-dependent; `store` itself is plain JSON and needs no `cfg`.
pub mod secure_store;
pub mod session;
pub mod store;
pub mod strength;
pub mod verifier;

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod integration_tests;
