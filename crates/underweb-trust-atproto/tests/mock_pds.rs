//! The adapter against a small in-process imitation of an ATProto PDS and
//! AppView (did:web identities on 127.0.0.1).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use underweb_trust::{KeyPair, Kind, Level, Statement, TrustAdapter, TrustStore, evaluate};
use underweb_trust_atproto::Atproto;

#[derive(Default)]
struct State {
    port: u16,
    /// handle -> did
    accounts: HashMap<String, String>,
    follows: HashMap<String, Vec<String>>,
    records: HashMap<(String, String), Vec<(String, Value)>>,
}

impl State {
    fn did(&self, name: &str) -> String {
        format!("did:web:127.0.0.1%3A{}:{name}", self.port)
    }
    fn handle_of(&self, did: &str) -> Option<String> {
        self.accounts
            .iter()
            .find(|(_, d)| *d == did)
            .map(|(h, _)| h.clone())
    }
}

fn respond(req: tiny_http::Request, status: u16, body: Value) {
    let resp = tiny_http::Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(
            "Content-Type: application/json"
                .parse::<tiny_http::Header>()
                .unwrap(),
        );
    let _ = req.respond(resp);
}

fn serve(state: Arc<Mutex<State>>) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    state.lock().unwrap().port = port;
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let url = url::Url::parse(&format!("http://x{}", req.url())).unwrap();
            let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
            let mut body = String::new();
            let _ = req.as_reader().read_to_string(&mut body);
            let auth = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .map(|h| h.value.to_string());
            let mut s = state.lock().unwrap();
            let path = url.path().to_string();
            match path.as_str() {
                "/xrpc/com.atproto.identity.resolveHandle" => match s.accounts.get(&q["handle"]) {
                    Some(did) => respond(req, 200, json!({ "did": did })),
                    None => respond(req, 400, json!({ "error": "InvalidRequest" })),
                },
                "/xrpc/app.bsky.graph.getFollows" => {
                    let actor = s
                        .accounts
                        .get(&q["actor"])
                        .cloned()
                        .unwrap_or(q["actor"].clone());
                    let follows: Vec<Value> = s
                        .follows
                        .get(&actor)
                        .into_iter()
                        .flatten()
                        .map(|d| json!({ "did": d, "handle": s.handle_of(d) }))
                        .collect();
                    respond(req, 200, json!({ "follows": follows }))
                }
                "/xrpc/com.atproto.repo.getRecord" => {
                    let key = (q["repo"].clone(), q["collection"].clone());
                    match s
                        .records
                        .get(&key)
                        .and_then(|r| r.iter().find(|(k, _)| *k == q["rkey"]))
                    {
                        Some((_, v)) => respond(req, 200, json!({ "value": v })),
                        None => respond(req, 400, json!({ "error": "RecordNotFound" })),
                    }
                }
                "/xrpc/com.atproto.repo.listRecords" => {
                    let key = (q["repo"].clone(), q["collection"].clone());
                    let recs: Vec<Value> = s
                        .records
                        .get(&key)
                        .into_iter()
                        .flatten()
                        .map(|(_, v)| json!({ "value": v }))
                        .collect();
                    respond(req, 200, json!({ "records": recs }))
                }
                "/xrpc/com.atproto.server.createSession" => {
                    let b: Value = serde_json::from_str(&body).unwrap();
                    let did = b["identifier"].as_str().unwrap().to_string();
                    if b["password"] == "app-pw" {
                        let handle = s.handle_of(&did);
                        respond(
                            req,
                            200,
                            json!({ "did": did, "handle": handle, "accessJwt": format!("tok {did}") }),
                        )
                    } else {
                        respond(req, 401, json!({ "error": "AuthenticationRequired" }))
                    }
                }
                "/xrpc/com.atproto.repo.putRecord" | "/xrpc/com.atproto.repo.createRecord" => {
                    let b: Value = serde_json::from_str(&body).unwrap();
                    let repo = b["repo"].as_str().unwrap().to_string();
                    if auth != Some(format!("Bearer tok {repo}")) {
                        respond(req, 401, json!({ "error": "AuthenticationRequired" }));
                        continue;
                    }
                    let coll = b["collection"].as_str().unwrap().to_string();
                    let list = s.records.entry((repo.clone(), coll.clone())).or_default();
                    let rkey = b["rkey"]
                        .as_str()
                        .map(String::from)
                        .unwrap_or_else(|| format!("r{}", list.len()));
                    list.retain(|(k, _)| *k != rkey);
                    list.push((rkey.clone(), b["record"].clone()));
                    respond(
                        req,
                        200,
                        json!({ "uri": format!("at://{repo}/{coll}/{rkey}") }),
                    )
                }
                p if p.ends_with("/did.json") => {
                    let name = p.trim_start_matches('/').trim_end_matches("/did.json");
                    let did = s.did(name);
                    let handle = s.handle_of(&did);
                    respond(
                        req,
                        200,
                        json!({
                            "id": did,
                            "alsoKnownAs": [format!("at://{}", handle.unwrap_or_default())],
                            "service": [{ "id": "#atproto_pds", "type": "AtprotoPersonalDataServer",
                                          "serviceEndpoint": format!("http://127.0.0.1:{}", s.port) }],
                        }),
                    )
                }
                _ => respond(req, 404, json!({})),
            }
        }
    });
    format!("http://127.0.0.1:{port}")
}

