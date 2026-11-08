//! Turning a package directory into archives and a release index.
//!
//! The output is a static site: serve `repo/` at `https://<domain>/` and
//! the package is at `uw://<domain>/<name>`.
//!
//! ```text
//! repo/.well-known/underweb/<name>.toml                       release index
//! repo/.well-known/underweb/<name>/<ver>/<name>-<ver>-src.tar.zst
//! repo/.well-known/underweb/<name>/<ver>/<name>-<ver>-<target>.tar.zst
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use toml_edit::{DocumentMut, Item};

use crate::{Artifact, Index, Kind, Package, Release, collect_files, hash, host_triple, pack};

/// Directories never shipped in a source archive.
const SKIP_DIRS: &[&str] = &["dist", "target"];

#[derive(Debug, Clone, Copy)]
pub struct PublishOptions {
    /// Include a source archive (always, for JS/TS).
    pub source: bool,
    /// Include a prebuilt archive for this machine (native packages).
    pub target: bool,
}

impl Default for PublishOptions {
    fn default() -> Self {
        PublishOptions {
            source: true,
            target: true,
        }
    }
}

pub fn index_path(repo: &Path, name: &str) -> PathBuf {
    repo.join(".well-known/underweb")
        .join(format!("{name}.toml"))
}

/// Write the package's archives into `repo` and add them to its index.
/// Returns the files written.
pub fn publish(pkg: &Package, repo: &Path, opts: PublishOptions) -> Result<Vec<PathBuf>> {
    let (name, version) = (pkg.name(), pkg.version());
    let rel_dir = format!("{name}/{version}");
    let out_dir = repo.join(".well-known/underweb").join(&rel_dir);
    std::fs::create_dir_all(&out_dir)?;
    let mut written = Vec::new();
    let mut release = Release {
        version: version.to_string(),
        source: None,
        targets: Default::default(),
        statement: None,
    };

    let mut write = |file: String, files: Vec<(PathBuf, Vec<u8>)>| -> Result<Artifact> {
        let bytes = pack(&files)?;
        let path = out_dir.join(&file);
        std::fs::write(&path, &bytes)?;
        written.push(path);
        Ok(Artifact {
            url: format!("{rel_dir}/{file}"),
            blake3: hash(&bytes),
            size: bytes.len() as u64,
        })
    };

    if opts.source || !pkg.kind().is_native() {
        release.source = Some(write(
            format!("{name}-{version}-src.tar.zst"),
            source_files(pkg)?,
        )?);
    }
    if opts.target && pkg.kind().is_native() {
        let triple = host_triple();
        release.targets.insert(
            triple.into(),
            write(
                format!("{name}-{version}-{triple}.tar.zst"),
                target_files(pkg)?,
            )?,
        );
    }

    let index_file = index_path(repo, name);
    let mut index = match std::fs::read_to_string(&index_file) {
        Ok(text) => Index::parse(&text)?,
        Err(_) => Index::new(name),
    };
    if index.name != name {
        bail!("{} is the index of {:?}", index_file.display(), index.name);
    }
    index.add(release);
    std::fs::write(&index_file, index.to_toml())?;
    written.push(index_file);
    Ok(written)
}

/// Everything needed to run (JS/TS) or build (native) the package.
pub fn source_files(pkg: &Package) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let mut files = collect_files(&pkg.dir, SKIP_DIRS)?;
    if pkg.kind() == Kind::Rust {
        let cargo = files
            .iter_mut()
            .find(|(p, _)| p == Path::new("Cargo.toml"))
            .context("Rust package without Cargo.toml")?;
        cargo.1 = publishable_cargo_toml(std::str::from_utf8(&cargo.1)?)?.into_bytes();
        if !files.iter().any(|(p, _)| p == Path::new("Cargo.lock")) {
            // a workspace member: ship the workspace's lockfile so the
            // build uses the same dependency versions
            if let Some(lock) = pkg
                .dir
                .ancestors()
                .skip(1)
                .take(4)
                .map(|d| d.join("Cargo.lock"))
                .find(|p| p.exists())
            {
                files.push(("Cargo.lock".into(), std::fs::read(lock)?));
            }
        }
    }
    Ok(files)
}

/// The manifest plus the library for this machine.
pub fn target_files(pkg: &Package) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let lib = pkg.native_library()?;
    let rel = lib
        .strip_prefix(&pkg.dir)
        .context("library outside the package")?
        .to_path_buf();
    Ok(vec![
        (
            "underweb.toml".into(),
            std::fs::read(pkg.dir.join("underweb.toml"))?,
        ),
        (rel, std::fs::read(&lib)?),
    ])
}

/// Like `cargo publish`: dependencies given as `{ path, version }` keep
/// only the version (the browser supplies the SDK), and inherited
/// workspace values are refused.
pub fn publishable_cargo_toml(text: &str) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parsing Cargo.toml")?;
    if text.contains(".workspace")
        || text.contains("workspace = true")
        || text.contains("workspace=true")
    {
        bail!("Cargo.toml inherits from a workspace; write the values out to publish it");
    }
    let mut tables: Vec<&mut Item> = Vec::new();
    let (deps, rest): (Vec<_>, Vec<_>) = doc.iter_mut().partition(|(k, _)| {
        matches!(
            k.get(),
            "dependencies" | "build-dependencies" | "dev-dependencies"
        )
    });
    tables.extend(deps.into_iter().map(|(_, v)| v));
    // [target.'cfg(...)'.dependencies] and friends
    for (_, targets) in rest.into_iter().filter(|(k, _)| k.get() == "target") {
        let Some(targets) = targets.as_table_like_mut() else {
            continue;
        };
        for (_, cfg) in targets.iter_mut() {
            let Some(cfg) = cfg.as_table_like_mut() else {
                continue;
            };
            for (k, deps) in cfg.iter_mut() {
                if k.get().ends_with("dependencies") {
                    tables.push(deps);
                }
            }
        }
    }
    for deps in tables {
        let Some(deps) = deps.as_table_like_mut() else {
            continue;
        };
        for (name, dep) in deps.iter_mut() {
            let Some(dep) = dep.as_table_like_mut() else {
                continue;
            };
            if dep.contains_key("path") {
                if !dep.contains_key("version") {
                    bail!(
                        "path dependency {} needs a version to be published",
                        name.get()
                    );
                }
                dep.remove("path");
            }
        }
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_paths() {
        let out = publishable_cargo_toml(
            "[package]\nname='a'\nversion='0.1.0'\n[dependencies]\nunderweb = { path = '../sdk', version = '0.1.0' }\nlibc = '0.2'\n",
        )
        .unwrap();
        assert!(!out.contains("path"));
        assert!(out.contains("underweb = { version = '0.1.0' }"));
        assert!(publishable_cargo_toml("[dependencies]\nx = { path = 'y' }\n").is_err());
        assert!(publishable_cargo_toml("[package]\nedition.workspace = true\n").is_err());
    }
}
