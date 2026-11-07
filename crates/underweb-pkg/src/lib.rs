//! `underweb.toml`, the package manifest.
//!
//! ```toml
//! [package]
//! name = "hello-rust"
//! version = "0.1.0"
//! kind = "rust"            # js | ts | rust | c
//!
//! # native packages: one prebuilt library per target
//! [targets.aarch64-apple-darwin]
//! file = "dist/aarch64-apple-darwin/libhello_rust.dylib"
//!
//! # js/ts packages
//! # entry = "main.ts"
//!
//! [permissions]            # declared now, enforced from phase 3
//! net.tcp.connect = ["example.org:7000"]
//! ```
//!
//! Archives, signatures and remote resolution come in phase 2.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

pub const MANIFEST: &str = "underweb.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Js,
    Ts,
    Rust,
    C,
}

impl Kind {
    pub fn is_native(self) -> bool {
        matches!(self, Kind::Rust | Kind::C)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub kind: Kind,
    /// JS/TS entry module, relative to the package directory.
    pub entry: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// Prebuilt shared library, relative to the package directory.
    pub file: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub package: PackageInfo,
    #[serde(default)]
    pub targets: BTreeMap<String, Target>,
    #[serde(default)]
    pub permissions: toml::Table,
}

/// A package directory with a parsed manifest.
#[derive(Debug, Clone)]
pub struct Package {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Package {
    pub fn open(dir: impl AsRef<Path>) -> Result<Package> {
        let dir = dir
            .as_ref()
            .canonicalize()
            .with_context(|| format!("package {}", dir.as_ref().display()))?;
        let path = dir.join(MANIFEST);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let manifest: Manifest =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let pkg = Package { dir, manifest };
        pkg.validate()?;
        Ok(pkg)
    }

    fn validate(&self) -> Result<()> {
        let p = &self.manifest.package;
        match p.kind {
            Kind::Js | Kind::Ts if p.entry.is_none() => {
                bail!("{} package needs `entry`", self.name())
            }
            _ => {}
        }
        if let Some(e) = &p.entry {
            relative(e)?;
        }
        for t in self.manifest.targets.values() {
            relative(&t.file)?;
        }
        Ok(())
    }

    pub fn name(&self) -> &str {
        &self.manifest.package.name
    }

    pub fn kind(&self) -> Kind {
        self.manifest.package.kind
    }

    /// Absolute path of the JS/TS entry module.
    pub fn entry_path(&self) -> Option<PathBuf> {
        self.manifest
            .package
            .entry
            .as_ref()
            .map(|e| self.dir.join(e))
    }

    /// The prebuilt library for this machine.
    pub fn native_library(&self) -> Result<PathBuf> {
        let triple = host_triple();
        let t = self.manifest.targets.get(triple).with_context(|| {
            format!(
                "{} has no build for {triple} (source builds come in a later version)",
                self.name()
            )
        })?;
        Ok(self.dir.join(&t.file))
    }
}

/// Paths in a manifest must stay inside the package.
fn relative(p: &str) -> Result<()> {
    let path = Path::new(p);
    if path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("path {p:?} must be relative and inside the package");
    }
    Ok(())
}

/// The Rust target triple of this build, used to pick prebuilt libraries.
pub fn host_triple() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        "aarch64-pc-windows-msvc"
    } else {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_rejects_escapes() {
        let ok: Manifest = toml::from_str(
            "[package]\nname='a'\nversion='1'\nkind='ts'\nentry='main.ts'\n[permissions]\nnet.udp.bind=[9000]\n",
        )
        .unwrap();
        assert_eq!(ok.package.kind, Kind::Ts);
        assert!(relative("../x").is_err());
        assert!(relative("/etc/passwd").is_err());
        assert!(relative("dist/lib.dylib").is_ok());
    }
}
