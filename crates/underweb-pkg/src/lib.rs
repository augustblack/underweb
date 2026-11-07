//! Packages: the `underweb.toml` manifest, `.tar.zst` archives, release
//! indexes, and package addresses.
//!
//! A publisher serves, for each package, a release index at
//! `https://<domain>/.well-known/underweb/<name>.toml` listing versions and,
//! per version, a source archive and prebuilt archives per target triple,
//! each with its BLAKE3 hash. `uw://<domain>/<name>[@version]` refers to it.
//! Signatures and vouches come with the trust phase; until then integrity
//! rests on HTTPS for the index and the hashes for the archives.

mod address;
mod archive;
mod index;
mod manifest;
mod publish;

pub use address::{Address, Parsed, parse_address};
pub use archive::{Limits, collect_files, hash, pack, unpack};
pub use index::{Artifact, Index, Release};
pub use manifest::*;
pub use publish::{
    PublishOptions, index_path, publish, publishable_cargo_toml, source_files, target_files,
};
