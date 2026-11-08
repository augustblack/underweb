//! Trust checks for opened packages, using the user's trust store and the
//! available networks: ATProto accounts, domains, and signed lists.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Context, Result};
use underweb_trust::lists::{List, ListAdapter};
use underweb_trust::{KeyPair, Kind, Statement, TrustAdapter, TrustStore, Verdict, evaluate};
use underweb_trust_atproto::Atproto;

use crate::store::Subject;

/// `dns:example.org` identities: the domain publishes its key(s) at
/// `https://example.org/.well-known/underweb/key`, one per line.
pub struct DomainAdapter {
    agent: ureq::Agent,
}

impl Default for DomainAdapter {
    fn default() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into();
        DomainAdapter { agent }
    }
}

impl DomainAdapter {
    fn keys(&self, domain: &str) -> Result<Vec<String>> {
        let host = domain.split(':').next().unwrap_or_default();
        let scheme = if host == "localhost" || host == "127.0.0.1" {
            "http"
        } else {
            "https"
        };
        let url = format!("{scheme}://{domain}/.well-known/underweb/key");
        let text = self
            .agent
            .get(&url)
            .call()?
            .body_mut()
            .with_config()
            .limit(64 << 10)
            .read_to_string()?;
        Ok(text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(String::from)
            .collect())
    }
}

impl TrustAdapter for DomainAdapter {
    fn network(&self) -> &str {
        "web"
    }
    fn handles(&self, identity: &str) -> bool {
        identity.starts_with("dns:")
    }
    /// Any of the domain's listed keys: answers with the first; callers
    /// compare. (A domain listing several keys can sign with any.)
    fn key_of(&self, identity: &str) -> Result<Option<String>> {
        Ok(self
            .keys(identity.trim_start_matches("dns:"))?
            .into_iter()
            .next())
    }
    fn display(&self, identity: &str) -> Option<String> {
        identity.strip_prefix("dns:").map(String::from)
    }
    fn statements(&self, _package: &str, _authors: &[String]) -> Result<Vec<Statement>> {
        Ok(Vec::new())
    }
}

pub struct Trust {
    store: Mutex<TrustStore>,
    pub atproto: Atproto,
    domains: DomainAdapter,
    agent: ureq::Agent,
}

impl Trust {
    pub fn default_dir() -> PathBuf {
        crate::paths::base_dir().join("trust")
    }

    /// The user's trust store in `dir` (default [`Trust::default_dir`]).
    pub fn open(dir: Option<&Path>) -> Result<Trust> {
        let dir = dir
            .map(Path::to_path_buf)
            .unwrap_or_else(Trust::default_dir);
        Ok(Trust::new(
            TrustStore::load(&dir.join("store.toml"))?,
            Atproto::default(),
        ))
    }

    pub fn new(store: TrustStore, atproto: Atproto) -> Trust {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into();
        Trust {
            store: Mutex::new(store),
            atproto,
            domains: DomainAdapter::default(),
            agent,
        }
    }

    /// The user's signing key, created on first use.
    pub fn key(dir: Option<&Path>) -> Result<KeyPair> {
        let dir = dir
            .map(Path::to_path_buf)
            .unwrap_or_else(Trust::default_dir);
        KeyPair::load_or_create(&dir.join("key"))
    }

    pub fn store(&self) -> MutexGuard<'_, TrustStore> {
        self.store.lock().unwrap()
    }

    /// Fetch a list by URL or path.
    pub fn fetch_list(&self, source: &str) -> Result<List> {
        let text = if source.starts_with("https://") || source.starts_with("http://") {
            self.agent
                .get(source)
                .call()?
                .body_mut()
                .with_config()
                .limit(8 << 20)
                .read_to_string()?
        } else {
            std::fs::read_to_string(source.trim_start_matches("file://"))
                .with_context(|| source.to_string())?
        };
        List::parse(&text)
    }

    /// Everything known about the archive of `subject`.
    pub fn check(&self, subject: &Subject, progress: &dyn Fn(&str)) -> Verdict {
        let (follows, sources) = {
            let s = self.store();
            (
                s.follows
                    .iter()
                    .map(|f| f.identity.clone())
                    .collect::<Vec<_>>(),
                s.lists.clone(),
            )
        };
        let mut lists = Vec::new();
        for src in &sources {
            match self.fetch_list(src) {
                Ok(l) => lists.push(l),
                Err(e) => progress(&format!("couldn't read trust list {src}: {e:#}")),
            }
        }
        let lists = ListAdapter::new(lists);
        let adapters: [&dyn TrustAdapter; 3] = [&self.atproto, &self.domains, &lists];

        let mut statements = Vec::new();
        if !follows.is_empty() {
            progress("checking who vouches for it");
            for a in adapters {
                let authors: Vec<String> =
                    follows.iter().filter(|f| a.handles(f)).cloned().collect();
                if authors.is_empty() {
                    continue;
                }
                match a.statements(&subject.package, &authors) {
                    Ok(s) => statements.extend(s),
                    Err(e) => progress(&format!("{}: {e:#}", a.network())),
                }
            }
        }
        let release = subject
            .release_statement
            .as_deref()
            .and_then(|j| serde_json::from_str::<Statement>(j).ok());
        let mut store = self.store();
        let verdict = evaluate(
            &subject.package,
            &subject.hash,
            release.as_ref(),
            &statements,
            &mut store,
            &adapters,
        );
        let _ = store.save(); // newly pinned keys
        verdict
    }
}

/// Sign a release in a repository's index with `key`, as `signer`
/// (`dns:<domain>`, an ATProto DID, or the bare key when `None`). Covers
/// every archive the release has now; sign again after adding targets.
pub fn sign_release(
    repo: &Path,
    name: &str,
    version: &str,
    key: &KeyPair,
    signer: Option<&str>,
) -> Result<Statement> {
    let path = underweb_pkg::index_path(repo, name);
    let mut index = underweb_pkg::Index::parse(&std::fs::read_to_string(&path)?)?;
    let release = index
        .releases
        .iter_mut()
        .find(|r| r.version == version)
        .with_context(|| format!("{name} has no release {version}"))?;
    let s = Statement::new(Kind::Release, name, version, &release.hashes()).sign(key, signer);
    release.statement = Some(serde_json::to_string(&s)?);
    std::fs::write(&path, index.to_toml())?;
    Ok(s)
}
