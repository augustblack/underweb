use anyhow::{Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer as _, Verifier as _};
use serde::{Deserialize, Serialize};

use crate::keys::{KeyPair, decode_key};

/// Signatures cover this prefix plus the statement's JSON without `sig`.
const DOMAIN: &[u8] = b"underweb-statement-v1\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// The publisher's statement that these archives are this release.
    Release,
    /// Someone's opinion of a release.
    Vouch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// (release statements)
    #[default]
    None,
    /// "I use this."
    Used,
    /// "I read the code."
    Reviewed,
    /// "Don't run this."
    Warn,
}

/// A signed claim about some archives of a package release.
///
/// The field order is the canonical order: the signed bytes are this
/// struct as JSON with `sig` empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    pub kind: Kind,
    pub package: String,
    pub version: String,
    /// BLAKE3 hashes of the archives covered, sorted.
    pub hashes: Vec<String>,
    #[serde(default)]
    pub level: Level,
    #[serde(default)]
    pub note: String,
    /// Who: `did:plc:…`/`did:web:…` (ATProto), `dns:example.org`, or
    /// `key:<public key>` (a bare key, nobody in particular).
    pub signer: String,
    /// Base64 Ed25519 public key that made `sig`.
    pub key: String,
    /// Unix seconds.
    pub created: u64,
    #[serde(default)]
    pub sig: String,
}

impl Statement {
    pub fn new(kind: Kind, package: &str, version: &str, hashes: &[String]) -> Statement {
        let mut hashes = hashes.to_vec();
        hashes.sort();
        hashes.dedup();
        Statement {
            kind,
            package: package.into(),
            version: version.into(),
            hashes,
            level: Level::None,
            note: String::new(),
            signer: String::new(),
            key: String::new(),
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            sig: String::new(),
        }
    }

    fn signed_bytes(&self) -> Vec<u8> {
        let unsigned = Statement {
            sig: String::new(),
            ..self.clone()
        };
        let mut bytes = DOMAIN.to_vec();
        bytes.extend(serde_json::to_vec(&unsigned).expect("statement serialises"));
        bytes
    }

    /// Sign as `signer` (defaults to the bare key identity).
    pub fn sign(mut self, key: &KeyPair, signer: Option<&str>) -> Statement {
        self.key = key.public_b64();
        self.signer = signer.map_or_else(|| key.identity(), str::to_string);
        self.sig = B64.encode(key.signing.sign(&self.signed_bytes()).to_bytes());
        self
    }

    /// Check the signature against `key`. Says nothing about whether
    /// `key` belongs to `signer`; that's the adapters' job.
    pub fn verify_signature(&self) -> Result<()> {
        if self.hashes.windows(2).any(|w| w[0] >= w[1]) {
            bail!("hashes must be sorted and distinct");
        }
        if self.signer.starts_with("key:") && self.signer != format!("key:{}", self.key) {
            bail!("a key: signer must be its own key");
        }
        let key = decode_key(&self.key)?;
        let sig = B64
            .decode(&self.sig)
            .map_err(|_| anyhow::anyhow!("bad signature encoding"))?;
        let sig =
            Signature::from_slice(&sig).map_err(|_| anyhow::anyhow!("bad signature length"))?;
        key.verify(&self.signed_bytes(), &sig)
            .map_err(|_| anyhow::anyhow!("signature does not match"))
    }

    /// Whether this statement covers the archive with `hash`.
    pub fn covers(&self, package: &str, hash: &str) -> bool {
        self.package == package && self.hashes.iter().any(|h| h == hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_tamper() {
        let k = KeyPair::generate();
        let s = Statement::new(Kind::Vouch, "p", "1.0.0", &["b".into(), "a".into()]).sign(&k, None);
        assert_eq!(s.hashes, vec!["a", "b"]);
        s.verify_signature().unwrap();
        assert!(s.covers("p", "a") && !s.covers("q", "a") && !s.covers("p", "c"));

        let mut t = s.clone();
        t.level = Level::Warn;
        assert!(t.verify_signature().is_err());
        let mut t = s.clone();
        t.hashes.push("z".into());
        assert!(t.verify_signature().is_err());
        // a claimed signer can't be another bare key
        let mut t = s.clone();
        t.signer = KeyPair::generate().identity();
        assert!(t.verify_signature().is_err());
        // round trip through JSON keeps it valid
        let back: Statement = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        back.verify_signature().unwrap();
    }
}
