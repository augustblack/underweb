//! Trust end to end: a domain publishes and signs a release, a reviewer's
//! signed list vouches for it, and the user's store decides.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use underweb_core::trust_types::lists::List;
use underweb_core::trust_types::{KeyPair, Kind, Level, Statement, TrustStore};
use underweb_core::{Store, Trust, sign_release};
use underweb_pkg::{Package, PublishOptions, publish};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn serve(dir: &Path) -> String {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let addr = server.server_addr().to_ip().unwrap().to_string();
    let dir = dir.to_path_buf();
    std::thread::spawn(move || {
        for req in server.incoming_requests() {
            let resp = match std::fs::read(dir.join(req.url().trim_start_matches('/'))) {
                Ok(b) => tiny_http::Response::from_data(b).boxed(),
                Err(_) => tiny_http::Response::empty(404).boxed(),
            };
            let _ = req.respond(resp);
        }
    });
    addr
}

struct Setup {
    tmp: tempfile::TempDir,
    host: String,
    repo: PathBuf,
    publisher: KeyPair,
}

/// hello-ts published at a local "domain" that serves its key, release
/// signed as that domain.
fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".well-known/underweb")).unwrap();
    let host = serve(&repo);
    let publisher = KeyPair::generate();
    std::fs::write(
        repo.join(".well-known/underweb/key"),
        publisher.public_b64() + "\n",
    )
    .unwrap();
    let pkg = Package::open(root().join("examples/hello-ts")).unwrap();
    publish(&pkg, &repo, PublishOptions::default()).unwrap();
    sign_release(
        &repo,
        "hello-ts",
        "0.1.0",
        &publisher,
        Some(&format!("dns:{host}")),
    )
    .unwrap();
    Setup {
        tmp,
        host,
        repo,
        publisher,
    }
}

fn reviewer_list(dir: &Path, reviewer: &KeyPair, level: Level, hashes: &[String]) -> String {
    let mut list = List::new(reviewer, "Reviewers' Guild");
    let mut s = Statement::new(Kind::Vouch, "hello-ts", "0.1.0", hashes);
    s.level = level;
    s.note = "looked fine".into();
    list.add(s.sign(reviewer, None));
    let path = dir.join(format!(
        "list-{}.json",
        &reviewer.public_b64()[..6].replace('/', "_")
    ));
    std::fs::write(&path, list.to_json()).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn signed_release_and_a_vouch_from_a_followed_list() {
    let s = setup();
    let store = Store::new(s.tmp.path().join("cache"));
    let resolved = store
        .open(&format!("uw://{}/hello-ts", s.host), &|_| {})
        .unwrap();
    let subject = resolved.subject.clone().unwrap();
    assert!(subject.release_statement.is_some());

    // nobody followed yet: published by the domain, nothing more
    let trust = Trust::new(TrustStore::in_memory(), Default::default());
    let v = trust.check(&subject, &|_| {});
    assert_eq!(
        v.publisher.as_ref().map(|p| p.name.as_str()),
        Some(s.host.as_str()),
        "{v:?}"
    );
    assert!(!v.auto_grant);
    assert_eq!(
        trust.store().pins.get(&format!("dns:{}", s.host)),
        Some(&s.publisher.public_b64())
    );

    // follow a reviewer's list that vouches for this release
    let reviewer = KeyPair::generate();
    let list = reviewer_list(
        s.tmp.path(),
        &reviewer,
        Level::Reviewed,
        &subject.release_hashes,
    );
    {
        let mut st = trust.store();
        st.subscribe(&list);
        st.follow(&reviewer.identity(), "Reviewers' Guild", "list");
    }
    let v = trust.check(&subject, &|_| {});
    assert_eq!(v.vouches.len(), 1);
    assert!(v.auto_grant, "{v:?}");
    assert_eq!(
        v.summary(),
        format!(
            "published by {} · vouched for by Reviewers' Guild (you follow)",
            s.host
        )
    );

    // a second reviewer warns: no more auto-grant
    let critic = KeyPair::generate();
    let warn = reviewer_list(s.tmp.path(), &critic, Level::Warn, &subject.release_hashes);
    {
        let mut st = trust.store();
        st.subscribe(&warn);
        st.follow(&critic.identity(), "Critic", "list");
    }
    let v = trust.check(&subject, &|_| {});
    assert_eq!(v.warnings.len(), 1);
    assert!(!v.auto_grant);
}

#[test]
fn a_swapped_domain_key_is_caught() {
    let s = setup();
    let store = Store::new(s.tmp.path().join("cache"));
    let subject = store
        .open(&format!("uw://{}/hello-ts", s.host), &|_| {})
        .unwrap()
        .subject
        .unwrap();
    let trust = Trust::new(TrustStore::in_memory(), Default::default());
    assert!(trust.check(&subject, &|_| {}).publisher.is_some());

    // someone takes over the site, replaces the key and re-signs
    let intruder = KeyPair::generate();
    std::fs::write(
        s.repo.join(".well-known/underweb/key"),
        intruder.public_b64(),
    )
    .unwrap();
    sign_release(
        &s.repo,
        "hello-ts",
        "0.1.0",
        &intruder,
        Some(&format!("dns:{}", s.host)),
    )
    .unwrap();
    let subject = store
        .open(&format!("uw://{}/hello-ts", s.host), &|_| {})
        .unwrap()
        .subject
        .unwrap();
    let v = trust.check(&subject, &|_| {});
    assert!(v.publisher.is_none());
    assert!(v.publisher_problem.unwrap().contains("changed"));
}

#[test]
fn a_release_signature_by_the_wrong_key_is_rejected() {
    let s = setup();
    // signed as the domain, but with a key the domain doesn't list
    sign_release(
        &s.repo,
        "hello-ts",
        "0.1.0",
        &KeyPair::generate(),
        Some(&format!("dns:{}", s.host)),
    )
    .unwrap();
    let store = Store::new(s.tmp.path().join("cache"));
    let subject = store
        .open(&format!("uw://{}/hello-ts", s.host), &|_| {})
        .unwrap()
        .subject
        .unwrap();
    let v = Trust::new(TrustStore::in_memory(), Default::default()).check(&subject, &|_| {});
    assert!(v.publisher.is_none());
    assert!(
        v.summary().starts_with("bad publisher signature"),
        "{}",
        v.summary()
    );
}
