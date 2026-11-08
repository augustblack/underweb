//! `uw`: tools for package authors and readers.
//!
//! ```text
//! uw build   [DIR]                        build a native package for this machine
//! uw publish [DIR] --repo REPO [--source-only | --target-only] [--sign [--as ID]]
//!                                         add the package to a static repository
//! uw serve   REPO [--port 8000]           serve a repository for testing
//! uw open    ADDRESS                      resolve an address, print where it landed
//!
//! uw key                                  show (or create) your signing key
//! uw link-bluesky HANDLE                  declare your key on your Bluesky account
//! uw vouch ADDRESS [--level used|reviewed|warn] [--note TEXT]
//!          (--list FILE [--name LIST-NAME] | --bluesky HANDLE)
//!                                         sign a vouch (or warning) for a release
//! uw follow HANDLE|ID                     trust someone's vouches
//! uw follow --list URL|FILE               subscribe to a signed list
//! uw follow-bluesky HANDLE                follow everyone HANDLE follows on Bluesky
//! uw unfollow ID
//! uw trust                                show who you follow and the policy
//! uw check ADDRESS                        who published and vouches for a package
//! ```
//!
//! `--sign --as dns:example.org` signs a release as the domain the
//! repository is served from, and writes the key the domain must publish to
//! `REPO/.well-known/underweb/key`. Publishing to Bluesky needs an app
//! password in `UNDERWEB_APP_PASSWORD`; it is never stored.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use underweb_core::trust_types::lists::List;
use underweb_core::trust_types::{Identity, Kind, Level, Statement};
use underweb_core::{Store, Trust, sign_release};
use underweb_pkg::{Package, PublishOptions, publish};

fn main() {
    if let Err(e) = run(std::env::args().skip(1).collect()) {
        eprintln!("uw: {e:#}");
        std::process::exit(1);
    }
}

fn usage() -> ! {
    eprintln!("{}", USAGE);
    std::process::exit(2)
}

const USAGE: &str = "usage:
  uw build [DIR]
  uw publish [DIR] --repo REPO [--source-only|--target-only] [--sign [--as ID]]
  uw serve REPO [--port 8000]
  uw open ADDRESS
  uw key
  uw link-bluesky HANDLE
  uw vouch ADDRESS [--level used|reviewed|warn] [--note TEXT] (--list FILE [--name NAME] | --bluesky HANDLE)
  uw follow HANDLE|ID | uw follow --list URL|FILE | uw follow-bluesky HANDLE | uw unfollow ID
  uw trust
  uw check ADDRESS";

#[derive(Default)]
struct Opts {
    positional: Vec<String>,
    repo: Option<PathBuf>,
    port: u16,
    publish: PublishOptions,
    sign: bool,
    signer: Option<String>,
    level: Option<String>,
    note: String,
    list: Option<String>,
    name: Option<String>,
    bluesky: Option<String>,
}

fn parse(rest: &[String]) -> Result<Opts> {
    let mut o = Opts {
        port: 8000,
        publish: PublishOptions::default(),
        ..Default::default()
    };
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .cloned()
                .with_context(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--repo" => o.repo = Some(PathBuf::from(val()?)),
            "--port" => o.port = val()?.parse()?,
            "--source-only" => o.publish.target = false,
            "--target-only" => o.publish.source = false,
            "--sign" => o.sign = true,
            "--as" => o.signer = Some(val()?),
            "--level" => o.level = Some(val()?),
            "--note" => o.note = val()?,
            "--list" => o.list = Some(val()?),
            "--name" => o.name = Some(val()?),
            "--bluesky" => o.bluesky = Some(val()?),
            "-h" | "--help" => usage(),
            s if s.starts_with("--") => bail!("unknown option {s}"),
            s => o.positional.push(s.to_string()),
        }
    }
    Ok(o)
}

fn app_password() -> Result<String> {
    std::env::var("UNDERWEB_APP_PASSWORD").map_err(|_| {
        anyhow::anyhow!(
            "set UNDERWEB_APP_PASSWORD to a Bluesky app password (Settings → Privacy and security → App passwords)"
        )
    })
}

