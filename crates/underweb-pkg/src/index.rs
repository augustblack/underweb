//! The release index a publisher serves for each package.
//!
//! ```toml
//! name = "hello-c"
//!
//! [[release]]
//! version = "0.1.0"
//! source = { url = "hello-c/0.1.0/hello-c-0.1.0-src.tar.zst", blake3 = "…", size = 1234 }
//!
//! [release.targets.aarch64-apple-darwin]
//! url = "hello-c/0.1.0/hello-c-0.1.0-aarch64-apple-darwin.tar.zst"
//! blake3 = "…"
//! size = 5678
//! ```
//!
//! URLs are relative to the index unless absolute.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub url: String,
    pub blake3: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Artifact>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub targets: BTreeMap<String, Artifact>,
    /// The publisher's signed release statement (JSON), covering the
    /// hashes of the archives above. See `underweb-trust`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statement: Option<String>,
}

impl Release {
    /// Every archive hash in this release.
    pub fn hashes(&self) -> Vec<String> {
        self.source
            .iter()
            .chain(self.targets.values())
            .map(|a| a.blake3.clone())
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    pub name: String,
    #[serde(default, rename = "release")]
    pub releases: Vec<Release>,
}

impl Index {
    pub fn new(name: &str) -> Index {
        Index {
            name: name.to_string(),
            releases: Vec::new(),
        }
    }

    pub fn parse(text: &str) -> Result<Index> {
        let index: Index = toml::from_str(text).context("parsing release index")?;
        for r in &index.releases {
            semver::Version::parse(&r.version)
                .with_context(|| format!("release version {:?}", r.version))?;
        }
        Ok(index)
    }

    pub fn to_toml(&self) -> String {
        let mut sorted = self.clone();
        sorted
            .releases
            .sort_by_key(|r| semver::Version::parse(&r.version).ok());
        toml::to_string_pretty(&sorted).expect("index serialises")
    }

    /// Add or extend a release. Artifacts for an existing version are
    /// merged, so each target can be published separately.
    pub fn add(&mut self, release: Release) {
        match self
            .releases
            .iter_mut()
            .find(|r| r.version == release.version)
        {
            Some(r) => {
                if release.source.is_some() {
                    r.source = release.source;
                }
                r.targets.extend(release.targets);
                if release.statement.is_some() {
                    r.statement = release.statement;
                }
            }
            None => self.releases.push(release),
        }
    }

    /// The newest release matching `version`: an exact version
    /// (`1.2.3`), a requirement (`1.2`, `^1`, `>=0.3`), or the newest
    /// overall when `None`.
    pub fn select(&self, version: Option<&str>) -> Result<&Release> {
        let req = match version {
            None => semver::VersionReq::STAR,
            Some(v) => match semver::Version::parse(v) {
                Ok(exact) => semver::VersionReq::parse(&format!("={exact}"))?,
                Err(_) => semver::VersionReq::parse(v).with_context(|| format!("version {v:?}"))?,
            },
        };
        let best = self
            .releases
            .iter()
            .filter_map(|r| semver::Version::parse(&r.version).ok().map(|v| (v, r)))
            .filter(|(v, _)| req.matches(v))
            .max_by(|a, b| a.0.cmp(&b.0));
        match best {
            Some((_, r)) => Ok(r),
            None if self.releases.is_empty() => bail!("{} has no releases", self.name),
            None => bail!(
                "{} has no release matching {}",
                self.name,
                version.unwrap_or("*")
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(v: &str) -> Release {
        Release {
            version: v.into(),
            source: Some(Artifact {
                url: format!("{v}.tar.zst"),
                blake3: "00".into(),
                size: 1,
            }),
            targets: BTreeMap::new(),
            statement: None,
        }
    }

    #[test]
    fn selects_versions() {
        let mut i = Index::new("p");
        for v in ["0.1.0", "0.2.0", "0.2.5", "1.0.0-beta.1"] {
            i.add(rel(v));
        }
        assert_eq!(i.select(None).unwrap().version, "0.2.5");
        assert_eq!(i.select(Some("0.2.0")).unwrap().version, "0.2.0");
        assert_eq!(i.select(Some("0.1")).unwrap().version, "0.1.0");
        assert_eq!(
            i.select(Some("^1.0.0-beta")).unwrap().version,
            "1.0.0-beta.1"
        );
        assert!(i.select(Some("3")).is_err());
        let again = Index::parse(&i.to_toml()).unwrap();
        assert_eq!(again.releases.len(), 4);
    }

    #[test]
    fn merges_targets() {
        let mut i = Index::new("p");
        i.add(rel("0.1.0"));
        let mut r = rel("0.1.0");
        r.source = None;
        r.targets.insert(
            "x".into(),
            Artifact {
                url: "x".into(),
                blake3: "1".into(),
                size: 2,
            },
        );
        i.add(r);
        assert_eq!(i.releases.len(), 1);
        assert!(i.releases[0].source.is_some() && i.releases[0].targets.contains_key("x"));
    }
}
