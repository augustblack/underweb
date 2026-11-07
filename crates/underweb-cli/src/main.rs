//! `uw`: tools for package authors.
//!
//! ```text
//! uw build   [DIR]                         build a native package for this machine
//! uw publish [DIR] --repo REPO [--source-only | --target-only]
//!                                          add the package to a static repository
//! uw serve   REPO [--port 8000]            serve a repository for testing
//! uw open    ADDRESS                       resolve an address, print where it landed
//! ```
//!
//! A repository is a directory you upload as-is to any web server; its
//! packages are then at `uw://<that domain>/<name>`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use underweb_core::Store;
use underweb_pkg::{Package, PublishOptions, publish};

fn main() {
    if let Err(e) = run(std::env::args().skip(1).collect()) {
        eprintln!("uw: {e:#}");
        std::process::exit(1);
    }
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  uw build [DIR]\n  uw publish [DIR] --repo REPO [--source-only|--target-only]\n  \
         uw serve REPO [--port 8000]\n  uw open ADDRESS"
    );
    std::process::exit(2)
}

fn run(args: Vec<String>) -> Result<()> {
    let Some((cmd, rest)) = args.split_first() else {
        usage()
    };
    let mut positional = Vec::new();
    let (mut repo, mut port) = (None, 8000u16);
    let mut opts = PublishOptions::default();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--repo" => repo = Some(PathBuf::from(it.next().context("--repo DIR")?)),
            "--port" => port = it.next().context("--port N")?.parse()?,
            "--source-only" => opts.target = false,
            "--target-only" => opts.source = false,
            "-h" | "--help" => usage(),
            s if s.starts_with("--") => bail!("unknown option {s}"),
            s => positional.push(s.to_string()),
        }
    }
    let dir = || PathBuf::from(positional.first().map_or(".", String::as_str));
    let progress = |l: &str| eprintln!("  {l}");
    match cmd.as_str() {
        "build" => {
            let pkg = Package::open(dir())?;
            Store::new(Store::default_root()).build(&pkg, &progress)?;
            println!("{}", pkg.built_library().display());
        }
        "publish" => {
            let repo = repo.context("publish needs --repo DIR")?;
            let mut pkg = Package::open(dir())?;
            if opts.target && pkg.needs_build() {
                eprintln!("building {} for this machine first", pkg.name());
                Store::new(Store::default_root()).build(&pkg, &progress)?;
                pkg = Package::open(&pkg.dir)?;
            }
            for f in publish(&pkg, &repo, opts)? {
                println!("{}", f.display());
            }
            println!(
                "\npackage address once {} is served: uw://<domain>/{}",
                repo.display(),
                pkg.name()
            );
        }
        "serve" => serve(
            &PathBuf::from(positional.first().context("serve REPO")?),
            port,
        )?,
        "open" => {
            let addr = positional.first().context("open ADDRESS")?;
            let r = Store::new(Store::default_root()).open(addr, &progress)?;
            println!("{}", r.dir.display());
        }
        _ => usage(),
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
