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
    } else if cfg!(windows) {
        "underweb_engine.dll"
    } else {
        "libunderweb_engine.so"
    });
    assert!(
        lib.exists(),
        "build the cdylib first (cargo build -p underweb-capi)"
    );
    let runner = target.join(if cfg!(windows) {
        "underweb-runner.exe"
    } else {
        "underweb-runner"
    });
    assert!(runner.exists(), "run `cargo xtask examples` first");

    let tmp = tempfile_dir();
    let exe = tmp.join(if cfg!(windows) { "embed.exe" } else { "embed" });
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/embed.c");
    let status = match std::env::var("CC") {
        Err(_) if cfg!(windows) => {
            let mut fe = std::ffi::OsString::from("/Fe:");
            fe.push(&exe);
            let mut fo = std::ffi::OsString::from("/Fo");
            fo.push(tmp.as_os_str());
            fo.push("\\");
            Command::new("cl.exe")
                .arg("/nologo")
                .arg(&src)
                .arg("/I")
                .arg(root.join("include"))
                .arg(fo)
                .arg(fe)
                // the import library cargo writes next to the DLL
                .arg(target.join("underweb_engine.dll.lib"))
                .status()
                .unwrap()
        }
        cc => Command::new(cc.unwrap_or_else(|_| "cc".into()))
            .arg(&src)
            .arg("-I")
            .arg(root.join("include"))
            .arg("-L")
            .arg(&target)
            .arg("-lunderweb_engine")
            .arg(format!("-Wl,-rpath,{}", target.display()))
            .arg("-o")
            .arg(&exe)
            .status()
            .unwrap(),
    };
    assert!(status.success());

    // Windows finds the engine DLL through PATH
    let path = std::env::join_paths(std::iter::once(target.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let out = Command::new(&exe)
        .arg(root.join("examples/hello-c"))
        .env("UNDERWEB_RUNNER", &runner)
        .env("PATH", path)
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
