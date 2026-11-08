//! Resolving addresses to package directories on disk.
//!
//! Remote packages are cached by content: an archive is stored under its
//! BLAKE3 hash and unpacked once into `packages/<hash>/`. The release index
//! is fetched every time a package is opened, so new versions show up at
//! once (the old browser's cache never refreshed); if the publisher can't
//! be reached, the last index seen is used.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use underweb_pkg::{
    Address, Artifact, Index, Limits, Package, hash, host_triple, parse_address, unpack,
};

/// Indexes are small; anything bigger is not an index.
const MAX_INDEX: u64 = 1 << 20;

#[derive(Debug, Clone)]
pub enum Origin {
    Local,
    Remote {
        index_url: String,
        version: String,
        built_from_source: bool,
    },
}

/// What trust decisions are about: one archive of one release.
#[derive(Debug, Clone)]
pub struct Subject {
    pub package: String,
    pub version: String,
    /// The archive that was run.
    pub hash: String,
    /// Every archive of the release (what a vouch covers).
    pub release_hashes: Vec<String>,
    /// The publisher's signed release statement, if the index has one.
    pub release_statement: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub dir: PathBuf,
    pub params: Vec<(String, String)>,
    pub origin: Origin,
    /// Remote packages only.
    pub subject: Option<Subject>,
}

pub struct Store {
    root: PathBuf,
    /// Turn the build sandbox off (development; the probe tests' control).
    pub sandbox: bool,
    /// The Underweb SDK sources Rust packages are built against.
    pub sdk: PathBuf,
    agent: ureq::Agent,
}

