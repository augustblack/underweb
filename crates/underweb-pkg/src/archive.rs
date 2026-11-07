//! Package archives: deterministic `.tar.zst` with regular files only.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

/// BLAKE3 of `bytes`, lowercase hex.
pub fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Files under `dir` as `(relative path, contents)`, sorted, skipping
/// hidden entries and any top-level directory in `skip_dirs`.
pub fn collect_files(dir: &Path, skip_dirs: &[&str]) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let mut out = Vec::new();
    walk(dir, dir, skip_dirs, &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn walk(root: &Path, dir: &Path, skip: &[&str], out: &mut Vec<(PathBuf, Vec<u8>)>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(root)?.to_path_buf();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            if dir == root && skip.contains(&name.as_ref()) {
                continue;
            }
            walk(root, &path, skip, out)?;
        } else if ty.is_file() {
            out.push((rel, std::fs::read(&path)?));
        }
        // symlinks and the rest are left out
    }
    Ok(())
}

/// Build an archive. Timestamps and owners are zeroed so the same files
/// always give the same bytes (and hash).
pub fn pack(files: &[(PathBuf, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut tar = tar::Builder::new(Vec::new());
    for (path, data) in files {
        check_path(path)?;
        let mut header = tar::Header::new_ustar();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_entry_type(tar::EntryType::Regular);
        tar.append_data(&mut header, path, data.as_slice())?;
    }
    let tar = tar.into_inner()?;
    Ok(zstd::encode_all(tar.as_slice(), 19)?)
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_total: u64,
    pub max_files: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_total: 512 << 20,
            max_files: 20_000,
        }
    }
}

/// Extract into `dest`, which must not exist yet. Only regular files and
/// directories with plain relative paths are accepted; anything else
/// (absolute paths, `..`, links, devices) fails the whole archive.
pub fn unpack(archive: &[u8], dest: &Path, limits: Limits) -> Result<()> {
    if dest.exists() {
        bail!("{} already exists", dest.display());
    }
    std::fs::create_dir_all(dest)?;
    let result = unpack_into(archive, dest, limits);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(dest);
    }
    result
}

fn unpack_into(archive: &[u8], dest: &Path, limits: Limits) -> Result<()> {
    let decoder = zstd::Decoder::new(archive)?;
    // cap decompressed size whatever the headers claim
    let mut tar = tar::Archive::new(decoder.take(limits.max_total + 1));
    let (mut total, mut files) = (0u64, 0usize);
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        check_path(&path)?;
        let target = dest.join(&path);
        match entry.header().entry_type() {
            tar::EntryType::Directory => std::fs::create_dir_all(&target)?,
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                files += 1;
                total += entry.header().size()?;
                if files > limits.max_files || total > limits.max_total {
                    bail!("archive too large");
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut data = Vec::new();
                entry.read_to_end(&mut data)?;
                std::fs::write(&target, data)?;
            }
            other => bail!("{}: unsupported entry type {other:?}", path.display()),
        }
    }
    Ok(())
}

fn check_path(path: &Path) -> Result<()> {
    let ok = !path.as_os_str().is_empty()
        && path.components().all(|c| matches!(c, Component::Normal(_)));
    if !ok {
        bail!("unsafe path in archive: {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<(PathBuf, Vec<u8>)> {
        vec![
            ("underweb.toml".into(), b"x".to_vec()),
            ("src/main.ts".into(), b"y".to_vec()),
        ]
    }

    #[test]
    fn round_trip_and_deterministic() {
        let a = pack(&files()).unwrap();
        assert_eq!(hash(&a), hash(&pack(&files()).unwrap()));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out");
        unpack(&a, &dest, Limits::default()).unwrap();
        assert_eq!(std::fs::read(dest.join("src/main.ts")).unwrap(), b"y");
    }

    /// A hand-made tar with a hostile entry, which `pack` refuses to write.
    fn evil(path: &str, ty: tar::EntryType) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        // write the name bytes directly to bypass the builder's checks
        let name = &mut h.as_gnu_mut().unwrap().name;
        name[..path.len()].copy_from_slice(path.as_bytes());
        h.set_size(1);
        h.set_entry_type(ty);
        if ty == tar::EntryType::Symlink {
            h.set_size(0);
            h.set_link_name("/etc/passwd").unwrap();
        }
        h.set_cksum();
        let data: &[u8] = if ty == tar::EntryType::Symlink {
            b""
        } else {
            b"z"
        };
        tar.append(&h, data).unwrap();
        zstd::encode_all(tar.into_inner().unwrap().as_slice(), 3).unwrap()
    }

    #[test]
    fn rejects_escapes_and_links() {
        let dir = tempfile::tempdir().unwrap();
        for (i, (path, ty)) in [
            ("../escape.txt", tar::EntryType::Regular),
            ("/abs.txt", tar::EntryType::Regular),
            ("link", tar::EntryType::Symlink),
        ]
        .into_iter()
        .enumerate()
        {
            let dest = dir.path().join(format!("d{i}"));
            assert!(
                unpack(&evil(path, ty), &dest, Limits::default()).is_err(),
                "{path}"
            );
            assert!(!dest.exists());
        }
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn enforces_size_limit() {
        let big = vec![(PathBuf::from("big"), vec![0u8; 1 << 20])];
        let dir = tempfile::tempdir().unwrap();
        let limits = Limits {
            max_total: 1000,
            max_files: 10,
        };
        assert!(unpack(&pack(&big).unwrap(), &dir.path().join("x"), limits).is_err());
    }
}
