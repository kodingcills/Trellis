//! trellis-store — durable metadata (spec §19).
//!
//! SQLite (WAL) persistence for M0–M3 domain objects. Storage is never a
//! second source of truth: canonical domain identities are persisted as
//! authored by trellis-core canonical hashing, never recomputed from
//! storage representations. Attestations are append-only; the repository
//! API exposes no mutation path for artifacts or existing attestations.
//! Publication is blob-first, metadata-second.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]

mod store_impl;

pub use store_impl::{Store, StoreError, SCHEMA_VERSION};

/// Everything a consumer of persistence typically needs.
pub mod prelude {
    pub use crate::store_impl::{Store, StoreError, SCHEMA_VERSION};
}
