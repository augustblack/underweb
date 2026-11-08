use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Follow {
    pub identity: String,
    /// What to call them: a handle, a domain.
    pub name: String,
    /// Where they came from: "Bluesky", "list", "web", "key".
    pub network: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicySettings {
    /// Grant a package's declared permissions without asking when enough
    /// people you follow vouch for it and none warn.
    pub auto_grant: bool,
    /// How many is enough.
    pub min_vouches: usize,
}

impl Default for PolicySettings {
    fn default() -> Self {
        PolicySettings {
            auto_grant: true,
            min_vouches: 1,
        }
    }
}

/// The user's trust settings: who they follow, which lists they read, and
/// the key first seen for each identity.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustStore {
    #[serde(default)]
    pub policy: PolicySettings,
    #[serde(default, rename = "follow")]
    pub follows: Vec<Follow>,
    /// Signed lists, by URL or path.
    #[serde(default, rename = "list")]
    pub lists: Vec<String>,
    /// identity -> public key, pinned the first time it was verified.
    #[serde(default)]
    pub pins: BTreeMap<String, String>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl TrustStore {
    pub fn load(path: &Path) -> Result<TrustStore> {
        let mut store: TrustStore = match std::fs::read_to_string(path) {
            Ok(t) => toml::from_str(&t).with_context(|| format!("parsing {}", path.display()))?,
            Err(_) => TrustStore::default(),
        };
        store.path = Some(path.to_path_buf());
        Ok(store)
    }

    pub fn in_memory() -> TrustStore {
        TrustStore::default()
    }

    pub fn save(&self) -> Result<()> {
        let Some(p) = &self.path else { return Ok(()) };
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn follows(&self, identity: &str) -> bool {
        self.follows.iter().any(|f| f.identity == identity)
    }

    pub fn follow(&mut self, identity: &str, name: &str, network: &str) {
        match self.follows.iter_mut().find(|f| f.identity == identity) {
            Some(f) => {
                f.name = name.into();
                f.network = network.into();
            }
            None => self.follows.push(Follow {
                identity: identity.into(),
                name: name.into(),
                network: network.into(),
            }),
        }
    }

    pub fn unfollow(&mut self, identity: &str) {
        self.follows.retain(|f| f.identity != identity);
    }

    pub fn subscribe(&mut self, list: &str) {
        if !self.lists.iter().any(|l| l == list) {
            self.lists.push(list.into());
        }
    }

    pub fn name_of(&self, identity: &str) -> Option<&str> {
        self.follows
            .iter()
            .find(|f| f.identity == identity)
            .map(|f| f.name.as_str())
    }
}
