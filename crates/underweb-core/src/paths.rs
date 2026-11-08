use std::path::{Path, PathBuf};

/// The runner binary: `$UNDERWEB_RUNNER`, else next to the current
/// executable (or its parent, for `cargo test` binaries in `deps/`).
pub fn runner_path() -> PathBuf {
    if let Some(p) = std::env::var_os("UNDERWEB_RUNNER") {
        return p.into();
    }
    let name = if cfg!(windows) {
        "underweb-runner.exe"
    } else {
        "underweb-runner"
    };
    let exe = std::env::current_exe().unwrap_or_default();
    let mut dir = exe.parent().map(PathBuf::from).unwrap_or_default();
    if !dir.join(name).exists() && dir.ends_with("deps") {
        dir.pop();
    }
    dir.join(name)
}

pub(crate) fn base_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/underweb")
    } else if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(x).join("underweb")
    } else {
        home.join(".local/share/underweb")
    }
}

/// Where "always allow" grants for a package from `identity` are kept.
pub fn grants_file(dir: Option<&Path>, package: &str, identity: &str) -> PathBuf {
    let dir = dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| base_dir().join("grants"));
    let id = underweb_pkg::hash(identity.as_bytes());
    dir.join(format!("{package}-{}.txt", &id[..16]))
}

/// Private data directory for a package.
pub fn data_dir(package: &str) -> PathBuf {
    let safe: String = package
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    base_dir().join("data").join(safe)
}