impl Store {
    /// The per-user cache: `~/Library/Caches/underweb` on macOS,
    /// `$XDG_CACHE_HOME/underweb` or `~/.cache/underweb` elsewhere.
    pub fn default_root() -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        if cfg!(target_os = "macos") {
            home.join("Library/Caches/underweb")
        } else if let Some(x) = std::env::var_os("XDG_CACHE_HOME") {
            PathBuf::from(x).join("underweb")
        } else {
            home.join(".cache/underweb")
        }
    }

    pub fn new(root: impl Into<PathBuf>) -> Store {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(60)))
            .user_agent(concat!("underweb/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        let sdk = std::env::var_os("UNDERWEB_SDK")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .canonicalize()
                    .unwrap_or_default()
            });
        Store {
            root: root.into(),
            sandbox: true,
            sdk,
            agent,
        }
    }

    /// Resolve an address, downloading and building as needed. `progress`
    /// receives human-readable status lines. Blocks; call it off the UI
    /// thread.
    pub fn open(&self, address: &str, progress: &dyn Fn(&str)) -> Result<Resolved> {
        let parsed = parse_address(address)?;
        match &parsed.address {
            Address::Local(path) => {
                let pkg = Package::open(path)?;
                if pkg.needs_build() {
                    bail!(
                        "{} has no library for {}; build it with `uw build`",
                        pkg.name(),
                        host_triple()
                    );
                }
                Ok(Resolved {
                    dir: pkg.dir,
                    params: parsed.params,
                    origin: Origin::Local,
                    subject: None,
                })
            }
            Address::Remote { name, version, .. } => {
                let index_url = parsed.address.index_url().unwrap();
                let index = self.index(&index_url, name, progress)?;
                let release = index.select(version.as_deref())?;
                progress(&format!("{name} {}", release.version));

                let triple = host_triple();
                let (artifact, prebuilt) = match release.targets.get(triple) {
                    Some(a) => (a, true),
                    None => (
                        release.source.as_ref().with_context(|| {
                            format!(
                                "{name} {} has neither a build for {triple} nor source",
                                release.version
                            )
                        })?,
                        false,
                    ),
                };
                let dir = self.fetch_package(&index_url, artifact, progress)?;
                let pkg = Package::open(&dir)?;
                if pkg.name() != name || pkg.version() != release.version {
                    bail!(
                        "archive holds {} {}, the index says {name} {}",
                        pkg.name(),
                        pkg.version(),
                        release.version
                    );
                }
                let mut built = false;
                if pkg.needs_build() {
                    if prebuilt {
                        bail!("the {triple} archive of {name} has no library");
                    }
                    progress(&format!(
                        "no prebuilt {triple} library; building {name} from source"
                    ));
                    self.build(&pkg, progress)?;
                    built = true;
                }
                let subject = Subject {
                    package: name.clone(),
                    version: release.version.clone(),
                    hash: artifact.blake3.clone(),
                    release_hashes: release.hashes(),
                    release_statement: release.statement.clone(),
                };
                Ok(Resolved {
                    dir,
                    params: parsed.params,
                    origin: Origin::Remote {
                        index_url,
                        version: release.version.clone(),
                        built_from_source: built,
                    },
                    subject: Some(subject),
                })
            }
        }
    }

    fn index(&self, url: &str, name: &str, progress: &dyn Fn(&str)) -> Result<Index> {
        let cached = self
            .root
            .join("indexes")
            .join(format!("{:.32}.toml", hash(url.as_bytes())));
        progress(&format!("fetching {url}"));
        let text = match self
            .get(url, MAX_INDEX)
            .and_then(|b| Ok(String::from_utf8(b)?))
        {
            Ok(text) => {
                std::fs::create_dir_all(cached.parent().unwrap())?;
                std::fs::write(&cached, &text)?;
                text
            }
            Err(e) => match std::fs::read_to_string(&cached) {
                Ok(text) => {
                    progress(&format!("offline ({e:#}); using the last index seen"));
                    text
                }
                Err(_) => return Err(e.context(format!("fetching {url}"))),
            },
        };
        let index = Index::parse(&text)?;
        if index.name != name {
            bail!("{url} is the index of {:?}, not {name:?}", index.name);
        }
        Ok(index)
    }

    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        let mut resp = self.agent.get(url).call()?;
        Ok(resp.body_mut().with_config().limit(limit).read_to_vec()?)
    }

    /// Download (unless cached), verify and unpack an archive. Returns the
    /// unpacked directory.
    fn fetch_package(
        &self,
        index_url: &str,
        a: &Artifact,
        progress: &dyn Fn(&str),
    ) -> Result<PathBuf> {
        if a.blake3.len() != 64 || !a.blake3.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("bad hash in index: {:?}", a.blake3);
        }
        let dir = self.root.join("packages").join(&a.blake3);
        if dir.join("underweb.toml").exists() {
            return Ok(dir);
        }
        let archive_path = self
            .root
            .join("archives")
            .join(format!("{}.tar.zst", a.blake3));
        let bytes = match std::fs::read(&archive_path) {
            Ok(b) if hash(&b) == a.blake3 => b,
            _ => {
                let url = url::Url::parse(index_url)?
                    .join(&a.url)
                    .context("archive url")?;
                progress(&format!("downloading {url} ({} kB)", a.size / 1024));
                let b = self.get(url.as_str(), (a.size + 1).min(Limits::default().max_total))?;
                let got = hash(&b);
                if got != a.blake3 {
                    bail!("{url} does not match its hash in the index (got {got}); refusing it");
                }
                std::fs::create_dir_all(archive_path.parent().unwrap())?;
                std::fs::write(&archive_path, &b)?;
                b
            }
        };
        // unpack beside, then rename, so a half-unpacked package is never seen
        let tmp = self
            .root
            .join("packages")
            .join(format!(".{}.{}", a.blake3, std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        unpack(&bytes, &tmp, Limits::default())?;
        match std::fs::rename(&tmp, &dir) {
            Ok(()) => {}
            Err(_) if dir.join("underweb.toml").exists() => {
                let _ = std::fs::remove_dir_all(&tmp);
            }
            Err(e) => return Err(e.into()),
        }
        Ok(dir)
    }

    /// Build a native package in place, in the sandboxed builder.
    pub fn build(&self, pkg: &Package, progress: &dyn Fn(&str)) -> Result<()> {
        if !pkg.has_source() {
            bail!("{} has no source to build from", pkg.name());
        }
        let work = self.root.join("build").join(pkg.dir.file_name().unwrap());
        std::fs::create_dir_all(work.join("tmp"))?;
        let mut cmd = Command::new(crate::runner_path());
        cmd.arg("--build")
            .arg("--package")
            .arg(&pkg.dir)
            .arg("--work")
            .arg(&work)
            .arg("--sdk")
            .arg(&self.sdk)
            .env_clear()
            .env("HOME", &work)
            .env("TMPDIR", work.join("tmp"))
            .env("PATH", builder_path(&[]))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Ok(cc) = std::env::var("CC") {
            cmd.env("CC", cc);
        }
        if !self.sandbox {
            cmd.arg("--no-sandbox");
        }
        if pkg.kind() == underweb_pkg::Kind::Rust {
            let rust = Toolchain::find()?;
            let cargo_home = self.root.join("cargo-home");
            std::fs::create_dir_all(&cargo_home)?;
            // dependencies are downloaded here, outside the sandbox;
            // `cargo fetch` runs no package code
            progress("fetching Rust dependencies");
            let status = Command::new(&rust.cargo)
                .current_dir(&pkg.dir)
                .arg("fetch")
                .args(sdk_patch(&self.sdk))
                .env("CARGO_HOME", &cargo_home)
                .env("RUSTUP_HOME", &rust.rustup_home)
                .stdout(Stdio::null())
                .status()?;
            if !status.success() {
                bail!("cargo fetch failed");
            }
            cmd.env("PATH", builder_path(std::slice::from_ref(&rust.bin)))
                .env("CARGO_HOME", &cargo_home)
                .env("RUSTUP_HOME", &rust.rustup_home)
                .arg("--read")
                .arg(&rust.rustup_home)
                .arg("--read")
                .arg(&rust.bin)
                .arg("--write")
                .arg(&cargo_home);
        }
        progress(&format!(
            "building {} (sandboxed: {})",
            pkg.name(),
            self.sandbox
        ));
        let mut child = cmd.spawn().context("starting the builder")?;
        let mut tail: Vec<String> = Vec::new();
        for line in BufReader::new(child.stderr.take().unwrap()).lines() {
            let line = line?;
            progress(&line);
            tail.push(line);
            if tail.len() > 30 {
                tail.remove(0);
            }
        }
        if !child.wait()?.success() {
            bail!("building {} failed:\n{}", pkg.name(), tail.join("\n"));
        }
        Ok(())
    }
}

