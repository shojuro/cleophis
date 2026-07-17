//! Cloud module — the ONLY code in Cleophis allowed to touch the network,
//! with one exception: `inference.rs::healthy()` polling 127.0.0.1.
//! The chat hot path must never route through this module.
#![allow(dead_code)] // transient scaffolding allowance: removed when commands.rs lands (Task A5)

pub mod config;
pub mod error;
pub mod session;
pub mod store;
