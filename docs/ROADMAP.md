# Underweb 2: a Rust package browser

## Context
The Vala revival in `underweb-git/` (branch `modernize`) works partly. It stays as-is, kept as a reference only. Its biggest flaw is that pages are fetched over http, compiled and run as native code with no sandbox and no trust model.

The new project is a Rust rewrite with these goals:
- packages, not pages, are the unit of content, and the engine is also an embeddable library;
- packages written in JS, TS, Rust and C run natively, with no wasm;
- normal web content renders through Chromium (CEF);
- a curated Underweb API is built on cairo, pango and GStreamer, and native packages can also call cairo and pango directly;
- packages can do brokered TCP/UDP networking;
- a mandatory OS sandbox is combined with an ATProto-based trust and vouching system.

Every original feature carries over: the canvas, audio and Pd, video and streaming, and the layout editor. The project is developed on macOS and runs on Mac, Linux and Windows.

**User decisions:**

| Topic | Decision |
|---|---|
| "Package" | Content is packages, and the engine is a library |
| Native delivery | Prebuilt binaries per platform, with a source fallback |
| Web engine | CEF, rendered offscreen and composited into the canvas |
| C APIs | Curated API, plus raw cairo and pango for native packages |
| Trust | Pluggable, with ATProto as the default adapter |
| Carry-over | All four features |
| Milestone 1 | A sandboxed hello-world in JS, TS and Rust |

Location: a new repo at `/Users/august/tmp/underweb2026/underweb-rs`. `underweb-git` is not touched.

## Architecture
```
            ┌──────────────── underweb (host, trusted) ─────────────────┐
            │ winit window · wgpu compositor · egui chrome (tabs, URL,  │
            │ permission prompts, trust panel, layout editor overlay)   │
            │ package manager · permission broker · trust engine        │
            │ audio mixer (cpal RT thread) · CEF (OSR) · media service  │
            └───▲───────────────▲──────────────────▲────────────────────┘
     ipc-channel│ + shm frames  │                  │ CEF's own sandboxed
   ┌────────────┴───┐  ┌────────┴───────┐   ┌──────┴──────┐ renderers
   │ runner --js    │  │ runner --native│   │ runner ...  │
   │ V8 (deno_core) │  │ dlopen cdylib  │   │ (one process│
   │ TS via deno_ast│  │ Rust / C pkg   │   │ per package)│
   └── OS sandbox ──┘  └── OS sandbox ──┘   └─────────────┘
```
- **One sandboxed process per package.** Each package runs in its own process, which locks itself down after startup and before it loads any package code, as Chromium does. The sandbox is always on. Trust only decides which permissions are granted without asking.
- **Drawing.** The host creates double-buffered shared-memory frames. The runner wraps each one in a `cairo::ImageSurface::create_for_data` surface, and the package draws with cairo and pango. When it calls `frame_ready()`, the host uploads the frame to a wgpu texture and composites it with CEF and video textures.
  - The host sends vsync ticks, which replace the old 2 ms timer.
  - Input events are sent to the runner over IPC.
- **The curated API is implemented once.** It lives in Rust in the `underweb-guest` crate and covers Layout, Shape, Path, Text, Image, Audio, Video, Net and Storage. It mirrors the Mentiras/MyActivatable design (`activate`, `deactivate`, `update`). It has three front ends:
  - the Rust crate itself;
  - a C header and library generated with `cbindgen`;
  - deno_core ops, plus a generated `underweb.d.ts` for TS.

  Native packages can also link `cairo-rs`/`pango-rs`, or the cairo and pango C libraries, directly against the surface they are given.
- **Engine as a library.** `underweb-core` contains the package manager, process host, broker, trust engine and compositor. It exposes a Rust API and a C API generated with cbindgen. `underweb-app` is a thin shell around it.

## Workspace layout (new repo)
| Crate | Role | Key deps |
|---|---|---|
| `underweb-core` | Host library: process spawning, broker, compositor, package manager, trust engine | `ipc-channel`, `wgpu`, `tokio`, `reqwest`, `blake3`, `ed25519-dalek` |
| `underweb-app` | Browser binary: window and chrome | `winit`, `egui`/`egui-wgpu`, `cef` (tauri-apps/cef-rs) |
| `underweb-ipc` | Message types and frame/shm protocol | `serde`, `ipc-channel` |
| `underweb-guest` | Curated API: Rust crate, C ABI (`underweb.h`) and JS ops | `cairo-rs`, `pango`, `pangocairo` |
| `underweb-runner` | Sandboxed process binary: `--js` / `--native` | `deno_core`, `deno_ast`, `libloading` |
| `underweb-sandbox` | Per-OS lockdown | macOS `sandbox_init` SBPL profile; Linux `landlock` + `seccompiler` + namespaces (or `birdcage`); Windows AppContainer + job object via `windows` |
| `underweb-pkg` | Manifest, archives, cache, builder | `toml`, `tar`, `zstd` |
| `underweb-trust` | Statement format, verification, policy engine, `TrustAdapter` trait | `ed25519-dalek`, `serde_json` |
| `underweb-trust-atproto` | Default adapter: identity, publishing, follow graph, labelers | `atrium-api` / `atrium-xrpc` |
| `examples/` | hello-js, hello-ts, hello-rust, hello-c, then ports of demos 01–15 | — |

