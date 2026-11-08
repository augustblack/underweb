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
mod permissions;
mod publish;

pub use address::{Address, Parsed, parse_address};
pub use archive::{Limits, collect_files, hash, pack, unpack};
pub use index::{Artifact, Index, Release};
pub use manifest::*;
pub use permissions::{Permission, Permissions};
pub use publish::{
    PublishOptions, index_path, publish, publishable_cargo_toml, source_files, target_files,
};

/// `path.canonicalize()`, but without Windows' `\\?\` prefix where the
/// plain form means the same: compilers, cargo and file URLs don't take it.
///
/// Inside a Windows AppContainer `canonicalize` fails (it asks the mount
/// manager, which containers may not), so there an existing path is
/// made absolute instead.
pub fn canonical(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let p = match path.canonicalize() {
        Ok(p) => p,
        Err(e) if cfg!(windows) => {
            if !path.try_exists()? {
                return Err(e);
            }
            std::path::absolute(path)?
        }
        Err(e) => return Err(e),
    };
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\")
            && rest.as_bytes().get(1) == Some(&b':')
            && rest.len() < 260
        {
            return Ok(std::path::PathBuf::from(rest));
        }
    }
    Ok(p)
}
