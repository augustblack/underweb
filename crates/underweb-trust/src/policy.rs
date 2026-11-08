use std::collections::HashMap;

use crate::adapter::{Identity, TrustAdapter};
use crate::{Kind, Level, Statement, TrustStore};

/// Someone whose statement was verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signer {
    pub identity: String,
    /// What to show: a handle, a domain, "key ab12…".
    pub name: String,
    /// "Bluesky", "web", "list", "key".
    pub network: String,
}

/// What the user should know about an archive before running it.
#[derive(Debug, Clone, Default)]
pub struct Verdict {
    /// Verified publisher of this exact archive.
    pub publisher: Option<Signer>,
    /// Why the release signature was rejected, if it was.
    pub publisher_problem: Option<String>,
    /// People you follow who vouch for this archive.
    pub vouches: Vec<(Signer, Level)>,
    /// People you follow who warn about it, with their note.
    pub warnings: Vec<(Signer, String)>,
    /// Statements that failed verification, and why.
    pub rejected: Vec<String>,
    /// Grant declared permissions without asking.
    pub auto_grant: bool,
}

impl Verdict {
    /// One line for the address bar, in people's words.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        match (&self.publisher, &self.publisher_problem) {
            (Some(p), _) => parts.push(format!("published by {}", p.name)),
            (None, Some(problem)) => parts.push(format!("bad publisher signature: {problem}")),
            (None, None) => parts.push("unsigned".to_string()),
        }
        if !self.warnings.is_empty() {
            let names: Vec<&str> = self.warnings.iter().map(|(s, _)| s.name.as_str()).collect();
            parts.push(format!("WARNED by {}", names.join(", ")));
        }
        if !self.vouches.is_empty() {
            let names: Vec<&str> = self.vouches.iter().map(|(s, _)| s.name.as_str()).collect();
            parts.push(format!("vouched for by {} (you follow)", names.join(", ")));
        }
        parts.join(" · ")
    }
}

/// Check every statement about the archive with `hash` and apply the
/// user's policy. Keys seen for the first time are pinned in `store`; a
/// different key later is rejected.
pub fn evaluate(
    package: &str,
    hash: &str,
    release: Option<&Statement>,
    statements: &[Statement],
    store: &mut TrustStore,
    adapters: &[&dyn TrustAdapter],
) -> Verdict {
    let mut v = Verdict::default();
    let mut keys: HashMap<String, Result<Signer, String>> = HashMap::new();

    let mut check = |s: &Statement, store: &mut TrustStore| -> Result<Signer, String> {
        s.verify_signature().map_err(|e| e.to_string())?;
        if !s.covers(package, hash) {
            return Err("does not cover this archive".into());
        }
        let signer = keys
            .entry(format!("{}|{}", s.signer, s.key))
            .or_insert_with(|| bind_key(&s.signer, &s.key, store, adapters))
            .clone()?;
        Ok(signer)
    };

    if let Some(r) = release {
        if r.kind != Kind::Release {
            v.publisher_problem = Some("not a release statement".into());
        } else {
            match check(r, store) {
                Ok(s) => v.publisher = Some(s),
                Err(e) => v.publisher_problem = Some(e),
            }
        }
    }
    // a publisher you follow counts as a vouch
    if let Some(p) = v.publisher.as_ref().filter(|p| store.follows(&p.identity)) {
        v.vouches.push((p.clone(), Level::Used));
    }

    for s in statements
        .iter()
        .filter(|s| s.kind == Kind::Vouch && s.package == package)
    {
        if !store.follows(&s.signer) {
            continue;
        }
        match check(s, store) {
            Ok(signer) if s.level == Level::Warn => {
                if !v
                    .warnings
                    .iter()
                    .any(|(w, _)| w.identity == signer.identity)
                {
                    v.warnings.push((signer, s.note.clone()));
                }
            }
            Ok(signer) => {
                if !v.vouches.iter().any(|(w, _)| w.identity == signer.identity) {
                    v.vouches.push((signer, s.level));
                }
            }
            Err(e) if e == "does not cover this archive" => {}
            Err(e) => v
                .rejected
                .push(format!("{}: {e}", Identity::short(&s.signer))),
        }
    }
    // a warning outweighs a vouch from the same person
    v.vouches
        .retain(|(s, _)| !v.warnings.iter().any(|(w, _)| w.identity == s.identity));

    v.auto_grant = store.policy.auto_grant
        && v.warnings.is_empty()
        && v.publisher_problem.is_none()
        && v.vouches.len() >= store.policy.min_vouches.max(1);
    v
}

