//! Development tasks: `cargo xtask <task>`.
//!
//! - `examples`: build the native example packages and copy each library
//!   into `examples/<pkg>/dist/<target>/`, where its manifest points.
//! - `header`: regenerate `include/underweb.h` from `underweb-abi`.
//! - `test`: `examples`, then `cargo test --workspace`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

const NATIVE_RUST_EXAMPLES: &[&str] = &["hello-rust", "probe-rust"];
const NATIVE_C_EXAMPLES: &[&str] = &["hello-c"];

fn main() -> Result<()> {
    let task = std::env::args().nth(1).unwrap_or_default();
    match task.as_str() {
        "examples" => examples(),
        "header" => header(),
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
            eprintln!("usage: cargo xtask <examples|header|test>");
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
    cmd.args(["build", "-p", "underweb-runner"])
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
        let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
        let status = Command::new(&cc)
            .args(["-shared", "-fPIC", "-Wall", "-O2", "-I"])
            .arg(root.join("include"))
            .arg(dir.join(format!("{}.c", e.replace('-', "_"))))
            .arg("-o")
            .arg(dist.join(&lib))
            .status()
            .with_context(|| format!("running {cc}"))?;
        if !status.success() {
            bail!("{cc} failed for {e}");
        }
        println!("{e}: dist/{triple}/{lib}");
    }
    Ok(())
}

fn header() -> Result<()> {
    let root = root();
    let config = cbindgen::Config::from_file(root.join("crates/underweb-abi/cbindgen.toml"))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    cbindgen::Builder::new()
        .with_crate(root.join("crates/underweb-abi"))
        .with_config(config)
        .generate()
        .context("cbindgen")?
        .write_to_file(root.join("include/underweb.h"));
    println!("wrote include/underweb.h");
    Ok(())
}
