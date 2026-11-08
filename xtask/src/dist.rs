//! `cargo xtask dist`: a release archive for this machine.
//!
//! ```text
//! target/dist/underweb-<version>-<triple>.{zip,tar.gz}
//! ```
//!
//! | OS | Layout | Libraries |
//! |---|---|---|
//! | macOS | `Underweb.app` (programs in `Contents/MacOS`, SDK and examples in `Contents/Resources`) | Homebrew dylibs copied into `Contents/Frameworks`, GStreamer plugins into `Contents/PlugIns`, install names rewritten |
//! | Windows | `bin\` (programs and every DLL they load), `lib\gstreamer-1.0\`, `share\underweb\` | from gvsbuild's and GStreamer's `bin` on `PATH` |
//! | Linux | `bin/`, `lib/`, `share/underweb/` | the system's (see `DEPENDENCIES.txt`) |
//!
//! Signing is optional and comes from the environment, so the same
//! command serves a laptop and the release workflow:
//!
//! - macOS: `UNDERWEB_MACOS_IDENTITY` (a "Developer ID Application"
//!   identity in the keychain; ad-hoc signing otherwise), and for
//!   notarization `UNDERWEB_NOTARY_PROFILE` (a `notarytool
//!   store-credentials` profile) or `APPLE_ID` + `APPLE_TEAM_ID` +
//!   `APPLE_APP_PASSWORD`.
//! - Windows: `UNDERWEB_WINDOWS_CERT` (a .pfx) and
//!   `UNDERWEB_WINDOWS_CERT_PASSWORD`, used with `signtool`.
//!
//! The archive itself is signed afterwards with `uw sign-release`.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

const PROGRAMS: &[&str] = &["underweb", "underweb-runner", "underweb-media", "uw"];

/// GStreamer plugins the media service uses (see underweb-media), by
/// plugin file stem. Missing ones are reported, not fatal: a platform
/// lacks some (applemedia is macOS only).
const GST_PLUGINS: &[&str] = &[
    "coreelements",
    "app",
    "playback",
    "typefindfunctions",
    "autodetect",
    "videoconvertscale",
    "videoconvert",
    "videoscale",
    "videorate",
    "videotestsrc",
    "audioconvert",
    "audioresample",
    "audiotestsrc",
    "audioparsers",
    "volume",
    "isomp4",
    "matroska",
    "ogg",
    "theora",
    "vorbis",
    "opus",
    "vpx",
    "libav",
    "videoparsersbad",
    "id3demux",
    "wavparse",
    "jpeg",
    "png",
    "applemedia",
    "mediafoundation",
    "d3d11",
    "directsound",
    "wasapi",
    "v4l2",
    "shout2",
];

struct Layout {
    /// What gets archived.
    top: PathBuf,
    bin: PathBuf,
    /// Shared libraries we bundle (Frameworks on macOS, bin on Windows).
    libs: PathBuf,
    plugins: PathBuf,
    /// The engine library and its C header.
    engine: PathBuf,
    include: PathBuf,
    share: PathBuf,
}

pub fn dist(root: &Path, triple: &str) -> Result<()> {
    let version = workspace_version(root)?;
    build(root)?;
    let release = root.join("target").join("dist-release");
    let out = root.join("target").join("dist");
    let name = format!("underweb-{version}-{triple}");
    let stage = out.join(&name);
    if stage.exists() {
        std::fs::remove_dir_all(&stage)?;
    }
    let l = layout(&stage);
    for d in [&l.bin, &l.libs, &l.engine, &l.include, &l.share] {
        std::fs::create_dir_all(d)?;
    }

    for p in PROGRAMS {
        let f = exe(p);
        copy(&release.join(&f), &l.bin.join(&f))?;
    }
    let engine = crate::dylib_name("underweb_engine");
    copy(&release.join(&engine), &l.engine.join(&engine))?;
    if cfg!(windows) {
        let implib = format!("{engine}.lib");
        copy(&release.join(&implib), &l.engine.join(&implib))?;
    }
    for h in ["underweb.h", "underweb_engine.h"] {
        copy(&root.join("include").join(h), &l.include.join(h))?;
    }
    sdk(root, &l.share.join("sdk"))?;
    examples(root, triple, &l.share.join("examples"))?;
    for f in ["README.md", "LICENSE"] {
        if root.join(f).exists() {
            copy(&root.join(f), &l.share.join(f))?;
        }
    }

    if cfg!(target_os = "macos") {
        macos::bundle(&l, &version)?;
        macos::sign(&l, root)?;
    } else if cfg!(windows) {
        windows::bundle(&l)?;
        windows::sign(&l)?;
    } else {
        std::fs::write(stage.join("DEPENDENCIES.txt"), LINUX_DEPS)?;
    }

    let archive = archive(&out, &name, &l.top)?;
    if cfg!(target_os = "macos") {
        macos::notarize(&archive, &l.top)?;
    }
    println!("{}", archive.display());
    Ok(())
}