fn run(args: Vec<String>) -> Result<()> {
    let Some((cmd, rest)) = args.split_first() else {
        usage()
    };
    let o = parse(rest)?;
    let first = || o.positional.first().cloned();
    let dir = || PathBuf::from(first().unwrap_or_else(|| ".".into()));
    let progress = |l: &str| eprintln!("  {l}");
    match cmd.as_str() {
        "build" => {
            let pkg = Package::open(dir())?;
            Store::new(Store::default_root()).build(&pkg, &progress)?;
            println!("{}", pkg.built_library().display());
        }
        "publish" => {
            let repo = o.repo.clone().context("publish needs --repo DIR")?;
            let mut pkg = Package::open(dir())?;
            if o.publish.target && pkg.needs_build() {
                eprintln!("building {} for this machine first", pkg.name());
                Store::new(Store::default_root()).build(&pkg, &progress)?;
                pkg = Package::open(&pkg.dir)?;
            }
            for f in publish(&pkg, &repo, o.publish)? {
                println!("{}", f.display());
            }
            if o.sign {
                let key = Trust::key(None)?;
                let s = sign_release(&repo, pkg.name(), pkg.version(), &key, o.signer.as_deref())?;
                println!(
                    "signed {} {} as {}",
                    pkg.name(),
                    pkg.version(),
                    Identity::short(&s.signer)
                );
                if let Some(domain) = s.signer.strip_prefix("dns:") {
                    let f = repo.join(".well-known/underweb/key");
                    let mut keys = std::fs::read_to_string(&f).unwrap_or_default();
                    if !keys.lines().any(|l| l.trim() == key.public_b64()) {
                        keys.push_str(&format!("{}\n", key.public_b64()));
                        std::fs::write(&f, keys)?;
                    }
                    println!(
                        "{} must be served at https://{domain}/.well-known/underweb/key",
                        f.display()
                    );
                }
            }
            println!(
                "\npackage address once {} is served: uw://<domain>/{}",
                repo.display(),
                pkg.name()
            );
        }
        "serve" => serve(&PathBuf::from(first().context("serve REPO")?), o.port)?,
        "open" => {
            let r = Store::new(Store::default_root())
                .open(&first().context("open ADDRESS")?, &progress)?;
            println!("{}", r.dir.display());
        }
        "key" => {
            let key = Trust::key(None)?;
            println!("public key: {}", key.public_b64());
            println!("identity:   {}", key.identity());
            println!("stored in   {}", Trust::default_dir().join("key").display());
        }
        "link-bluesky" => {
            let handle = first().context("link-bluesky HANDLE")?;
            let trust = Trust::open(None)?;
            let session = trust.atproto.login(&handle, &app_password()?)?;
            trust
                .atproto
                .publish_key(&session, &Trust::key(None)?.public_b64())?;
            println!(
                "{} ({}) now vouches with this machine's key",
                session.handle, session.did
            );
        }
        "vouch" => vouch(&o)?,
        "follow" => {
            let trust = Trust::open(None)?;
            if let Some(list) = &o.list {
                let l = trust.fetch_list(list)?;
                let name = if l.name.is_empty() {
                    Identity::short(&l.owner)
                } else {
                    l.name.clone()
                };
                let mut st = trust.store();
                st.subscribe(list);
                st.follow(&l.owner, &name, "list");
                st.save()?;
                println!(
                    "following the list {name} ({} statements)",
                    l.statements.len()
                );
            } else {
                let who = first().context("follow HANDLE|ID")?;
                let (id, name, net) = if who.starts_with("key:") || who.starts_with("dns:") {
                    (
                        who.clone(),
                        Identity::short(&who),
                        if who.starts_with("key:") {
                            "key"
                        } else {
                            "web"
                        },
                    )
                } else {
                    let did = trust.atproto.resolve(&who)?;
                    let handle = trust.atproto.did_info(&did)?.handle.unwrap_or(who.clone());
                    (did, handle, "Bluesky")
                };
                let mut st = trust.store();
                st.follow(&id, &name, net);
                st.save()?;
                println!("following {name}");
            }
        }
        "follow-bluesky" => {
            let who = first().context("follow-bluesky HANDLE")?;
            let trust = Trust::open(None)?;
            let follows = trust.atproto.follows_of(&who)?;
            let mut st = trust.store();
            for (did, handle) in &follows {
                st.follow(did, handle, "Bluesky");
            }
            st.save()?;
            println!(
                "now following {} accounts that {who} follows",
                follows.len()
            );
        }
        "unfollow" => {
            let trust = Trust::open(None)?;
            let mut st = trust.store();
            st.unfollow(&first().context("unfollow ID")?);
            st.save()?;
        }
        "trust" => {
            let trust = Trust::open(None)?;
            let st = trust.store();
            println!(
                "grant declared permissions without asking when {} or more people you follow vouch: {}",
                st.policy.min_vouches,
                if st.policy.auto_grant { "yes" } else { "no" }
            );
            println!("following {}:", st.follows.len());
            for f in &st.follows {
                println!("  {} ({}) {}", f.name, f.network, f.identity);
            }
            for l in &st.lists {
                println!("  list {l}");
            }
        }
        "check" => {
            let addr = first().context("check ADDRESS")?;
            let r = Store::new(Store::default_root()).open(&addr, &progress)?;
            let subject = r
                .subject
                .context("local packages have no publisher or vouches")?;
            let v = Trust::open(None)?.check(&subject, &progress);
            println!("{}", v.summary());
            for (s, level) in &v.vouches {
                println!("  vouch: {} ({}) {level:?}", s.name, s.network);
            }
            for (s, note) in &v.warnings {
                println!("  WARNING: {} ({}): {note}", s.name, s.network);
            }
            for r in &v.rejected {
                println!("  ignored: {r}");
            }
            println!(
                "{}",
                if v.auto_grant {
                    "declared permissions: granted without asking"
                } else {
                    "declared permissions: asked for"
                }
            );
        }
        _ => usage(),
    }
    Ok(())
}