struct World {
    net: Atproto,
    state: Arc<Mutex<State>>,
}

fn world(names: &[&str]) -> World {
    let state = Arc::new(Mutex::new(State::default()));
    let base = serve(state.clone());
    {
        let mut s = state.lock().unwrap();
        for n in names {
            let did = s.did(n);
            s.accounts.insert(format!("{n}.test"), did);
        }
    }
    World {
        net: Atproto::new(&base, &base),
        state,
    }
}

fn vouch(key: &KeyPair, did: &str, level: Level, hash: &str) -> Statement {
    let mut s = Statement::new(Kind::Vouch, "pkg", "1.0.0", &[hash.to_string()]);
    s.level = level;
    s.sign(key, Some(did))
}

#[test]
fn vouches_from_followed_accounts() {
    let w = world(&["alice", "bob", "me", "mallory"]);
    let did = |n: &str| w.state.lock().unwrap().did(n);
    {
        let mut s = w.state.lock().unwrap();
        let follows = vec![s.did("alice"), s.did("bob")];
        let me = s.did("me");
        s.follows.insert(me, follows);
    }
    let (alice_key, bob_key, mallory_key) = (
        KeyPair::generate(),
        KeyPair::generate(),
        KeyPair::generate(),
    );

    // alice: binds her key, vouches
    let alice = w.net.login("alice.test", "app-pw").unwrap();
    assert_eq!(alice.handle, "alice.test");
    w.net.publish_key(&alice, &alice_key.public_b64()).unwrap();
    w.net
        .publish_statement(
            &alice,
            &vouch(&alice_key, &alice.did, Level::Reviewed, "h1"),
        )
        .unwrap();
    // can't publish as someone else
    assert!(
        w.net
            .publish_statement(&alice, &vouch(&alice_key, &did("bob"), Level::Used, "h1"))
            .is_err()
    );
    assert!(w.net.login("alice.test", "wrong").is_err());

    // bob: key bound, but a statement in his repo was signed with mallory's
    // key (a compromised server, say)
    let bob = w.net.login("bob.test", "app-pw").unwrap();
    w.net.publish_key(&bob, &bob_key.public_b64()).unwrap();
    w.state
        .lock()
        .unwrap()
        .records
        .entry((bob.did.clone(), "app.underweb.statement".into()))
        .or_default()
        .push((
            "x".into(),
            json!({ "statement": vouch(&mallory_key, &bob.did, Level::Reviewed, "h1") }),
        ));
    // mallory puts a statement claiming to be alice in her own repo
    let mallory = w.net.login("mallory.test", "app-pw").unwrap();
    w.state
        .lock()
        .unwrap()
        .records
        .entry((mallory.did.clone(), "app.underweb.statement".into()))
        .or_default()
        .push((
            "y".into(),
            json!({ "statement": vouch(&mallory_key, &alice.did, Level::Warn, "h1") }),
        ));

    // me: import follows, look up statements, decide
    let mut store = TrustStore::in_memory();
    for (d, h) in w.net.follows_of("me.test").unwrap() {
        store.follow(&d, &h, "Bluesky");
    }
    assert_eq!(store.follows.len(), 2);
    let authors: Vec<String> = store
        .follows
        .iter()
        .map(|f| f.identity.clone())
        .chain([mallory.did.clone()])
        .collect();
    let statements = w.net.statements("pkg", &authors).unwrap();
    assert_eq!(
        statements.len(),
        2,
        "mallory's impersonation is filtered by repo"
    ); // alice's + bob's forged one
    let v = evaluate("pkg", "h1", None, &statements, &mut store, &[&w.net]);
    assert_eq!(v.vouches.len(), 1, "{v:?}");
    assert_eq!(v.vouches[0].0.name, "alice.test");
    assert_eq!(v.rejected.len(), 1, "{:?}", v.rejected);
    assert!(v.auto_grant);
    assert!(store.pins.contains_key(&alice.did));
}