const LINUX_DEPS: &str = "\
Underweb uses the system's cairo, pango and GStreamer. On Debian/Ubuntu:

  sudo apt install libcairo2 libpango-1.0-0 libpangocairo-1.0-0 \\
    libgstreamer1.0-0 libgstreamer-plugins-base1.0-0 \\
    gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-libav \\
    libasound2 libxkbcommon0 libwayland-client0

Run bin/underweb. The runner and media service must stay next to it.
";

fn layout(stage: &Path) -> Layout {
    if cfg!(target_os = "macos") {
        let c = stage.join("Underweb.app").join("Contents");
        Layout {
            top: stage.join("Underweb.app"),
            bin: c.join("MacOS"),
            libs: c.join("Frameworks"),
            // codesign accepts only bundles and dylibs in Frameworks
            plugins: c.join("PlugIns"),
            engine: c.join("Frameworks"),
            include: c.join("Resources").join("include"),
            share: c.join("Resources"),
        }
    } else {
        Layout {
            top: stage.to_path_buf(),
            bin: stage.join("bin"),
            libs: stage.join(if cfg!(windows) { "bin" } else { "lib" }),
            plugins: stage.join("lib").join("gstreamer-1.0"),
            engine: stage.join(if cfg!(windows) { "bin" } else { "lib" }),
            include: stage.join("include"),
            share: stage.join("share").join("underweb"),
        }
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    if let Some(d) = to.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::copy(from, to).with_context(|| format!("copying {}", from.display()))?;
    Ok(())
}

fn workspace_version(root: &Path) -> Result<String> {
    let text = std::fs::read_to_string(root.join("Cargo.toml"))?;
    let section = text
        .split("[workspace.package]")
        .nth(1)
        .context("no [workspace.package]")?;
    section
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("version = \"")
                .and_then(|v| v.strip_suffix('"'))
        })
        .map(str::to_string)
        .context("no workspace version")
}

fn build(root: &Path) -> Result<()> {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["build", "--profile", "dist-release"])
        .current_dir(root);
    for p in [
        "underweb-app",
        "underweb-runner",
        "underweb-media",
        "underweb-cli",
        "underweb-capi",
    ] {
        cmd.args(["-p", p]);
    }
    if !cmd.status()?.success() {
        bail!("cargo build failed");
    }
    Ok(())
}

/// What the source builder compiles packages against: the Rust SDK crate
/// (and the ABI crate it uses) as standalone crates, the C header, the TS
/// types.
fn sdk(root: &Path, to: &Path) -> Result<()> {
    let ws = std::fs::read_to_string(root.join("Cargo.toml"))?;
    let inherited = |key: &str| -> Result<String> {
        ws.lines()
            .find(|l| l.starts_with(&format!("{key} = ")))
            .map(|l| l.to_string())
            .with_context(|| format!("workspace has no {key}"))
    };
    for krate in ["underweb", "underweb-abi"] {
        let from = root.join("crates").join(krate);
        let dest = to.join("crates").join(krate);
        copy_tree(&from.join("src"), &dest.join("src"), &|_| true)?;
        // no workspace around it in the SDK: spell inherited values out
        let mut manifest = String::new();
        for line in std::fs::read_to_string(from.join("Cargo.toml"))?.lines() {
            let t = line.trim();
            let out = if let Some(k) = t.strip_suffix(".workspace = true") {
                match k {
                    "version" | "edition" | "license" => workspace_package_line(&ws, k)?,
                    "underweb-abi" => "underweb-abi = { path = \"../underweb-abi\" }".into(),
                    dep => inherited(dep)?,
                }
            } else if let Some((k, rest)) = t.split_once(" = { workspace = true") {
                // `cairo-rs = { workspace = true, optional = true }`
                let ws_line = inherited(k)?;
                let spec = ws_line.split_once(" = ").unwrap().1.trim();
                let extra = rest.trim_start_matches(',').trim().trim_end_matches('}');
                let table = if spec.starts_with('{') {
                    spec.trim_end_matches('}').trim_end().to_string()
                } else {
                    format!("{{ version = {spec}")
                };
                if extra.trim().is_empty() {
                    format!("{k} = {table} }}")
                } else {
                    format!("{k} = {table}, {} }}", extra.trim())
                }
            } else {
                line.to_string()
            };
            manifest.push_str(&out);
            manifest.push('\n');
        }
        manifest.push_str("\n[workspace]\n");
        std::fs::write(dest.join("Cargo.toml"), manifest)?;
    }
    copy_tree(&root.join("include"), &to.join("include"), &|_| true)?;
    copy_tree(&root.join("sdk"), &to.join("sdk"), &|_| true)?;
    Ok(())
}