fn vouch(o: &Opts) -> Result<()> {
    let addr = o.positional.first().context("vouch ADDRESS")?;
    let level = match o.level.as_deref().unwrap_or("used") {
        "used" => Level::Used,
        "reviewed" => Level::Reviewed,
        "warn" => Level::Warn,
        other => bail!("--level is used, reviewed or warn, not {other:?}"),
    };
    let r = Store::new(Store::default_root()).open(addr, &|l| eprintln!("  {l}"))?;
    let subject = r
        .subject
        .context("vouch for uw:// packages (local ones have no release)")?;
    let key = Trust::key(None)?;
    let mut s = Statement::new(
        Kind::Vouch,
        &subject.package,
        &subject.version,
        &subject.release_hashes,
    );
    s.level = level;
    s.note = o.note.clone();
    match (&o.list, &o.bluesky) {
        (Some(file), None) => {
            let mut list = match std::fs::read_to_string(file) {
                Ok(t) => List::parse(&t)?,
                Err(_) => List::new(&key, ""),
            };
            if let Some(name) = &o.name {
                list.name = name.clone();
            }
            if list.owner != key.identity() {
                bail!(
                    "{file} belongs to {}, not your key",
                    Identity::short(&list.owner)
                );
            }
            list.add(s.sign(&key, None));
            std::fs::write(file, list.to_json())?;
            println!("added to {file}; others can follow it with: uw follow --list <its URL>");
        }
        (None, Some(handle)) => {
            let trust = Trust::open(None)?;
            let session = trust.atproto.login(handle, &app_password()?)?;
            let uri = trust
                .atproto
                .publish_statement(&session, &s.sign(&key, Some(&session.did)))?;
            println!("published {uri}");
        }
        _ => bail!("say where: --list FILE or --bluesky HANDLE"),
    }
    Ok(())
}

fn serve(repo: &Path, port: u16) -> Result<()> {
    let repo = repo
        .canonicalize()
        .with_context(|| format!("{}", repo.display()))?;
    let server =
        tiny_http::Server::http(("127.0.0.1", port)).map_err(|e| anyhow::anyhow!("{e}"))?;
    let index_dir = repo.join(".well-known/underweb");
    println!("serving {} on http://127.0.0.1:{port}", repo.display());
    for e in std::fs::read_dir(&index_dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        if let Some(name) = e.file_name().to_string_lossy().strip_suffix(".toml") {
            println!("  uw://localhost:{port}/{name}");
        }
    }
    for req in server.incoming_requests() {
        let rel = req
            .url()
            .split('?')
            .next()
            .unwrap_or("")
            .trim_start_matches('/')
            .to_string();
        let path = repo.join(&rel);
        let ok = !rel.split('/').any(|c| c == "..") && path.starts_with(&repo) && path.is_file();
        let resp = if ok {
            tiny_http::Response::from_data(std::fs::read(&path)?).boxed()
        } else {
            tiny_http::Response::empty(404).boxed()
        };
        eprintln!("{} {}", if ok { 200 } else { 404 }, req.url());
        let _ = req.respond(resp);
    }
    Ok(())
}