/// Does `key` belong to `identity`? Bare keys are themselves; others ask
/// the adapter that speaks for them, then compare with the pinned key.
fn bind_key(
    identity: &str,
    key: &str,
    store: &mut TrustStore,
    adapters: &[&dyn TrustAdapter],
) -> Result<Signer, String> {
    let name = |adapter: Option<&dyn TrustAdapter>, store: &TrustStore| {
        adapter
            .and_then(|a| a.display(identity))
            .or_else(|| store.name_of(identity).map(String::from))
            .unwrap_or_else(|| Identity::short(identity))
    };
    if Identity::is_key(identity) {
        let name = name(None, store);
        return Ok(Signer {
            identity: identity.into(),
            name,
            network: "key".into(),
        });
    }
    // lists don't vouch for keys; the identity's own network does
    let Some(adapter) = adapters
        .iter()
        .copied()
        .find(|a| a.handles(identity) && a.network() != "list")
    else {
        return Err(format!("no way to look up {}", Identity::short(identity)));
    };
    let published = adapter
        .key_of(identity)
        .map_err(|e| format!("looking up {}: {e:#}", Identity::short(identity)))?;
    if published.as_deref() != Some(key) {
        return Err(format!("the key is not {}'s", Identity::short(identity)));
    }
    match store.pins.get(identity) {
        Some(pinned) if pinned != key => {
            return Err(format!(
                "{}'s key changed since it was first seen",
                Identity::short(identity)
            ));
        }
        Some(_) => {}
        None => {
            store.pins.insert(identity.into(), key.into());
        }
    }
    let name = name(Some(adapter), store);
    Ok(Signer {
        identity: identity.into(),
        name,
        network: adapter.network().into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KeyPair;
    use crate::lists::{List, ListAdapter};

    struct FakeNet {
        keys: HashMap<String, String>,
        statements: Vec<Statement>,
    }

    impl TrustAdapter for FakeNet {
        fn network(&self) -> &str {
            "Bluesky"
        }
        fn handles(&self, id: &str) -> bool {
            id.starts_with("did:")
        }
        fn key_of(&self, id: &str) -> anyhow::Result<Option<String>> {
            Ok(self.keys.get(id).cloned())
        }
        fn display(&self, id: &str) -> Option<String> {
            Some(format!("{}.example", &id[8..]))
        }
        fn statements(&self, package: &str, authors: &[String]) -> anyhow::Result<Vec<Statement>> {
            Ok(self
                .statements
                .iter()
                .filter(|s| s.package == package && authors.contains(&s.signer))
                .cloned()
                .collect())
        }
    }

    fn vouch(k: &KeyPair, signer: &str, level: Level, hashes: &[&str]) -> Statement {
        let hashes: Vec<String> = hashes.iter().map(|h| h.to_string()).collect();
        let mut s = Statement::new(Kind::Vouch, "pkg", "1.0.0", &hashes);
        s.level = level;
        s.sign(k, Some(signer))
    }

    #[test]
    fn follows_vouches_warnings_and_keys() {
        let (pubk, alice, bob, mallory) = (
            KeyPair::generate(),
            KeyPair::generate(),
            KeyPair::generate(),
            KeyPair::generate(),
        );
        let net = FakeNet {
            keys: HashMap::from([
                ("did:plc:pub".to_string(), pubk.public_b64()),
                ("did:plc:alice".to_string(), alice.public_b64()),
                ("did:plc:bob".to_string(), bob.public_b64()),
            ]),
            statements: vec![],
        };
        let release = Statement::new(Kind::Release, "pkg", "1.0.0", &["h1".into(), "h2".into()])
            .sign(&pubk, Some("did:plc:pub"));
        let mut store = TrustStore::in_memory();

        // nobody followed: signed, but no vouches
        let v = evaluate("pkg", "h1", Some(&release), &[], &mut store, &[&net]);
        assert_eq!(v.publisher.as_ref().unwrap().name, "pub.example");
        assert!(!v.auto_grant);
        assert_eq!(v.summary(), "published by pub.example");

        store.follow("did:plc:alice", "alice", "Bluesky");
        store.follow("did:plc:bob", "bob", "Bluesky");
        let a = vouch(&alice, "did:plc:alice", Level::Reviewed, &["h1"]);
        let forged = vouch(&mallory, "did:plc:bob", Level::Reviewed, &["h1"]); // bob's name, mallory's key
        let v = evaluate(
            "pkg",
            "h1",
            Some(&release),
            &[a.clone(), forged],
            &mut store,
            &[&net],
        );
        assert_eq!(v.vouches.len(), 1);
        assert_eq!(v.rejected.len(), 1, "{:?}", v.rejected);
        assert!(v.auto_grant);

        // a vouch for another archive doesn't count
        let v = evaluate(
            "pkg",
            "h2",
            Some(&release),
            std::slice::from_ref(&a),
            &mut store,
            &[&net],
        );
        assert!(v.vouches.is_empty() && !v.auto_grant);

        // a warning blocks auto-grant
        let w = vouch(&bob, "did:plc:bob", Level::Warn, &["h1"]);
        let v = evaluate(
            "pkg",
            "h1",
            Some(&release),
            &[a.clone(), w],
            &mut store,
            &[&net],
        );
        assert_eq!(v.warnings.len(), 1);
        assert!(!v.auto_grant);
        assert!(v.summary().contains("WARNED by bob.example"));

        // the release doesn't verify for a different archive
        let v = evaluate("pkg", "zzz", Some(&release), &[], &mut store, &[&net]);
        assert!(v.publisher.is_none() && v.publisher_problem.is_some());
    }

    #[test]
    fn pinned_keys_detect_changes() {
        let (old, new) = (KeyPair::generate(), KeyPair::generate());
        let mut store = TrustStore::in_memory();
        store.follow("did:plc:alice", "alice", "Bluesky");
        let net_old = FakeNet {
            keys: HashMap::from([("did:plc:alice".into(), old.public_b64())]),
            statements: vec![],
        };
        let v = evaluate(
            "pkg",
            "h",
            None,
            &[vouch(&old, "did:plc:alice", Level::Used, &["h"])],
            &mut store,
            &[&net_old],
        );
        assert_eq!(v.vouches.len(), 1);
        // the account now claims another key
        let net_new = FakeNet {
            keys: HashMap::from([("did:plc:alice".into(), new.public_b64())]),
            statements: vec![],
        };
        let v = evaluate(
            "pkg",
            "h",
            None,
            &[vouch(&new, "did:plc:alice", Level::Used, &["h"])],
            &mut store,
            &[&net_new],
        );
        assert!(v.vouches.is_empty());
        assert!(v.rejected[0].contains("changed"), "{:?}", v.rejected);
    }

    #[test]
    fn lists_and_bare_keys() {
        let lab = KeyPair::generate();
        let mut list = List::new(&lab, "The Lab");
        list.add(vouch(&lab, &lab.identity(), Level::Reviewed, &["h"]));
        // a statement by someone else smuggled into the list is ignored
        let other = KeyPair::generate();
        list.statements
            .push(vouch(&other, &other.identity(), Level::Reviewed, &["h"]));
        let list = List::parse(&list.to_json()).unwrap();
        let adapter = ListAdapter::new(vec![list]);

        let mut store = TrustStore::in_memory();
        store.follow(&lab.identity(), "The Lab", "list");
        let found = adapter
            .statements("pkg", &[lab.identity(), other.identity()])
            .unwrap();
        assert_eq!(found.len(), 1);
        let v = evaluate("pkg", "h", None, &found, &mut store, &[&adapter]);
        assert_eq!(v.vouches.len(), 1);
        assert_eq!(v.vouches[0].0.name, "The Lab");
        assert!(v.auto_grant);
    }
}