/// PATH for the builder: the given directories, wherever `pkg-config`
/// lives (Homebrew), and the system's. The sandbox decides what is
/// actually readable.
fn builder_path(first: &[PathBuf]) -> String {
    let mut dirs: Vec<PathBuf> = first.to_vec();
    if let Some(d) = which("pkg-config").and_then(|p| p.parent().map(Path::to_path_buf)) {
        dirs.push(d);
    }
    dirs.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
    dirs.dedup();
    std::env::join_paths(dirs)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(program))
        .find(|p| p.exists())
}

fn sdk_patch(sdk: &Path) -> [String; 2] {
    let path = sdk.join("crates/underweb");
    [
        "--config".into(),
        format!("patch.crates-io.underweb.path={:?}", path.to_string_lossy()),
    ]
}

struct Toolchain {
    cargo: PathBuf,
    bin: PathBuf,
    rustup_home: PathBuf,
}

impl Toolchain {
    fn find() -> Result<Toolchain> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let candidates = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .chain([home.join(".cargo/bin")]);
        let cargo = candidates
            .map(|d| d.join("cargo"))
            .find(|c| c.exists())
            .context("this package is built from Rust source, and Rust is not installed (see https://rustup.rs)")?;
        let rustup_home = std::env::var_os("RUSTUP_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".rustup"));
        let bin = cargo.parent().unwrap().canonicalize()?;
        Ok(Toolchain {
            cargo,
            bin,
            rustup_home,
        })
    }
}
