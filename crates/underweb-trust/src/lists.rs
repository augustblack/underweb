//! Signed lists: a file of statements by one owner, fetched from a URL or
//! a path. Subscribing to a list means following its owner. Good for
//! organisations ("everything our lab reviewed") and for offline use.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::adapter::{Identity, TrustAdapter};
use crate::{KeyPair, Statement};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct List {
    /// Who the list speaks for: a bare key or a domain.
    pub owner: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    pub statements: Vec<Statement>,
}

impl List {
    pub fn new(key: &KeyPair, name: &str) -> List {
        List {
            owner: key.identity(),
            name: name.into(),
            statements: Vec::new(),
        }
    }

    pub fn parse(text: &str) -> Result<List> {
        let list: List = serde_json::from_str(text)?;
        if !(Identity::is_key(&list.owner) || Identity::is_domain(&list.owner)) {
            bail!("list owners are keys or domains");
        }
        Ok(list)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("list serialises")
    }

    /// Add (replacing the owner's earlier statement on the same release).
    pub fn add(&mut self, s: Statement) {
        self.statements
            .retain(|o| !(o.package == s.package && o.version == s.version && o.kind == s.kind));
        self.statements.push(s);
    }
}

/// The lists the user subscribes to, already fetched.
#[derive(Default)]
pub struct ListAdapter {
    lists: Vec<List>,
}

impl ListAdapter {
    pub fn new(lists: Vec<List>) -> ListAdapter {
        ListAdapter { lists }
    }
}

impl TrustAdapter for ListAdapter {
    fn network(&self) -> &str {
        "list"
    }

    fn handles(&self, identity: &str) -> bool {
        self.lists.iter().any(|l| l.owner == identity)
    }

    /// Lists don't vouch for keys: bare keys are themselves, and domain
    /// keys come from the domain.
    fn key_of(&self, _identity: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn display(&self, identity: &str) -> Option<String> {
        self.lists
            .iter()
            .find(|l| l.owner == identity && !l.name.is_empty())
            .map(|l| l.name.clone())
    }

    fn statements(&self, package: &str, authors: &[String]) -> Result<Vec<Statement>> {
        Ok(self
            .lists
            .iter()
            .filter(|l| authors.contains(&l.owner))
            // a list only speaks for its owner
            .flat_map(|l| l.statements.iter().filter(move |s| s.signer == l.owner))
            .filter(|s| s.package == package)
            .cloned()
            .collect())
    }
}