## Packages
- **Manifest (`underweb.toml`).** It contains:
  - name, version, author DID;
  - kind (`js`, `ts`, `rust`, `c` or `html`) and entry point;
  - `[targets]`: per-target binary URL and blake3 hash (`aarch64-apple-darwin`, `x86_64-pc-windows-msvc`, ...);
  - `[source]`;
  - `[permissions]`, for example `net.tcp.connect = ["host:port"]`, `net.udp.bind = [9000]`, `audio.out`, `audio.in`, `camera`, `fetch = ["https://api.x"]`;
  - dependencies.
- **Addressing.** A package is named `uw://<handle-or-DID>/<name>[@version]`. An `http(s)` URL opens in CEF, unless the site serves `/.well-known/underweb.toml`; then it opens as a package.
- **Archives and cache.** Archives are content-addressed `.tar.zst` files. The cache key is the hash, and versions are checked for updates. That fixes underweb's cache that never refreshed.
- **Source fallback.** If no prebuilt binary matches the platform, the browser builds the source in a sandboxed *builder* process:
  - `cargo fetch --locked` runs first, then the build runs with the network denied. This matters because `build.rs` and proc macros are arbitrary code.
  - C builds use a bundled `zig cc`. Rust builds need `rustup`, which the browser detects and offers to install.
  - If the build is reproducible and its hash matches the published binary, vouches for that binary apply to the local build too.
- **Native ABI.** A native package exports `uw_activate(const UwHost*)`, `uw_deactivate`, `uw_update` and `uw_abi_version`.

## Permissions and networking
- The broker checks each request against the manifest permissions and the user's grants. Grant prompts appear in the egui chrome.
- **TCP/UDP.** The host opens the socket and passes the fd into the runner: `SCM_RIGHTS` on Mac and Linux, `WSADuplicateSocket` on Windows. The package then does fast I/O on that socket directly, while the sandbox still blocks `socket()`/`connect()`.
- **Fetch.** HTTP(S) requests go through a host API backed by reqwest and are restricted by origin.
- **Storage.** Each package gets a private data directory, the only place it may write.

## Trust: pluggable, ATProto by default
Trust doesn't depend on any one network. The UI never shows protocol words like "DID" or "PDS". It says "Vouched by 3 people you follow: alice.bsky.social, ...".

- **Statements (`underweb-trust`).** A release or a vouch is a small signed statement: `{type: release|vouch, subject: package name@version, hash (blake3), level: used|reviewed|warn, note, signer identity, signature}`. Verification is local and the same whatever the source. This is the only format the policy engine sees.
- **Adapters.** A `TrustAdapter` trait has five operations: sign in, publish a statement, fetch statements about a hash, fetch the user's follows, and fetch labels.
- **ATProto, the default adapter (in Phase 4):**
  - An identity is an existing Bluesky/ATProto account; login is by handle and app password, then OAuth.
  - Statements are stored as `app.underweb.release` and `app.underweb.vouch` records. Repo commits are signed, so the record carries the signature.
  - Follows are imported from the user's ATProto follow graph.
  - Labeler services can flag packages, for example as malware or a crypto miner.
  - Archives are hosted as a PDS blob or at any https mirror. The hash in the record is what's checked.
