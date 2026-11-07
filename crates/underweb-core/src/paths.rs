use std::path::PathBuf;

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

/// Private data directory for a package.
pub fn data_dir(package: &str) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let base = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/underweb")
    } else if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(x).join("underweb")
    } else {
        home.join(".local/share/underweb")
    };
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
    base.join("data").join(safe)
}
