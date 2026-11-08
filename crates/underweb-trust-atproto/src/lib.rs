//! ATProto (Bluesky) as a trust network.
//!
//! - An identity is an account's DID (`did:plc:…` or `did:web:…`); people
//!   see its handle (`alice.bsky.social`), shown only when the handle
//!   resolves back to the same DID.
//! - An account binds its Underweb signing key with an
//!   `app.underweb.key` record (rkey `self`) in its repository.
//! - Releases and vouches are `app.underweb.statement` records carrying a
//!   signed [`Statement`], verified locally like any other.
//! - Follows come from the account's Bluesky follow graph.
//!
//! Reading uses public endpoints and needs no account. Publishing logs in
//! with an app password (never stored) and writes to the user's own repo.
//!
//! Records are read over HTTPS from each account's server (PDS); repo
//! commit signatures and Merkle proofs are not checked yet. Statements are
//! still signed by the user's own key, and the browser pins the first key
//! seen for each account, so a server can't silently change who said what
//! after the fact.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use underweb_trust::{Statement, TrustAdapter, check_key};

pub const KEY_COLLECTION: &str = "app.underweb.key";
pub const STATEMENT_COLLECTION: &str = "app.underweb.statement";

/// Where a DID lives.
#[derive(Debug, Clone)]
pub struct DidInfo {
    pub did: String,
    pub pds: String,
    /// The handle, if it verifies (resolves back to this DID).
    pub handle: Option<String>,
}

/// A logged-in account, for publishing.
#[derive(Debug, Clone)]
pub struct Session {
    pub did: String,
    pub handle: String,
    pub pds: String,
    access_jwt: String,
}

pub struct Atproto {
    agent: ureq::Agent,
    /// Public AppView for handles and follows.
    pub appview: String,
    /// The did:plc directory.
    pub plc: String,
    dids: Mutex<HashMap<String, DidInfo>>,
}

impl Default for Atproto {
    fn default() -> Self {
        Atproto::new("https://public.api.bsky.app", "https://plc.directory")
    }
}

impl Atproto {
    pub fn new(appview: &str, plc: &str) -> Atproto {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .user_agent(concat!("underweb/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Atproto {
            agent,
            appview: appview.trim_end_matches('/').into(),
            plc: plc.trim_end_matches('/').into(),
            dids: Mutex::new(HashMap::new()),
        }
    }

    fn get_json(&self, url: &str) -> Result<Value> {
        let mut resp = self
            .agent
            .get(url)
            .call()
            .with_context(|| format!("GET {url}"))?;
        Ok(resp.body_mut().with_config().limit(4 << 20).read_json()?)
    }

    fn xrpc_url(base: &str, method: &str, params: &[(&str, &str)]) -> String {
        let mut u = url::Url::parse(&format!("{base}/xrpc/{method}")).expect("valid base url");
        u.query_pairs_mut().extend_pairs(params);
        u.to_string()
    }

    /// `alice.bsky.social` -> `did:plc:…`. DIDs pass through.
    pub fn resolve(&self, handle_or_did: &str) -> Result<String> {
        let h = handle_or_did.trim_start_matches('@');
        if h.starts_with("did:") {
            return Ok(h.to_string());
        }
        let v = self.get_json(&Atproto::xrpc_url(
            &self.appview,
            "com.atproto.identity.resolveHandle",
            &[("handle", h)],
        ))?;
        v["did"]
            .as_str()
            .map(String::from)
            .context("resolveHandle: no did")
    }

    /// The DID document's PDS and (verified) handle.
    pub fn did_info(&self, did: &str) -> Result<DidInfo> {
        if let Some(i) = self.dids.lock().unwrap().get(did) {
            return Ok(i.clone());
        }
        let doc = self.get_json(&self.did_doc_url(did)?)?;
        if doc["id"].as_str() != Some(did) {
            bail!("DID document for {did} is about {}", doc["id"]);
        }
        let pds = doc["service"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|s| {
                s["id"]
                    .as_str()
                    .is_some_and(|id| id.ends_with("#atproto_pds"))
            })
            .and_then(|s| s["serviceEndpoint"].as_str())
            .context("DID document has no PDS")?
            .trim_end_matches('/')
            .to_string();
        let claimed = doc["alsoKnownAs"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|a| a.as_str()?.strip_prefix("at://").map(String::from));
        // a handle counts only if it points back here
        let handle = claimed.filter(|h| self.resolve(h).ok().as_deref() == Some(did));
        let info = DidInfo {
            did: did.into(),
            pds,
            handle,
        };
        self.dids.lock().unwrap().insert(did.into(), info.clone());
        Ok(info)
    }

    fn did_doc_url(&self, did: &str) -> Result<String> {
        if did.starts_with("did:plc:") {
            return Ok(format!("{}/{did}", self.plc));
        }
        let Some(rest) = did.strip_prefix("did:web:") else {
            bail!("unsupported DID method: {did}")
        };
        let mut parts = rest.split(':');
        let host = parts
            .next()
            .unwrap_or_default()
            .replace("%3A", ":")
            .replace("%3a", ":");
        let path: Vec<&str> = parts.collect();
        let loopback = host.starts_with("localhost") || host.starts_with("127.0.0.1");
        let scheme = if loopback { "http" } else { "https" };
        Ok(if path.is_empty() {
            format!("{scheme}://{host}/.well-known/did.json")
        } else {
            format!("{scheme}://{host}/{}/did.json", path.join("/"))
        })
    }

    /// Who `actor` follows on Bluesky: (DID, handle).
    pub fn follows_of(&self, actor: &str) -> Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        let mut cursor = String::new();
        for _ in 0..50 {
            let mut params = vec![("actor", actor), ("limit", "100")];
            if !cursor.is_empty() {
                params.push(("cursor", &cursor));
            }
            let v = self.get_json(&Atproto::xrpc_url(
                &self.appview,
                "app.bsky.graph.getFollows",
                &params,
            ))?;
            for f in v["follows"].as_array().into_iter().flatten() {
                if let (Some(did), Some(handle)) = (f["did"].as_str(), f["handle"].as_str()) {
                    out.push((did.to_string(), handle.to_string()));
                }
            }
            match v["cursor"].as_str() {
                Some(c) if !c.is_empty() => cursor = c.to_string(),
                _ => break,
            }
        }
        Ok(out)
    }

