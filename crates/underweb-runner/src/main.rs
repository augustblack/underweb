//! `underweb-runner`: one process per package.
//!
//! ```text
//! underweb-runner --ipc-fd N --package DIR --data DIR [--no-sandbox] [--param k=v]...
//! underweb-runner --build ...   (see build.rs)
//! ```
//!
//! Startup order matters: everything that needs broad file access (IPC,
//! fonts, V8) is set up first, then the process locks itself down, and only
//! then is package code loaded.

mod audio;
mod build;
mod js;
mod native;
mod net;
mod page;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context, Result, bail};
use underweb_guest::Scene;
use underweb_ipc::{Channel, GuestMsg, PROTOCOL_VERSION};
use underweb_pkg::{Kind, Package};

struct Args {
    ipc_fd: i32,
    package: PathBuf,
    data: PathBuf,
    sandbox: bool,
    params: Vec<(String, String)>,
    audio_rate: u32,
}

fn parse_args() -> Result<Args> {
    let mut it = std::env::args().skip(1);
    let (mut ipc_fd, mut package, mut data) = (None, None, None);
    let mut sandbox = true;
    let mut params = Vec::new();
    let mut audio_rate = 0;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ipc-fd" => ipc_fd = Some(it.next().context("--ipc-fd N")?.parse()?),
            "--package" => package = Some(PathBuf::from(it.next().context("--package DIR")?)),
            "--data" => data = Some(PathBuf::from(it.next().context("--data DIR")?)),
            "--no-sandbox" => sandbox = false,
            "--audio-rate" => audio_rate = it.next().context("--audio-rate N")?.parse()?,
            "--param" => {
                let kv = it.next().context("--param k=v")?;
                let (k, v) = kv.split_once('=').unwrap_or((&kv, ""));
                params.push((k.to_string(), v.to_string()));
            }
            other => bail!("unknown argument {other:?}"),
        }
    }
    Ok(Args {
        ipc_fd: ipc_fd.context("missing --ipc-fd")?,
        package: package.context("missing --package")?,
        data: data.context("missing --data")?,
        sandbox,
        params,
        audio_rate,
    })
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--build") {
        build::main(std::env::args().skip(2));
    }
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("underweb-runner: {e:#}");
            std::process::exit(2);
        }
    };
    let chan = match unsafe { Channel::from_raw_fd(args.ipc_fd) } {
        Ok(c) => c,
        Err(e) => {
            eprintln!("underweb-runner: bad ipc fd: {e:#}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(&chan, args) {
        let _ = chan.send(&GuestMsg::Error(format!("{e:#}")), None);
        std::process::exit(1);
    }
}

fn run(chan: &Channel, args: Args) -> Result<()> {
    let pkg = Package::open(&args.package)?;
    std::fs::create_dir_all(&args.data).context("creating data dir")?;
    underweb_guest::warm_up_fonts();

    let scene = Rc::new(RefCell::new(Scene::new()));
    let audio = Rc::new(audio::Audio::new(chan, args.audio_rate, &pkg.dir));
    scene
        .borrow_mut()
        .params
        .extend(args.params.iter().cloned());

    let lock = |chan: &Channel| -> Result<()> {
        if args.sandbox {
            underweb_sandbox::lockdown(&underweb_sandbox::Policy {
                read_only: vec![pkg.dir.clone()],
                read_write: vec![args.data.clone()],
                allow_exec: false,
            })?;
        }
        chan.send(
            &GuestMsg::Hello {
                protocol: PROTOCOL_VERSION,
                sandboxed: args.sandbox,
            },
            None,
        )
    };

    match pkg.kind() {
        Kind::Js | Kind::Ts => {
            let mut page = js::JsPage::new(&pkg, scene.clone(), chan, &args.data, audio.clone())?;
            lock(chan)?;
            page.load()?;
            page::run(chan, &scene, &mut page, &audio)
        }
        Kind::Rust | Kind::C => {
            let lib = pkg.native_library()?;
            lock(chan)?;
            let mut page =
                native::NativePage::load(&lib, scene.clone(), chan, &args.data, audio.clone())?;
            page::run(chan, &scene, &mut page, &audio)
        }
    }
}
