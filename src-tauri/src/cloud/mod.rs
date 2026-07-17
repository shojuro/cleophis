//! Cloud module — the ONLY code in Cleophis allowed to touch the network,
//! with one exception: `inference.rs::healthy()` polling 127.0.0.1.
//! The chat hot path must never route through this module.

pub mod auth;
pub mod commands;
pub mod config;
pub mod error;
pub mod rest;
pub mod session;
pub mod store;

#[cfg(test)]
pub(crate) mod test_support;
