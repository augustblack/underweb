//! Compile tests/embed.c against the generated header and the cdylib, and
//! run it on an example package.

use std::path::Path;
use std::process::Command;

#[test]
fn c_program_embeds_the_engine() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let target = root.join("target/debug");
    let lib = target.join(if cfg!(target_os = "macos") {
        "libunderweb_engine.dylib"
    } else {
        "libunderweb_engine.so"
    });
    assert!(
        lib.exists(),
        "build the cdylib first (cargo build -p underweb-capi)"
    );
    assert!(
        target.join("underweb-runner").exists(),
        "run `cargo xtask examples` first"
    );

    let tmp = tempfile_dir();
    let exe = tmp.join("embed");
    let status = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/embed.c"))
        .arg("-I")
        .arg(root.join("include"))
        .arg("-L")
        .arg(&target)
        .arg("-lunderweb_engine")
        .arg(format!("-Wl,-rpath,{}", target.display()))
        .arg("-o")
        .arg(&exe)
        .status()
        .unwrap();
    assert!(status.success());

    let out = Command::new(&exe)
        .arg(root.join("examples/hello-c"))
        .env("UNDERWEB_RUNNER", target.join("underweb-runner"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("title C circles"), "{stdout}");
    assert!(
        stdout.contains("frame 320x240 sandboxed=1 pixel=51,51,230"),
        "{stdout}"
    );
}

fn tempfile_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("underweb-embed-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}