fn workspace_package_line(ws: &str, key: &str) -> Result<String> {
    let section = ws.split("[workspace.package]").nth(1).unwrap_or("");
    let section = section.split("\n[").next().unwrap_or("");
    section
        .lines()
        .find(|l| l.starts_with(&format!("{key} = ")))
        .map(str::to_string)
        .with_context(|| format!("[workspace.package] has no {key}"))
}

/// The example packages, with only this machine's prebuilt libraries.
fn examples(root: &Path, triple: &str, to: &Path) -> Result<()> {
    let from = root.join("examples");
    copy_tree(&from, to, &|rel: &Path| {
        let parts: Vec<_> = rel.components().map(|c| c.as_os_str()).collect();
        if parts.iter().any(|p| *p == "target") {
            return false;
        }
        // <pkg>/dist/<triple>/...: keep ours
        match parts.iter().position(|p| *p == "dist") {
            Some(i) => parts.get(i + 1).is_none_or(|t| *t == triple),
            None => true,
        }
    })
}

fn copy_tree(from: &Path, to: &Path, keep: &dyn Fn(&Path) -> bool) -> Result<()> {
    fn walk(base: &Path, dir: &Path, to: &Path, keep: &dyn Fn(&Path) -> bool) -> Result<()> {
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            let rel = p.strip_prefix(base).unwrap();
            if !keep(rel) || rel.file_name().is_some_and(|n| n == ".DS_Store") {
                continue;
            }
            if p.is_dir() {
                walk(base, &p, to, keep)?;
            } else {
                copy(&p, &to.join(rel))?;
            }
        }
        Ok(())
    }
    walk(from, from, to, keep)
}

