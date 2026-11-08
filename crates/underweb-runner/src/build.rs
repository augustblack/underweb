//! `underweb-runner --build`: build a native package from source, inside
//! the sandbox.
//!
//! ```text
//! underweb-runner --build --package DIR --work DIR --sdk DIR
//!                 [--read DIR]... [--write DIR]... [--no-sandbox]
//! ```
//!
//! Building runs the package's own code (Rust build scripts and proc
//! macros), so it is locked down like a runner except that it may start
//! programs (cargo, rustc, cc), which inherit the restrictions: no network,
//! and only the package, the SDK, the toolchain and the work directory are
//! visible. Dependencies were fetched beforehand by the host
//! (`cargo fetch`, which runs no package code).
//!
//! The library ends up at `Package::built_library()`. Progress goes to
//! stderr; the exit status says whether it worked.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use underweb_pkg::{Kind, Package, library_file_name};

struct BuildArgs {
    package: PathBuf,
    work: PathBuf,
    sdk: PathBuf,
    read: Vec<PathBuf>,
    write: Vec<PathBuf>,
    sandbox: bool,
}

fn parse(mut it: impl Iterator<Item = String>) -> Result<BuildArgs> {
    let (mut package, mut work, mut sdk) = (None, None, None);
    let (mut read, mut write) = (Vec::new(), Vec::new());
    let mut sandbox = true;
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--package" => package = Some(PathBuf::from(val()?)),
            "--work" => work = Some(PathBuf::from(val()?)),
            "--sdk" => sdk = Some(PathBuf::from(val()?)),
            "--read" => read.push(PathBuf::from(val()?)),
            "--write" => write.push(PathBuf::from(val()?)),
            "--no-sandbox" => sandbox = false,
            other => bail!("unknown argument {other:?}"),
        }
    }
    Ok(BuildArgs {
        package: package.context("missing --package")?,
        work: work.context("missing --work")?,
        sdk: sdk.context("missing --sdk")?,
        read,
        write,
        sandbox,
    })
}

pub fn main(args: impl Iterator<Item = String>) -> ! {
    match run(args) {
        Ok(lib) => {
            eprintln!("built {}", lib.display());
            std::process::exit(0)
        }
        Err(e) => {
            eprintln!("build failed: {e:#}");
            std::process::exit(1)
        }
    }
}

fn run(args: impl Iterator<Item = String>) -> Result<PathBuf> {
    let args = parse(args)?;
    let pkg = Package::open(&args.package)?;
    if !pkg.kind().is_native() {
        bail!("{} is not a native package", pkg.name());
    }
    std::fs::create_dir_all(&args.work)?;
    if args.sandbox {
        let mut read_only = vec![args.sdk.clone()];
        read_only.extend(args.read.iter().cloned());
        let mut read_write = vec![pkg.dir.clone(), args.work.clone()];
        read_write.extend(args.write.iter().cloned());
        underweb_sandbox::lockdown(&underweb_sandbox::Policy {
            read_only,
            read_write,
            allow_exec: true,
        })?;
        eprintln!("sandboxed build of {} {}", pkg.name(), pkg.version());
    }
    let out = pkg.built_library();
    std::fs::create_dir_all(out.parent().unwrap())?;
    match pkg.kind() {
        Kind::C => build_c(&pkg, &args.sdk, &args.work, &out)?,
        Kind::Rust => build_rust(&pkg, &args.sdk, &args.work, &out)?,
        _ => unreachable!(),
    }
    Ok(out)
}

fn run_cmd(cmd: &mut Command) -> Result<()> {
    eprintln!("$ {cmd:?}");
    let status = cmd
        .status()
        .with_context(|| format!("starting {:?}", cmd.get_program()))?;
    if !status.success() {
        bail!("{:?} failed ({status})", cmd.get_program());
    }
    Ok(())
}

fn build_c(pkg: &Package, sdk: &Path, work: &Path, out: &Path) -> Result<()> {
    let b = &pkg.manifest.build;
    if b.sources.is_empty() {
        bail!("no [build] sources in underweb.toml");
    }
    match std::env::var("CC") {
        // MSVC: a DLL, objects kept out of the package
        Err(_) if cfg!(windows) => {
            let mut fo = work.as_os_str().to_owned();
            fo.push("\\");
            let mut fe = std::ffi::OsString::from("/Fe:");
            fe.push(out);
            let mut objs = std::ffi::OsString::from("/Fo");
            objs.push(fo);
            run_cmd(
                Command::new("cl.exe")
                    .current_dir(&pkg.dir)
                    .args(["/nologo", "/LD", "/O2", "/W3", "/I"])
                    .arg(sdk.join("include"))
                    .args(&b.cflags)
                    .args(&b.sources)
                    .arg(objs)
                    .arg(fe),
            )
        }
        cc => run_cmd(
            Command::new(cc.unwrap_or_else(|_| "cc".into()))
                .current_dir(&pkg.dir)
                .args(["-shared", "-fPIC", "-O2", "-Wall", "-I"])
                .arg(sdk.join("include"))
                .args(&b.cflags)
                .args(&b.sources)
                .arg("-o")
                .arg(out),
        ),
    }
}

fn build_rust(pkg: &Package, sdk: &Path, work: &Path, out: &Path) -> Result<()> {
    let target = work.join("target");
    run_cmd(
        Command::new("cargo")
            .current_dir(&pkg.dir)
            .args(["build", "--release", "--lib", "--offline", "--target-dir"])
            .arg(&target)
            .args(sdk_patch(sdk)),
    )?;
    let built = target.join("release").join(library_file_name(pkg.name()));
    std::fs::copy(&built, out).with_context(|| format!("copying {}", built.display()))?;
    Ok(())
}

/// Packages depend on `underweb = "0.x"`; point that at the SDK the
/// browser ships instead of crates.io. (Same flags as the host's
/// `cargo fetch`.)
fn sdk_patch(sdk: &Path) -> [String; 2] {
    let path = sdk.join("crates/underweb");
    [
        "--config".into(),
        format!("patch.crates-io.underweb.path={:?}", path.to_string_lossy()),
    ]
}