- **Later adapters**, each implementing the same trait:
  - signed trust lists in git or at an https URL, for organisations and offline use;
  - Sigstore with GitHub/Google login, for authors who don't use Bluesky;
  - GitHub and Mastodon follow-graph import (follows only, because Mastodon posts can't be verified later);
  - Nostr.
- **Local policy.** A trust store holds the people, labelers and lists the user follows, whichever adapter they came from. A policy such as "auto-grant the declared permissions if ≥N followed identities vouched for this exact hash and no subscribed labeler or list warns" decides grants; otherwise the browser asks. Trust never turns off the sandbox.
- **Vouching in the UI.** The trust panel shows who vouched and through which network. It lets the user vouch or warn using any account they've connected.

## Web content (CEF)
- **Offscreen rendering.** `cef-rs` uses windowless rendering. `on_accelerated_paint` (falling back to `on_paint`) produces a texture that wgpu composites, so HTML can sit inside shapes the way video does.
- **Message loop.** CEF runs with `external_message_pump` so the winit loop stays in control.
- **macOS bundle.** CEF needs an app bundle with helper apps (GPU, Renderer, Plugin). A `cargo xtask bundle` command builds it.
- **Size.** CEF adds about 200 MB per platform. Its binaries come from the official CEF builds.
- **Later.** HTML pages could get a `window.underweb` bridge through CEF's message router, guarded by permissions.

## Media
- **Audio.** cpal runs the real-time output and input thread in the host. Each package gets a lock-free SPSC ring buffer in shared memory. The package renders audio on its own thread, using `ugens` ported from `libmentiras/ugens.vala`, and **libpd** through `libpd-rs`, which vendors libpd as a crate.
- **Video, camera and streaming.** These run in a separate media-service process (trusted, with hardware decoders) using `gstreamer-rs`. Frames go to the package through shm, so the package can draw them clipped by cairo paths, as in the old `videoplayer.vala`. Camera capture and Icecast streaming (`shout2send`) need permissions.
- **Layout editor.** The guest API keeps a retained scene graph of shapes and mirrors it to the host. The editor is an egui overlay that does selection, dragging and property editing. It exports a page to JS, TS, Rust or C templates, replacing `build_files/header.*`/`footer.*`.

## Phases
0. **Skeleton.** Workspace, `cargo xtask`, and CI on GitHub Actions (macOS required, Linux builds).
1. **Milestone 1: sandboxed hello in 3 languages, on macOS.**
   - Host window (winit, wgpu, egui with a URL bar and tabs).
   - The runner process, IPC, and double-buffered shm cairo frames.
   - Guest API v0: Layout and Shape (rect, path, fill, stroke), plus text with pango.
   - Runner modes: `--js` (V8), `--js` with TS transpiled by deno_ast, and `--native` for a Rust cdylib; `hello-c` through `underweb.h` as a stretch goal.
   - The macOS sandbox profile, plus Linux landlock and seccomp built in CI.
   - Packages load from a local directory with a manifest; no signing yet.
2. **Packages.** Archives, cache, `uw://` resolution, prebuilt targets, the sandboxed source builder, and the C embedding API.
3. **Permissions.** The broker, fd-passed TCP/UDP, fetch, storage, and prompts.
4. **Trust.** The statement format, local verification, the policy engine and the trust panel, then the ATProto adapter (login, records, follows, labelers). The signed-list adapter follows, as a way to test the adapter trait.
5. **CEF.** CEF tabs with offscreen compositing and the macOS bundle.
6. **Audio and Pd.** The ring-buffer audio path, ugens, and libpd.
7. **Video.** The media service: playback in shapes, camera, and Icecast.
8. **Editor and demos.** The layout editor and code export; ports of demos 01–15.
9. **Windows and releases.** The Windows AppContainer sandbox; signed release packaging for all three OSes, built by the CI matrix.

**After approval, I implement Phases 0–1.** Later phases come back for review one at a time.

## Known risks and constraints
- **V8 needs JIT.** The macOS sandbox profile and hardened runtime must allow `MAP_JIT` (the `allow-jit` entitlement). Otherwise, run V8 with `--jitless` in the strictest mode.
- **Fonts inside the sandbox.** Pango needs font access: the CoreText mach services on Mac, and the font and fontconfig directories read-only on Linux. The sandbox profile is built allowlist-first, and denials are logged.
- **Building on the Mac and running everywhere.** The Mac builds locally. Linux and Windows release binaries come from CI, because CEF, V8, GStreamer and pango aren't practical to cross-compile from a Mac. On Windows, cairo and pango come from gvsbuild and GStreamer from its MSVC installer.
- **Windows sandbox timing.** The Windows sandbox comes last. Until then, Windows runners are marked "unsandboxed" in the UI and get no permissions automatically.
- **Licensing is still open.**
  - The original project is GPL-3.0.
  - The dependencies are BSD (CEF, V8, libpd) or LGPL (cairo, pango, GStreamer).
  - Pick a license before the first release.

## Verification (Milestone 1)
- `cargo build --workspace` succeeds on macOS, and CI also builds it on Linux.
- `cargo run -p underweb-app -- examples/hello-{js,ts,rust}` shows each package's shape with pango text. I check this with a self-rendered snapshot mode, `UNDERWEB_SNAPSHOT=out.png`, reused from the Vala work because screen capture isn't available.
- Sandbox tests are a test package for each runner mode. Each one tries to read `~/.ssh`, write outside its data directory, open a TCP socket and exec `/bin/sh`. Every attempt must fail, and the integration test asserts the denials. A control run without the sandbox shows the same calls succeeding.
- Unit tests cover IPC round-trips and shm frame swapping. If a package crashes or hangs, only its tab shows an error, and the host keeps running.