    fn list_records(&self, did: &str, collection: &str) -> Result<Vec<Value>> {
        let pds = self.did_info(did)?.pds;
        let mut out = Vec::new();
        let mut cursor = String::new();
        for _ in 0..20 {
            let mut params = vec![("repo", did), ("collection", collection), ("limit", "100")];
            if !cursor.is_empty() {
                params.push(("cursor", &cursor));
            }
            let v = self.get_json(&Atproto::xrpc_url(
                &pds,
                "com.atproto.repo.listRecords",
                &params,
            ))?;
            out.extend(
                v["records"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|r| r["value"].clone()),
            );
            match v["cursor"].as_str() {
                Some(c) if !c.is_empty() => cursor = c.to_string(),
                _ => break,
            }
        }
        Ok(out)
    }

    // ---- publishing ----------------------------------------------------

    /// Log in with an app password (Bluesky settings → App passwords).
    pub fn login(&self, handle_or_did: &str, app_password: &str) -> Result<Session> {
        let did = self.resolve(handle_or_did)?;
        let info = self.did_info(&did)?;
        let url = format!("{}/xrpc/com.atproto.server.createSession", info.pds);
        let mut resp = self
            .agent
            .post(&url)
            .send_json(json!({ "identifier": did, "password": app_password }))
            .context("logging in")?;
        let v: Value = resp.body_mut().read_json()?;
        Ok(Session {
            did: v["did"].as_str().context("no did")?.to_string(),
            handle: v["handle"].as_str().unwrap_or(handle_or_did).to_string(),
            pds: info.pds,
            access_jwt: v["accessJwt"]
                .as_str()
                .context("no access token")?
                .to_string(),
        })
    }

    fn post(&self, session: &Session, method: &str, body: Value) -> Result<Value> {
        let url = format!("{}/xrpc/{method}", session.pds);
        let mut resp = self
            .agent
            .post(&url)
            .header("Authorization", &format!("Bearer {}", session.access_jwt))
            .send_json(body)
            .with_context(|| method.to_string())?;
        Ok(resp.body_mut().read_json()?)
    }

    /// Declare `key` (base64 Ed25519) as this account's Underweb key.
    pub fn publish_key(&self, session: &Session, key: &str) -> Result<()> {
        check_key(key)?;
        self.post(
            session,
            "com.atproto.repo.putRecord",
            json!({
                "repo": session.did,
                "collection": KEY_COLLECTION,
                "rkey": "self",
                "record": { "$type": KEY_COLLECTION, "key": key, "createdAt": now_rfc3339() },
            }),
        )?;
        Ok(())
    }

    /// Publish a statement signed as this account. Returns its at:// URI.
    pub fn publish_statement(&self, session: &Session, s: &Statement) -> Result<String> {
        if s.signer != session.did {
            bail!(
                "the statement is signed as {}, not {}",
                s.signer,
                session.did
            );
        }
        s.verify_signature()?;
        let v = self.post(
            session,
            "com.atproto.repo.createRecord",
            json!({
                "repo": session.did,
                "collection": STATEMENT_COLLECTION,
                "record": { "$type": STATEMENT_COLLECTION, "statement": s, "createdAt": now_rfc3339() },
            }),
        )?;
        Ok(v["uri"].as_str().unwrap_or_default().to_string())
    }
}

impl TrustAdapter for Atproto {
    fn network(&self) -> &str {
        "Bluesky"
    }

    fn handles(&self, identity: &str) -> bool {
        identity.starts_with("did:plc:") || identity.starts_with("did:web:")
    }

    fn key_of(&self, identity: &str) -> Result<Option<String>> {
        let pds = self.did_info(identity)?.pds;
        let url = Atproto::xrpc_url(
            &pds,
            "com.atproto.repo.getRecord",
            &[
                ("repo", identity),
                ("collection", KEY_COLLECTION),
                ("rkey", "self"),
            ],
        );
        match self.agent.get(&url).call() {
            Ok(mut resp) => {
                let v: Value = resp.body_mut().read_json()?;
                Ok(v["value"]["key"].as_str().map(String::from))
            }
            // no such record: this account has no Underweb key
            Err(ureq::Error::StatusCode(400 | 404)) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn display(&self, identity: &str) -> Option<String> {
        self.did_info(identity).ok()?.handle
    }

    fn statements(&self, package: &str, authors: &[String]) -> Result<Vec<Statement>> {
        let mut out = Vec::new();
        for did in authors.iter().filter(|a| self.handles(a)) {
            // one unreachable account shouldn't hide everyone else's
            let Ok(records) = self.list_records(did, STATEMENT_COLLECTION) else {
                continue;
            };
            for r in records {
                // only statements signed as the repo's own account
                out.extend(
                    serde_json::from_value::<Statement>(r["statement"].clone())
                        .ok()
                        .filter(|s| s.package == package && &s.signer == did),
                );
            }
        }
        Ok(out)
    }
}

/// `2026-11-07T10:20:30Z` without a date library.
fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc3339_shape() {
        let s = super::now_rfc3339();
        assert_eq!(s.len(), 20);
        assert!(s.starts_with("20") && s.ends_with('Z'));
    }
}
