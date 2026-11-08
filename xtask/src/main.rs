//! Development tasks: `cargo xtask <task>`.
//!
//! - `examples`: build the native example packages and copy each library
//!   into `examples/<pkg>/dist/<target>/`, where its manifest points.
//! - `header`: regenerate `include/underweb.h` from `underweb-abi`.
//! - `test`: `examples`, then `cargo test --workspace`.
//! - `dist`: `examples`, then a release archive in `target/dist/` (see
//!   dist.rs).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

mod dist;

const NATIVE_RUST_EXAMPLES: &[&str] = &[
    "hello-rust",
    "demo-08-draw",
    "probe-rust",
    "probe-net-rust",
    "probe-audio-rust",
    "probe-video-rust",
];
const NATIVE_C_EXAMPLES: &[&str] = &["hello-c"];

fn main() -> Result<()> {
    let task = std::env::args().nth(1).unwrap_or_default();
    match task.as_str() {
        "examples" => examples(),
        "header" => header(),
        "dist" => {
            examples()?;
            dist::dist(&root(), &host_triple()?)
        }
        "test" => {
            examples()?;
            let ok = Command::new(env!("CARGO"))
                .args(["test", "--workspace"])
                .current_dir(root())
                .status()?;
            if !ok.success() {
                bail!("tests failed");
            }
            Ok(())
        }
        _ => {
            eprintln!("usage: cargo xtask <examples|header|test|dist>");
            std::process::exit(2)
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn host_triple() -> Result<String> {
    let out = Command::new("rustc").arg("-vV").output()?;
    let text = String::from_utf8(out.stdout)?;
    text.lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(str::to_string)
        .context("rustc -vV has no host line")
}

fn dylib_name(stem: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("lib{stem}.dylib")
    } else if cfg!(windows) {
        format!("{stem}.dll")
    } else {
        format!("lib{stem}.so")
    }
}

fn examples() -> Result<()> {
    let root = root();
    let triple = host_triple()?;
    let mut cmd = Command::new(env!("CARGO"));
    // the runner too: tests and the app spawn it from target/debug
    cmd.args([
        "build",
        "-p",
        "underweb-runner",
        "-p",
        "underweb-capi",
        "-p",
        "underweb-cli",
        "-p",
        "underweb-media",
    ])
    .current_dir(&root);
    for e in NATIVE_RUST_EXAMPLES {
        cmd.args(["-p", e]);
    }
    if !cmd.status()?.success() {
        bail!("cargo build failed");
    }
    for e in NATIVE_RUST_EXAMPLES {
        let lib = dylib_name(&e.replace('-', "_"));
        let dist = root.join("examples").join(e).join("dist").join(&triple);
        std::fs::create_dir_all(&dist)?;
        std::fs::copy(root.join("target/debug").join(&lib), dist.join(&lib))
            .with_context(|| format!("copying {lib}"))?;
        println!("{e}: dist/{triple}/{lib}");
    }
    for e in NATIVE_C_EXAMPLES {
        let dir = root.join("examples").join(e);
        let lib = dylib_name(&e.replace('-', "_"));
        let dist = dir.join("dist").join(&triple);
        std::fs::create_dir_all(&dist)?;
        let src = dir.join(format!("{}.c", e.replace('-', "_")));
        let out = dist.join(&lib);
        let status = match std::env::var("CC") {
            Err(_) if cfg!(windows) => {
                let mut fe = std::ffi::OsString::from("/Fe:");
                fe.push(&out);
                let mut fo = std::ffi::OsString::from("/Fo");
                fo.push(dist.as_os_str());
                fo.push("\\");
                Command::new("cl.exe")
                    .args(["/nologo", "/LD", "/O2", "/W3", "/I"])
                    .arg(root.join("include"))
                    .arg(&src)
                    .arg(fo)
                    .arg(fe)
                    .status()
                    .context("running cl.exe (use a Visual Studio developer prompt)")?
            }
            cc => {
                let cc = cc.unwrap_or_else(|_| "cc".into());
                Command::new(&cc)
                    .args(["-shared", "-fPIC", "-Wall", "-O2", "-I"])
                    .arg(root.join("include"))
                    .arg(&src)
                    .arg("-o")
                    .arg(&out)
                    .status()
                    .with_context(|| format!("running {cc}"))?
            }
        };
        if !status.success() {
            bail!("compiling {e} failed");
        }
        println!("{e}: dist/{triple}/{lib}");
    }
    Ok(())
}

fn header() -> Result<()> {
    let root = root();
    for (krate, out) in [
        ("underweb-abi", "underweb.h"),
        ("underweb-capi", "underweb_engine.h"),
    ] {
        let dir = root.join("crates").join(krate);
        let config = cbindgen::Config::from_file(dir.join("cbindgen.toml"))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        cbindgen::Builder::new()
            .with_crate(&dir)
            .with_config(config)
            .generate()
            .context("cbindgen")?
            .write_to_file(root.join("include").join(out));
        println!("wrote include/{out}");
    }
    Ok(())
}
