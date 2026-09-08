//! trellis-source — deterministic source state (spec §5, §6, §49).
//!
//! Implements **M1 — deterministic source state**: working-tree manifests,
//! content hashing of tracked files, lazy reconciliation of a stored
//! snapshot against the current working tree, and explicitly declared
//! environment fingerprints.
//!
//! Dirtiness is **discovered, not tracked** (spec §5): there is no watcher
//! and no daemon here. `reconcile` is called at query/freeze time and
//! computes the exact changed-file set; it is deterministic and
//! side-effect free.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]

pub mod environment;
pub mod freeze;
pub mod manifest;
pub mod reconcile;

/// Everything a consumer of deterministic source state typically needs.
pub mod prelude {
    pub use crate::environment::EnvironmentFingerprint;
    pub use crate::freeze::freeze;
    pub use crate::manifest::{
        build_manifest, read_tree, Manifest, ManifestEntry, ManifestOptions,
    };
    pub use crate::reconcile::{reconcile, reconcile_against_working_tree, ChangedSet};
}
