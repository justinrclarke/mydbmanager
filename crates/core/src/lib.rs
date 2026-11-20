//! # mydb-core
//!
//! Provider-agnostic database management engine shared by the `mydb` CLI and
//! the MyDB Manager desktop app.
//!
//! The crate is organised around a single typed model of a database
//! ([`introspect::SchemaCatalog`]) which powers every higher-level feature:
//!
//! - [`conn`] — connection profiles shared between CLI and desktop app
//! - [`introspect`] — the schema catalog object graph
//! - [`provider`] — the [`DatabaseProvider`] trait implemented per engine
//! - [`query`] — ad-hoc query execution results
//! - [`dump`] — hybrid dump/restore engine (native tools + Rust fallback)
//! - [`transfer`] — environment-to-environment transfer orchestration
//! - [`diff`] — schema comparison and migration SQL generation

#![forbid(unsafe_code)]

pub mod conn;
pub mod diff;
pub mod dump;
pub mod error;
pub mod introspect;
pub mod provider;
pub mod providers;
pub mod query;
pub mod sql;
pub mod ssh;
pub mod transfer;

pub use error::{MyDbError, Result};
pub use provider::{Capabilities, DatabaseProvider, ProviderKind};

/// Version of the core engine, exposed to the desktop UI and CLI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
