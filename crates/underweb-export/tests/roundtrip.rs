//! Export a page in each language, open the export, and check it builds
//! the same scene. Needs the runner (`cargo xtask examples`); the Rust
//! export is compiled by the browser's sandboxed builder, so it also needs
//! a Rust toolchain and takes a while.

use std::path::{Path, PathBuf};

use underweb_core::scene::SceneDoc;
use underweb_core::{Guest, Package, PermissionPolicy, SpawnOptions, Store};
use underweb_export::Lang;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn scratch(name: &str) -> PathBuf {
    let d = root().join("target/test-export").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn read(dir: &Path) -> SceneDoc {
    assert!(
        underweb_core::runner_path().exists(),
        "runner not built: run `cargo xtask examples` first"
    );
    Guest::read_scene(
        dir,
        SpawnOptions {
            permissions: PermissionPolicy::DenyAll,
            remember_grants: false,
            data_dir: Some(std::env::temp_dir().join("underweb-test-data/export")),
            mixer: Some(underweb_core::Mixer::new(48000, 2)),
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("{}: {e:#}", dir.display()))
}

/// Ids differ between runs; everything else must match.
fn normalised(mut d: SceneDoc) -> SceneDoc {
    for (i, l) in d.layouts.iter_mut().enumerate() {
        l.id = i as u32;
        for (j, s) in l.shapes.iter_mut().enumerate() {
            s.id = j as u32;
        }
    }
    d
}

fn round_trip(example: &str, langs: &[Lang]) {
    let original = read(&root().join("examples").join(example));
    assert!(!original.layouts.is_empty(), "{example} built nothing");
    for &lang in langs {
        let out = scratch(&format!("{example}-{}", lang.label().to_lowercase()));
        let notes = underweb_export::write(
            &original,
            lang,
            "exported",
            Some(&root().join("examples").join(example)),
            &out,
        )
        .unwrap();
        assert!(notes.is_empty(), "{lang:?}: {notes:?}");
        if matches!(lang, Lang::C | Lang::Rust) {
            let store = Store::new(scratch(&format!("store-{example}-{lang:?}")));
            let pkg = Package::open(&out).unwrap();
            store
                .build(&pkg, &|l| eprintln!("  {l}"))
                .unwrap_or_else(|e| {
                    let src = std::fs::read_to_string(out.join(lang.source_file())).unwrap();
                    panic!("{lang:?} export doesn't build: {e:#}\n{src}")
                });
        }
        let again = read(&out);
        assert_eq!(
            normalised(again),
            normalised(original.clone()),
            "{lang:?} export of {example} differs; source:\n{}",
            std::fs::read_to_string(out.join(lang.source_file())).unwrap()
        );
    }
}

#[test]
fn shapes_survive_export_to_js_ts_and_c() {
    round_trip("demo-03-shapes", &[Lang::Js, Lang::Ts, Lang::C]);
}

#[test]
fn images_and_backgrounds_survive_export() {
    round_trip("demo-06-image", &[Lang::Ts, Lang::C]);
}

#[test]
fn shapes_survive_export_to_rust() {
    round_trip("demo-03-shapes", &[Lang::Rust]);
}
