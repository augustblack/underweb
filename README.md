# Underweb

A browser for **packages** instead of web pages. Packages are written in
JavaScript, TypeScript, Rust or C, run natively (no wasm), and draw on a
vector canvas built on cairo and pango. Every package runs in its own
sandboxed process.

This is a Rust rewrite of the 2011 Vala project. See
[docs/ROADMAP.md](docs/ROADMAP.md) for the design and phases.

## Status: phases 0–4 and 6–9

Working on macOS:

- the browser: tabs, an address bar, a sandbox badge, a log panel
- one `underweb-runner` process per tab, sandboxed before any package code
  loads (macOS: `sandbox_init`; Linux: Landlock + seccomp; Windows: an
  AppContainer and a job object chosen when the process is created; Linux
  and Windows are tested in CI only)
- JS and TS packages in V8 (deno_core), with TypeScript transpiled by
  deno_ast. They get the Underweb API and nothing else: no Deno, Node or
  fetch.
- Rust and C packages as prebuilt shared libraries behind a C ABI
  ([include/underweb.h](include/underweb.h)), with optional raw cairo
  drawing
- layouts and shapes (paths, fill, stroke, pango text), hit testing, pointer
  events, animation driven by the display's frame rate, and HiDPI
- shared-memory frames, so a crashing package only takes its own tab down
- `uw://<domain>/<name>[@version]` packages: a release index served from
  `https://<domain>/.well-known/underweb/<name>.toml`, archives checked
  against their BLAKE3 hashes, a content-addressed cache that picks up new
  versions on every open and still works offline
- native packages with no prebuilt library for this machine are built from
  source in a sandboxed builder (cc for C, cargo for Rust): build scripts
  can run compilers but can't reach the network or your files
- `uw`, a command-line tool for authors: `build`, `publish`, `serve`
- the engine as a C library (`include/underweb_engine.h`)
- networking through a permission broker: packages declare what they need
  in `[permissions]` (TCP connect/listen, UDP bind/send, fetch URL
  prefixes); the browser asks the user ("Allow once / Always allow /
  Deny") and does the I/O itself, so package processes still can't open a
  socket. "Always" is remembered per package origin; the Permissions panel
  shows and forgets decisions.
- private storage per package (`storage` in JS/TS, `data_dir()` for native)

- the layout editor (**Edit** in the address bar, or `--edit`): select
  shapes, drag them and their points, change colours, text, fonts,
  pictures and the background, add and restack shapes, while the page
  keeps running in its sandbox. **Export** writes the scene as a new
  package in JavaScript, TypeScript, Rust or C (`uw export` does the same
  from the command line), replacing the old `build_files` templates.
- the 2011 demos, ported: see [examples/README.md](examples/README.md)

- trust: publishers sign releases, anyone can vouch for or warn about an
  exact archive, and the browser shows "published by … · vouched for by …
  (you follow)". When enough people you follow vouch and none warn, the
  package's declared permissions are granted without asking; trust never
  turns off the sandbox. Identities can be Bluesky accounts, domains, or
  bare keys; vouches live on Bluesky or in signed lists.

- release archives for macOS, Linux and Windows (`cargo xtask dist`, and
  the Release workflow on a `v*` tag), signed with a release statement
  that `uw verify-release` checks

Not yet: CEF web pages (5, skipped for now).

## Build and run

Needs Rust ≥ 1.91, cairo, pango, GStreamer, and cmake (to build libpd):

```sh
brew install cairo pango pkgconf cmake gstreamer         # macOS
sudo apt install libcairo2-dev libpango1.0-dev pkg-config cmake clang \
  libclang-dev libasound2-dev libxkbcommon-dev libwayland-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-good gstreamer1.0-libav            # Debian/Ubuntu
```

On Windows: Visual Studio Build Tools (run from a developer prompt so
`cl.exe` is on `PATH`), GStreamer's MSVC installer (1.26 or later; it
includes cairo and pango), and `pkg-config`
(`choco install pkgconfiglite`), with GStreamer's `bin` on `PATH` and its
`lib\pkgconfig` in `PKG_CONFIG_PATH`; see
[.github/actions/native-deps](.github/actions/native-deps/action.yml).

