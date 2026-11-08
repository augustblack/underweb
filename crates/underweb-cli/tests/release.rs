//! `uw sign-release` / `uw verify-release` on files, as the release
//! workflow uses them.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output};

fn uw(args: &[&OsStr], home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_uw"))
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .output()
        .unwrap()
}

#[test]
fn sign_then_verify_and_detect_tampering() {
    let dir = std::env::temp_dir().join(format!("uw-release-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("a.zip"), dir.join("b.tar.gz"));
    std::fs::write(&a, b"archive a").unwrap();
    std::fs::write(&b, b"archive b").unwrap();
    let statement = dir.join("release.json");
    let out = uw(
        &[
            "sign-release".as_ref(),
            a.as_os_str(),
            b.as_os_str(),
            "--name".as_ref(),
            "underweb".as_ref(),
            "--version".as_ref(),
            "9.9.9".as_ref(),
            "--out".as_ref(),
            statement.as_os_str(),
        ],
        &dir,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&statement).unwrap()).unwrap();
    let key = json["key"].as_str().unwrap().to_string();

    let verify = |files: &[&Path], key: &str| {
        let mut args: Vec<&OsStr> = vec!["verify-release".as_ref(), statement.as_os_str()];
        args.extend(files.iter().map(|f| f.as_os_str()));
        args.extend([OsStr::new("--key"), OsStr::new(key)]);
        uw(&args, &dir)
    };
    let ok = verify(&[&a, &b], &key);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert!(String::from_utf8_lossy(&ok.stdout).contains("underweb 9.9.9 signed by key:"));

    // a different key is refused
    let other = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    assert!(!verify(&[&a], other).status.success());

    // so is a changed file
    std::fs::write(&b, b"archive b, with something extra").unwrap();
    let bad = verify(&[&b], &key);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("is not part of underweb 9.9.9"));
    let _ = std::fs::remove_dir_all(&dir);
}
