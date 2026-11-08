use anyhow::Result;

use crate::Statement;

/// The kinds of identity, by prefix.
pub struct Identity;

impl Identity {
    pub fn is_key(id: &str) -> bool {
        id.starts_with("key:")
    }
    pub fn is_did(id: &str) -> bool {
        id.starts_with("did:")
    }
    pub fn is_domain(id: &str) -> bool {
        id.starts_with("dns:")
    }

    /// A short form for people: a key's first characters, a bare domain.
    pub fn short(id: &str) -> String {
        if let Some(k) = id.strip_prefix("key:") {
            format!("key {}…", &k[..k.len().min(8)])
        } else if let Some(d) = id.strip_prefix("dns:") {
            d.to_string()
        } else {
            id.to_string()
        }
    }
}

/// A network that knows identities and their statements.
pub trait TrustAdapter: Send + Sync {
    /// Shown to people: "Bluesky", "web", "list".
    fn network(&self) -> &str;
    /// Whether this adapter speaks for `identity`.
    fn handles(&self, identity: &str) -> bool;
    /// The public key (base64) `identity` signs statements with, from the
    /// identity's own source of truth. `None` if it has none.
    fn key_of(&self, identity: &str) -> Result<Option<String>>;
    /// A friendly name: a handle, a domain.
    fn display(&self, _identity: &str) -> Option<String> {
        None
    }
    /// Statements by any of `authors` about `package`.
    fn statements(&self, package: &str, authors: &[String]) -> Result<Vec<Statement>>;
}