```sh

cargo xtask examples          # builds the runner and the native example packages
cargo run -p underweb-app -- hello-js hello-ts hello-rust hello-c
cargo run -p underweb-app -- --edit demo-03-shapes   # open in the layout editor
```

Addresses are `uw://` packages or package directories, optionally with
parameters (`examples/probe-rust?crash=1`). `--no-sandbox` turns the sandbox off for
debugging, and the badge turns red.

`cargo xtask test` builds everything and runs all tests, including the
sandbox tests: a probe package tries to read your files, write outside its
folder, open a TCP connection and run `/bin/sh`, and every attempt must
fail. The same probe with the sandbox off must succeed.

## Releases

`cargo xtask dist` builds a release archive for this machine in
`target/dist/`: `Underweb.app` in a zip on macOS (Homebrew libraries and
GStreamer plugins bundled), a zip with every DLL on Windows, a tarball on
Linux (using the system's libraries). It signs what it can from the
environment (see `xtask/src/dist.rs`): a Developer ID and notarization on
macOS, Authenticode on Windows, ad-hoc otherwise.

Pushing a `v*` tag runs the Release workflow on all platforms and drafts a
GitHub release with the archives, `SHA256SUMS`, and (with the
`UNDERWEB_RELEASE_KEY` secret) a signed release statement:

```sh
uw sign-release target/dist/underweb-* --name underweb --version 0.1.0 --key KEYFILE --out release.json
uw verify-release release.json underweb-0.1.0-aarch64-apple-darwin.zip --key <public key>
```

## Publishing

A repository is a plain directory you can upload to any web server:

```sh
cargo run -p underweb-cli -- publish examples/hello-ts --repo site/
cargo run -p underweb-cli -- publish examples/hello-c --repo site/   # source + this machine's build
cargo run -p underweb-cli -- serve site/ --port 8000                # local test server
cargo run -p underweb-app -- uw://localhost:8000/hello-ts
```

Uploaded to `https://example.org/`, the packages are at
`uw://example.org/hello-ts`. Publish native packages from each platform
(or CI) to add prebuilt libraries; machines without one build from the
source archive.

## Trust

```sh
uw key                                   # your signing key (created on first use)
uw publish examples/hello-net --repo site/ --sign --as dns:example.org
                                         # serve site/ at https://example.org/
uw vouch uw://example.org/hello-net --level reviewed --note "read it" \
    --list reviews.json --name "My reviews"          # a signed list you host…
UNDERWEB_APP_PASSWORD=… uw link-bluesky alice.bsky.social
UNDERWEB_APP_PASSWORD=… uw vouch uw://example.org/hello-net --bluesky alice.bsky.social
                                         # …or a record on your Bluesky account
uw follow alice.bsky.social              # whose vouches count
uw follow --list https://example.org/reviews.json
uw follow-bluesky alice.bsky.social      # everyone alice follows
uw check uw://example.org/hello-net      # what the browser will say
```

A statement is a small JSON document signed with an Ed25519 key and checked
on your machine. Bluesky accounts declare their key in an `app.underweb.key`
record and publish statements as `app.underweb.statement` records; domains
publish keys at `/.well-known/underweb/key`. The first key seen for an
identity is pinned, and a different one later is flagged. Not yet:
checking ATProto repository proofs (records are read over HTTPS from each
account's server), vouching from inside the browser, and more adapters
(Sigstore, Mastodon follows, Nostr).

## Writing a package

Every package is a directory with an `underweb.toml`:

```toml
[package]
name = "my-page"
version = "0.1.0"
kind = "ts"          # js | ts | rust | c
entry = "main.ts"    # js/ts only; native packages list [targets.<triple>]
```

```ts
import { Layout, Shape, window } from "underweb";   // types: sdk/js/underweb.d.ts

export function activate() {
  const layout = new Layout();
  window.addLayout(layout);
  const s = new Shape().rectangle(100, 100, 300, 200).setFillColor(1, 0, 0);
  s.text = "Hello";
  layout.append(s);
}
export function update(dt: number) {}   // every frame while window.animating
export function pointer(ev) {}          // ev.shape is the shape under the pointer
```

Shapes can be moved, turned and scaled, show pictures, and flow text
inside their outline:

```ts
import { Image, Matrix } from "underweb";
s.translate(200, 100).rotate(Math.PI / 18);  // or s.matrix = new Matrix(...)
s.image = Image.open("photo.jpg");           // PNG, JPEG, GIF from the package
s.markup = true;                             // Pango markup: <b>, <span foreground="red">
s.flow = true;                               // word by word inside the outline
s.name = "photo";                            // shown in the layout editor
layout.setBackground(1, 0.9, 1);
```

Network access needs a declaration, and the user's OK:

```toml
[permissions]
net.tcp.connect = ["example.org:7000", "*.example.org:443"]
net.tcp.listen  = [9000]
net.udp.bind    = [57120]
net.udp.send    = ["192.168.1.20:57110", "*:9000"]
fetch           = ["https://api.example.org/v1/"]
```

```ts
import { net, storage } from "underweb";
const c = await net.connect("example.org", 7000);   // rejects if refused
c.ondata = (bytes) => { /* … */ };
c.send("hello");
const r = await net.fetch("https://api.example.org/v1/items");
storage.set("last", await r.text());
```

Sound needs no permission:

```ts
import { audio } from "underweb";
const osc = audio.osc("square", { freq: 220, amp: 0.2, pan: 0 });
osc.freq = 330;                    // smoothed, no clicks
audio.pd.open("synth.pd");         // a patch in the package
audio.pd.send("freq", 440);        // to [r freq]
audio.pd.subscribe("beat");        // from [s beat]
audio.pd.onmessage = (receiver, value) => { /* … */ };
const samples = audio.scope(512);  // what's playing, for drawing
```

Video goes in shapes:

```js
import { Shape, video } from "underweb";
const clip = video.open("clip.webm", { loop: true }).play();  // or "https://…", "camera", "test:ball"
shape.video = clip;                                          // clipped to the shape's path
clip.onready = ({ width, height }) => { /* … */ };
video.broadcast("camera", "http://icecast.example.org:8000/live.ogg", { password: "…" });
```

`examples/hello-video` is the 2011 07videoplay demo, ported.
`examples/hello-audio` and `examples/hello-pd` are the 2011 audio and Pure
Data demos, ported (hello-pd uses the original `440.pd`).
`examples/hello-net` asks to fetch from GitHub. `--allow-declared` grants
declared permissions without asking (development).

See `examples/` for the same idea in Rust (the `underweb` crate and
`export_page!`) and in C.

## Layout

| Path | What |
|---|---|
| `apps/underweb-app` | the browser (eframe/egui on wgpu) |
| `crates/underweb-core` | the engine as a library: package store and builder, spawns runners, receives frames |
| `crates/underweb-runner` | the per-package process: V8 or native loader, frame loop |
| `crates/underweb-guest` | the page API (scene of layouts and shapes) and its cairo/pango renderer |
| `crates/underweb-sandbox` | per-OS lockdown |
| `crates/underweb-ipc` | socket messages with fd passing, shared memory |
| `crates/underweb-abi` | the C ABI, source of `include/underweb.h` |
| `crates/underweb` | SDK for Rust packages |
| `crates/underweb-pkg` | `underweb.toml`, archives, release indexes, addresses, publishing |
| `crates/underweb-cli` | `uw`, the authoring tool |
| `crates/underweb-export` | the layout editor's export: a scene as a JS, TS, Rust or C package |
| `crates/underweb-capi` | the engine as a C library |
| `crates/underweb-media` | the media process: one sandboxed GStreamer pipeline per video stream |
| `crates/underweb-audio` | the audio ring, oscillators, the engine with Pure Data, and the browser's mixer |
| `crates/underweb-trust` | signed statements, keys, the trust store and policy, signed lists |
| `crates/underweb-trust-atproto` | Bluesky/ATProto: identities, keys, statements, follows |
| `sdk/js` | TypeScript types for JS/TS packages |
| `xtask` | `cargo xtask examples / header / test / dist` |

## License

GPL-3.0-or-later, like the original (to be confirmed before a release).
