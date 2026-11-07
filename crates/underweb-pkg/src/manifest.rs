//! `underweb.toml`, the package manifest.
//!
//! Native packages may also carry sources and a `[build]` section; when no
//! prebuilt library matches the machine, the browser builds one in a
//! sandbox (`kind = "rust"`: the crate in the package directory;
//! `kind = "c"`: the listed sources). The result goes to
//! `dist/<target>/lib<name>.<ext>`.
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
//! [permissions]            # see permissions.rs
//! net.tcp.connect = ["example.org:7000"]
//! ```
//!

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

/// How to build a native package from source.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    /// C sources, relative to the package directory.
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub cflags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub package: PackageInfo,
    #[serde(default)]
    pub targets: BTreeMap<String, Target>,
    #[serde(default)]
    pub build: Build,
    #[serde(default)]
    pub permissions: crate::Permissions,
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
        for f in &self.manifest.build.sources {
            relative(f)?;
        }
        self.manifest.permissions.validate()?;
        if !valid_name(&p.name) {
            bail!(
                "package name {:?}: use lowercase letters, digits, - and _",
                p.name
            );
        }
        semver::Version::parse(&p.version).with_context(|| format!("version {:?}", p.version))?;
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

    pub fn version(&self) -> &str {
        &self.manifest.package.version
    }

    /// The library for this machine: the prebuilt one the manifest lists
    /// for this target, or one built from source at [`Package::built_library`].
    pub fn native_library(&self) -> Result<PathBuf> {
        let triple = host_triple();
        if let Some(t) = self.manifest.targets.get(triple) {
            let p = self.dir.join(&t.file);
            if p.exists() {
                return Ok(p);
            }
        }
        let built = self.built_library();
        if built.exists() {
            return Ok(built);
        }
        bail!(
            "{} has no library for {triple}; it needs to be built from source",
            self.name()
        )
    }

    /// Where a source build for this machine puts the library.
    pub fn built_library(&self) -> PathBuf {
        self.dir
            .join("dist")
            .join(host_triple())
            .join(library_file_name(self.name()))
    }

    /// A native package with no library for this machine yet.
    pub fn needs_build(&self) -> bool {
        self.kind().is_native() && self.native_library().is_err()
    }

    /// Whether the package has what a source build needs.
    pub fn has_source(&self) -> bool {
        match self.kind() {
            Kind::Rust => self.dir.join("Cargo.toml").exists(),
            Kind::C => !self.manifest.build.sources.is_empty(),
            _ => true,
        }
    }
}

/// `lib<name>.dylib` / `lib<name>.so` / `<name>.dll`, with `-` as `_`
/// (what cargo names a cdylib).
pub fn library_file_name(package: &str) -> String {
    let stem = package.replace('-', "_");
    if cfg!(target_os = "macos") {
        format!("lib{stem}.dylib")
    } else if cfg!(windows) {
        format!("{stem}.dll")
    } else {
        format!("lib{stem}.so")
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
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