fn archive(out: &Path, name: &str, top: &Path) -> Result<PathBuf> {
    let (file, status) = if cfg!(target_os = "macos") {
        let f = out.join(format!("{name}.zip"));
        let _ = std::fs::remove_file(&f);
        // ditto keeps the bundle's signatures and symlinks intact
        let s = Command::new("ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(top)
            .arg(&f)
            .status()?;
        (f, s)
    } else if cfg!(windows) {
        let f = out.join(format!("{name}.zip"));
        let _ = std::fs::remove_file(&f);
        // Windows 10+ ships bsdtar, which writes zip with -a
        let s = Command::new("tar")
            .current_dir(out)
            .arg("-a")
            .arg("-cf")
            .arg(&f)
            .arg(name)
            .status()?;
        (f, s)
    } else {
        let f = out.join(format!("{name}.tar.gz"));
        let s = Command::new("tar")
            .current_dir(out)
            .arg("-czf")
            .arg(&f)
            .arg(name)
            .status()?;
        (f, s)
    };
    if !status.success() {
        bail!("archiving {} failed", top.display());
    }
    Ok(file)
}

/// Every shared library `files` load, transitively, that `resolve` finds
/// outside the system. `deps` lists one file's imports.
fn closure(
    files: &[PathBuf],
    deps: &dyn Fn(&Path) -> Result<Vec<String>>,
    resolve: &dyn Fn(&Path, &str) -> Option<PathBuf>,
) -> Result<HashMap<String, PathBuf>> {
    let mut found: HashMap<String, PathBuf> = HashMap::new();
    let mut todo: Vec<PathBuf> = files.to_vec();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    while let Some(f) = todo.pop() {
        if !seen.insert(f.clone()) {
            continue;
        }
        for d in deps(&f)? {
            if let Some(p) = resolve(&f, &d) {
                let key = p.file_name().unwrap().to_string_lossy().into_owned();
                if let std::collections::hash_map::Entry::Vacant(e) = found.entry(key) {
                    e.insert(p.clone());
                    todo.push(p);
                }
            }
        }
    }
    Ok(found)
}

/// Imported library names, from the file's own headers.
fn imports(path: &Path) -> Result<Vec<String>> {
    use object::{Object, read::File};
    let data = std::fs::read(path)?;
    let file = File::parse(&*data).with_context(|| format!("parsing {}", path.display()))?;
    let mut libs: BTreeSet<String> = BTreeSet::new();
    for i in file.imports()? {
        libs.insert(String::from_utf8_lossy(i.library()).into_owned());
    }
    Ok(libs.into_iter().collect())
}

mod macos {
    use super::*;

    /// Libraries that come with macOS stay where they are.
    fn is_system(lib: &str) -> bool {
        lib.starts_with("/usr/lib/") || lib.starts_with("/System/")
    }

    /// `otool -L`, without the first line (the file's own id).
    fn load_commands(path: &Path) -> Result<Vec<String>> {
        let out = Command::new("otool").arg("-L").arg(path).output()?;
        if !out.status.success() {
            bail!("otool -L {}", path.display());
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .skip(1)
            .filter_map(|l| l.trim().split(" (compatibility").next())
            .map(str::to_string)
            .collect())
    }

    /// Where `lib`, as `from` names it, really is.
    fn resolve(from: &Path, lib: &str) -> Option<PathBuf> {
        if is_system(lib) {
            return None;
        }
        let dir = from.parent()?;
        let p = if let Some(rest) = lib.strip_prefix("@loader_path/") {
            dir.join(rest)
        } else if let Some(rest) = lib.strip_prefix("@rpath/") {
            // Homebrew kegs keep their dependencies side by side
            [
                dir.join(rest),
                PathBuf::from("/opt/homebrew/lib").join(rest),
                PathBuf::from("/usr/local/lib").join(rest),
            ]
            .into_iter()
            .find(|p| p.exists())?
        } else if lib.starts_with('@') {
            return None;
        } else {
            PathBuf::from(lib)
        };
        p.canonicalize().ok()
    }

    fn brew_prefix() -> PathBuf {
        Command::new("brew")
            .arg("--prefix")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
            .unwrap_or_else(|| PathBuf::from("/opt/homebrew"))
    }

    fn install_name_tool(args: &[&std::ffi::OsStr]) -> Result<()> {
        let s = Command::new("install_name_tool").args(args).output()?;
        if !s.status.success() {
            bail!(
                "install_name_tool {:?}: {}",
                args,
                String::from_utf8_lossy(&s.stderr)
            );
        }
        Ok(())
    }

    pub fn bundle(l: &Layout, version: &str) -> Result<()> {
        std::fs::write(
            l.top.join("Contents").join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Underweb</string>
  <key>CFBundleDisplayName</key><string>Underweb</string>
  <key>CFBundleIdentifier</key><string>org.underweb.Underweb</string>
  <key>CFBundleExecutable</key><string>underweb</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>CFBundleVersion</key><string>{version}</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSCameraUsageDescription</key><string>Pages you allow can show your camera.</string>
  <key>NSMicrophoneUsageDescription</key><string>Pages you allow can hear your microphone.</string>
</dict>
</plist>
"#
            ),
        )?;

        // GStreamer plugins first: they pull in most of the libraries
        let gst = brew_prefix().join("lib").join("gstreamer-1.0");
        std::fs::create_dir_all(&l.plugins)?;
        let mut plugins = Vec::new();
        for p in GST_PLUGINS {
            let f = gst.join(format!("libgst{p}.dylib"));
            if f.exists() {
                let to = l.plugins.join(f.file_name().unwrap());
                copy(&f, &to)?;
                plugins.push(to);
            } else {
                println!("note: no GStreamer plugin {p} here");
            }
        }

        let mut roots: Vec<PathBuf> = PROGRAMS.iter().map(|p| l.bin.join(p)).collect();
        roots.push(l.engine.join(crate::dylib_name("underweb_engine")));
        roots.extend(plugins.iter().cloned());
        let deps = |p: &Path| load_commands(p);
        let libs = closure(&roots, &deps, &resolve)?;
        for (name, from) in &libs {
            let to = l.libs.join(name);
            copy(from, &to)?;
            let mut perm = std::fs::metadata(&to)?.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            std::fs::set_permissions(&to, perm)?;
        }
        println!(
            "bundled {} libraries, {} plugins",
            libs.len(),
            plugins.len()
        );

        // point every reference at the bundled copy
        let fix = |file: &Path, prefix: &str| -> Result<()> {
            for dep in load_commands(file)? {
                if let Some(real) = resolve(file, &dep) {
                    let name = real.file_name().unwrap().to_string_lossy().into_owned();
                    if libs.contains_key(&name) {
                        let new = format!("{prefix}{name}");
                        install_name_tool(&[
                            "-change".as_ref(),
                            dep.as_ref(),
                            new.as_ref(),
                            file.as_os_str(),
                        ])?;
                    }
                }
            }
            Ok(())
        };
        for p in PROGRAMS {
            fix(&l.bin.join(p), "@executable_path/../Frameworks/")?;
        }
        for name in libs
            .keys()
            .cloned()
            .chain([crate::dylib_name("underweb_engine")])
        {
            let f = l.libs.join(&name);
            let id = format!("@loader_path/{name}");
            install_name_tool(&["-id".as_ref(), id.as_ref(), f.as_os_str()])?;
            fix(&f, "@loader_path/")?;
        }
        for p in &plugins {
            let id = format!("@loader_path/{}", p.file_name().unwrap().to_string_lossy());
            install_name_tool(&["-id".as_ref(), id.as_ref(), p.as_os_str()])?;
            fix(p, "@loader_path/../Frameworks/")?;
        }
        Ok(())
    }

    const RUNNER_ENTITLEMENTS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <!-- V8 compiles JavaScript to machine code -->
  <key>com.apple.security.cs.allow-jit</key><true/>
  <key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
  <!-- native packages are libraries their authors built, not us -->
  <key>com.apple.security.cs.disable-library-validation</key><true/>
</dict>
</plist>
"#;

    const APP_ENTITLEMENTS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.device.camera</key><true/>
  <key>com.apple.security.device.audio-input</key><true/>
</dict>
</plist>
"#;

    pub fn sign(l: &Layout, root: &Path) -> Result<()> {
        let identity = std::env::var("UNDERWEB_MACOS_IDENTITY").ok();
        let tmp = root.join("target").join("dist");
        let runner_ent = tmp.join("runner.entitlements");
        let app_ent = tmp.join("app.entitlements");
        std::fs::write(&runner_ent, RUNNER_ENTITLEMENTS)?;
        std::fs::write(&app_ent, APP_ENTITLEMENTS)?;
        let sign = |path: &Path, entitlements: Option<&Path>| -> Result<()> {
            let mut c = Command::new("codesign");
            c.args(["--force", "--sign"]);
            match &identity {
                Some(id) => {
                    c.arg(id).args(["--options", "runtime", "--timestamp"]);
                }
                None => {
                    c.arg("-");
                }
            }
            if let Some(e) = entitlements {
                c.arg("--entitlements").arg(e);
            }
            let out = c.arg(path).output()?;
            if !out.status.success() {
                bail!(
                    "codesign {}: {}",
                    path.display(),
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            Ok(())
        };
        // inside out: libraries, helpers, then the bundle
        for dir in [&l.plugins, &l.libs] {
            for e in std::fs::read_dir(dir)? {
                let p = e?.path();
                if p.extension().is_some_and(|x| x == "dylib") {
                    sign(&p, None)?;
                }
            }
        }
        sign(&l.bin.join("underweb-runner"), Some(&runner_ent))?;
        sign(&l.bin.join("underweb-media"), Some(&app_ent))?;
        sign(&l.bin.join("uw"), Some(&runner_ent))?;
        sign(&l.top, Some(&app_ent))?;
        let check = Command::new("codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(&l.top)
            .status()?;
        if !check.success() {
            bail!("the signed bundle does not verify");
        }
        println!(
            "signed ({})",
            identity
                .as_deref()
                .unwrap_or("ad hoc: runs here, not notarized")
        );
        Ok(())
    }

    /// Notarize and staple when credentials are given, then re-zip so the
    /// archive carries the ticket.
    pub fn notarize(archive: &Path, app: &Path) -> Result<()> {
        let mut args: Vec<String> = vec!["notarytool".into(), "submit".into()];
        args.push(archive.to_string_lossy().into());
        if let Ok(profile) = std::env::var("UNDERWEB_NOTARY_PROFILE") {
            args.extend(["--keychain-profile".into(), profile]);
        } else if let (Ok(id), Ok(team), Ok(pw)) = (
            std::env::var("APPLE_ID"),
            std::env::var("APPLE_TEAM_ID"),
            std::env::var("APPLE_APP_PASSWORD"),
        ) {
            args.extend([
                "--apple-id".into(),
                id,
                "--team-id".into(),
                team,
                "--password".into(),
                pw,
            ]);
        } else {
            return Ok(());
        }
        args.push("--wait".into());
        if !Command::new("xcrun").args(&args).status()?.success() {
            bail!("notarization failed");
        }
        if !Command::new("xcrun")
            .args(["stapler", "staple"])
            .arg(app)
            .status()?
            .success()
        {
            bail!("stapling failed");
        }
        std::fs::remove_file(archive)?;
        if !Command::new("ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(app)
            .arg(archive)
            .status()?
            .success()
        {
            bail!("re-archiving after stapling failed");
        }
        println!("notarized");
        Ok(())
    }
}

mod windows {
    use super::*;

    /// Where the GTK-stack and GStreamer DLLs are: `bin` directories on
    /// `PATH` that hold one of them.
    fn dll_dirs() -> Vec<PathBuf> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path)
            .filter(|d| {
                ["cairo-2.dll", "glib-2.0-0.dll", "gstreamer-1.0-0.dll"]
                    .iter()
                    .any(|m| d.join(m).is_file())
            })
            .collect()
    }

    pub fn bundle(l: &Layout) -> Result<()> {
        let dirs = dll_dirs();
        if dirs.is_empty() {
            bail!("no cairo/GStreamer DLLs on PATH (add gvsbuild's and GStreamer's bin)");
        }
        // GStreamer's plugins live in <root>\lib\gstreamer-1.0
        std::fs::create_dir_all(&l.plugins)?;
        let mut plugins = Vec::new();
        for d in &dirs {
            let Some(root) = d.parent() else { continue };
            let gst = root.join("lib").join("gstreamer-1.0");
            for p in GST_PLUGINS {
                let f = gst.join(format!("gst{p}.dll"));
                let to = l.plugins.join(f.file_name().unwrap());
                if f.exists() && !to.exists() {
                    copy(&f, &to)?;
                    plugins.push(to);
                }
            }
        }
        let resolve = |_: &Path, name: &str| -> Option<PathBuf> {
            let lower = name.to_ascii_lowercase();
            if lower.starts_with("api-ms-win-") || lower.starts_with("ext-ms-") {
                return None;
            }
            // only what we found in the library dirs; System32 stays
            dirs.iter().map(|d| d.join(name)).find(|p| p.is_file())
        };
        let mut roots: Vec<PathBuf> = PROGRAMS.iter().map(|p| l.bin.join(exe(p))).collect();
        roots.push(l.engine.join(crate::dylib_name("underweb_engine")));
        roots.extend(plugins.iter().cloned());
        let libs = closure(&roots, &|p| imports(p), &resolve)?;
        for (name, from) in &libs {
            copy(from, &l.libs.join(name))?;
        }
        println!("bundled {} DLLs, {} plugins", libs.len(), plugins.len());
        Ok(())
    }

    pub fn sign(l: &Layout) -> Result<()> {
        let (Ok(cert), Ok(password)) = (
            std::env::var("UNDERWEB_WINDOWS_CERT"),
            std::env::var("UNDERWEB_WINDOWS_CERT_PASSWORD"),
        ) else {
            println!("not code-signed (no UNDERWEB_WINDOWS_CERT)");
            return Ok(());
        };
        let mut files: Vec<PathBuf> = PROGRAMS.iter().map(|p| l.bin.join(exe(p))).collect();
        files.push(l.engine.join(crate::dylib_name("underweb_engine")));
        let status = Command::new("signtool")
            .args(["sign", "/fd", "SHA256", "/td", "SHA256", "/tr"])
            .arg("http://timestamp.digicert.com")
            .arg("/f")
            .arg(cert)
            .arg("/p")
            .arg(password)
            .args(&files)
            .status()
            .context("running signtool (from the Windows SDK)")?;
        if !status.success() {
            bail!("signtool failed");
        }
        println!("code-signed {} files", files.len());
        Ok(())
    }
}