#[test]
fn release_signed_by_an_account_and_a_warning() {
    let w = world(&["publisher", "carol"]);
    let (pub_key, carol_key) = (KeyPair::generate(), KeyPair::generate());
    let p = w.net.login("publisher.test", "app-pw").unwrap();
    w.net.publish_key(&p, &pub_key.public_b64()).unwrap();
    let release =
        Statement::new(Kind::Release, "pkg", "1.0.0", &["h1".into()]).sign(&pub_key, Some(&p.did));

    let carol = w.net.login("carol.test", "app-pw").unwrap();
    w.net.publish_key(&carol, &carol_key.public_b64()).unwrap();
    let mut warn = vouch(&carol_key, &carol.did, Level::Warn, "h1");
    warn.note = "mines bitcoin".into();
    let warn = warn.sign(&carol_key, Some(&carol.did));
    w.net.publish_statement(&carol, &warn).unwrap();

    let mut store = TrustStore::in_memory();
    store.follow(&carol.did, "carol.test", "Bluesky");
    let statements = w
        .net
        .statements("pkg", std::slice::from_ref(&carol.did))
        .unwrap();
    let v = evaluate(
        "pkg",
        "h1",
        Some(&release),
        &statements,
        &mut store,
        &[&w.net],
    );
    assert_eq!(v.publisher.as_ref().unwrap().name, "publisher.test");
    assert_eq!(v.warnings[0].1, "mines bitcoin");
    assert!(!v.auto_grant);
    assert_eq!(
        v.summary(),
        "published by publisher.test · WARNED by carol.test"
    );
}

#[test]
fn account_without_key_is_not_trusted() {
    let w = world(&["dave"]);
    let k = KeyPair::generate();
    let dave = w.net.login("dave.test", "app-pw").unwrap();
    // no publish_key
    let mut store = TrustStore::in_memory();
    store.follow(&dave.did, "dave", "Bluesky");
    let v = evaluate(
        "pkg",
        "h1",
        None,
        &[vouch(&k, &dave.did, Level::Used, "h1")],
        &mut store,
        &[&w.net],
    );
    assert!(v.vouches.is_empty());
    assert!(v.rejected[0].contains("key is not"), "{:?}", v.rejected);
}

/// Real Bluesky, read-only: `cargo test -p underweb-trust-atproto -- --ignored`.
#[test]
#[ignore = "uses the network"]
fn live_bluesky_lookup() {
    let net = Atproto::default();
    let did = net.resolve("bsky.app").unwrap();
    assert!(did.starts_with("did:plc:"), "{did}");
    let info = net.did_info(&did).unwrap();
    assert!(info.pds.starts_with("https://"), "{info:?}");
    assert_eq!(info.handle.as_deref(), Some("bsky.app"));
    let follows = net.follows_of("bsky.app").unwrap();
    assert!(!follows.is_empty());
    // no Underweb key on this account
    assert_eq!(net.key_of(&did).unwrap(), None);
    println!(
        "bsky.app = {did} on {}, follows {}",
        info.pds,
        follows.len()
    );
}
