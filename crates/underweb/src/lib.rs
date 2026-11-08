//! Write Underweb packages in Rust.
//!
//! ```ignore
//! use underweb::{Layout, Page, Shape, Window};
//!
//! #[derive(Default)]
//! struct Hello;
//!
//! impl Page for Hello {
//!     fn activate(&mut self, window: &Window) {
//!         window.set_title("Rust shape");
//!         let layout = Layout::new();
//!         window.add_layout(&layout);
//!         let s = Shape::new();
//!         s.rectangle(100.0, 100.0, 300.0, 200.0).set_fill_color(1.0, 0.0, 0.0, 1.0);
//!         s.set_text("Hello from Rust");
//!         layout.append(&s);
//!     }
//! }
//!
//! underweb::export_page!(Hello);
//! ```
//!
//! Build the package as a `cdylib`. Everything goes through the C ABI in
//! `underweb-abi`, so packages don't link the runner, and a package built
//! with an older SDK keeps working as long as the ABI version matches.

use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicPtr, Ordering};

#[cfg(feature = "cairo")]
pub use cairo;
pub use underweb_abi as abi;
use underweb_abi::UwHost;

static HOST: AtomicPtr<UwHost> = AtomicPtr::new(std::ptr::null_mut());

fn host() -> &'static UwHost {
    let p = HOST.load(Ordering::Acquire);
    assert!(!p.is_null(), "underweb API used before activate()");
    unsafe { &*p }
}

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap()
}

/// The browser window (tab) the page is shown in.
pub struct Window(());

impl Window {
    pub fn set_title(&self, title: &str) {
        let h = host();
        (h.set_title)(h.ctx, cstring(title).as_ptr())
    }
    pub fn add_layout(&self, layout: &Layout) {
        let h = host();
        (h.add_layout)(h.ctx, layout.0)
    }
    /// With animation on, [`Page::update`] runs every display frame.
    pub fn set_animating(&self, on: bool) {
        let h = host();
        (h.set_animating)(h.ctx, on)
    }
    pub fn redraw(&self) {
        let h = host();
        (h.request_redraw)(h.ctx)
    }
    pub fn width(&self) -> f64 {
        let h = host();
        (h.width)(h.ctx)
    }
    pub fn height(&self) -> f64 {
        let h = host();
        (h.height)(h.ctx)
    }
    /// A `?key=value` parameter from the package URL.
    pub fn param(&self, key: &str) -> Option<String> {
        let h = host();
        let p = (h.param)(h.ctx, cstring(key).as_ptr());
        (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
    pub fn log(&self, message: &str) {
        let h = host();
        (h.log)(h.ctx, cstring(message).as_ptr())
    }
    /// The package's private directory, the only place it can write.
    pub fn data_dir(&self) -> std::path::PathBuf {
        let h = host_v2();
        let p = (h.data_dir)(h.ctx);
        unsafe { CStr::from_ptr(p) }
            .to_string_lossy()
            .into_owned()
            .into()
    }
}

/// The ABI 2 functions exist only in newer runners.
fn host_v2() -> &'static UwHost {
    let h = host();
    assert!(
        h.abi_version >= 2,
        "this browser is too old for network and storage functions"
    );
    h
}

fn host_v3() -> &'static UwHost {
    let h = host();
    assert!(
        h.abi_version >= 3,
        "this browser is too old for audio functions"
    );
    h
}

/// Sound, rendered in the package's process off the page's thread and
/// played by the browser.
pub mod audio {
    use std::ffi::c_void;
    use std::sync::Mutex;

    use super::host_v3;

    /// The browser's sample rate; 0 without audio.
    pub fn rate() -> u32 {
        let h = host_v3();
        (h.audio_rate)(h.ctx)
    }
    pub fn start() -> bool {
        let h = host_v3();
        (h.audio_start)(h.ctx)
    }
    pub fn stop() {
        let h = host_v3();
        (h.audio_stop)(h.ctx)
    }
    pub fn set_volume(v: f32) {
        let h = host_v3();
        (h.audio_volume)(h.ctx, v)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Wave {
        Sine = 0,
        Saw = 1,
        Square = 2,
        Triangle = 3,
        Noise = 4,
    }

    /// An oscillator; stops when dropped.
    pub struct Osc(u32);

    impl Osc {
        /// `pan` from -1 (left) to 1 (right). None if all are in use.
        pub fn new(wave: Wave, freq: f32, amp: f32, pan: f32) -> Option<Osc> {
            let h = host_v3();
            let id = (h.osc_new)(h.ctx, wave as u32, freq, amp, pan);
            (id != 0).then_some(Osc(id))
        }
        pub fn set(&self, freq: f32, amp: f32, pan: f32) {
            let h = host_v3();
            (h.osc_set)(h.ctx, self.0, freq, amp, pan)
        }
    }

    impl Drop for Osc {
        fn drop(&mut self) {
            let h = host_v3();
            (h.osc_free)(h.ctx, self.0)
        }
    }

    /// The latest `frames` frames played, mono.
    pub fn scope(frames: usize) -> Vec<f32> {
        let h = host_v3();
        let mut v = vec![0f32; frames];
        let n = (h.audio_scope)(h.ctx, v.as_mut_ptr(), frames);
        v.truncate(n);
        v
    }

    type Boxed = Box<dyn FnMut(&mut [f32]) + Send>;
    static CURRENT: Mutex<Option<usize>> = Mutex::new(None);

    extern "C" fn trampoline(user: *mut c_void, frames: *mut f32, count: usize, channels: u32) {
        let f = unsafe { &mut *(user as *mut Boxed) };
        let buf = unsafe { std::slice::from_raw_parts_mut(frames, count * channels as usize) };
        f(buf);
    }

    /// Your own sound: `f` adds to interleaved stereo frames, on the audio
    /// thread. It must not block (no locks, no I/O, no allocation).
    pub fn set_callback(f: impl FnMut(&mut [f32]) + Send + 'static) {
        let boxed: Box<Boxed> = Box::new(Box::new(f));
        let user = Box::into_raw(boxed);
        let h = host_v3();
        (h.audio_callback)(h.ctx, Some(trampoline), user as *mut c_void);
        // the old one is no longer called once audio_callback returns
        if let Some(old) = CURRENT.lock().unwrap().replace(user as usize) {
            drop(unsafe { Box::from_raw(old as *mut Boxed) });
        }
    }

    pub fn clear_callback() {
        let h = host_v3();
        (h.audio_callback)(h.ctx, None, std::ptr::null_mut());
        if let Some(old) = CURRENT.lock().unwrap().take() {
            drop(unsafe { Box::from_raw(old as *mut Boxed) });
        }
    }
}

fn host_v4() -> &'static UwHost {
    let h = host();
    assert!(
        h.abi_version >= 4,
        "this browser is too old for video functions"
    );
    h
}

/// Video streams, decoded by the browser in their own sandboxed processes
/// and shown in shapes with [`crate::Shape::set_video`].
pub mod video {
    use super::{cstring, host_v4};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Video(pub u32);

    #[derive(Debug, Clone, Copy, Default)]
    pub struct Info {
        pub width: u32,
        pub height: u32,
        pub position: f64,
        pub duration: f64,
    }

    /// A file in the package, an http(s) URL, "camera", or "test:<pattern>".
    pub fn open(source: &str, looping: bool) -> Video {
        let h = host_v4();
        Video((h.media_open)(h.ctx, cstring(source).as_ptr(), looping))
    }

    /// Show `source` and send it to an Icecast server
    /// (`http://host:port/mount.ogg`).
    pub fn broadcast(source: &str, url: &str, password: &str) -> Video {
        let h = host_v4();
        Video((h.media_broadcast)(
            h.ctx,
            cstring(source).as_ptr(),
            cstring(url).as_ptr(),
            cstring(password).as_ptr(),
        ))
    }

    impl Video {
        pub fn play(&self) {
            let h = host_v4();
            (h.media_play)(h.ctx, self.0)
        }
        pub fn pause(&self) {
            let h = host_v4();
            (h.media_pause)(h.ctx, self.0)
        }
        pub fn seek(&self, seconds: f64) {
            let h = host_v4();
            (h.media_seek)(h.ctx, self.0, seconds)
        }
        pub fn close(&self) {
            let h = host_v4();
            (h.media_close)(h.ctx, self.0)
        }
        /// None until the first frame.
        pub fn info(&self) -> Option<Info> {
            let h = host_v4();
            let mut i = underweb_abi::UwMediaInfo::default();
            (h.media_info)(h.ctx, self.0, &mut i).then_some(Info {
                width: i.width,
                height: i.height,
                position: i.position,
                duration: i.duration,
            })
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Kind {
        Ready,
        Ended,
        Broadcasting,
        Error,
    }

    #[derive(Debug, Clone)]
    pub struct MediaEvent {
        pub video: Video,
        pub kind: Kind,
        pub width: u32,
        pub height: u32,
        /// For errors: what went wrong.
        pub text: String,
    }
}

/// Pure Data patches shipped with the package.
pub mod pd {
    use super::{cstring, host_v3};

    /// Open a patch (path relative to the package); starts sound.
    pub fn open(path: &str) -> bool {
        let h = host_v3();
        (h.pd_open)(h.ctx, cstring(path).as_ptr())
    }
    pub fn bang(receiver: &str) {
        let h = host_v3();
        (h.pd_bang)(h.ctx, cstring(receiver).as_ptr())
    }
    pub fn float(receiver: &str, value: f32) {
        let h = host_v3();
        (h.pd_float)(h.ctx, cstring(receiver).as_ptr(), value)
    }
    pub fn symbol(receiver: &str, symbol: &str) {
        let h = host_v3();
        (h.pd_symbol)(h.ctx, cstring(receiver).as_ptr(), cstring(symbol).as_ptr())
    }
    pub fn list(receiver: &str, values: &[f32]) {
        let h = host_v3();
        (h.pd_list)(
            h.ctx,
            cstring(receiver).as_ptr(),
            values.as_ptr(),
            values.len(),
        )
    }
    /// Get what Pd sends to `[s receiver]` in [`crate::Page::pd`].
    pub fn subscribe(receiver: &str) {
        let h = host_v3();
        (h.pd_subscribe)(h.ctx, cstring(receiver).as_ptr())
    }

    #[derive(Debug, Clone, PartialEq)]
    pub enum Value {
        Bang,
        Float(f32),
        Symbol(String),
        List(Vec<f32>),
    }

    #[derive(Debug, Clone)]
    pub struct PdEvent {
        pub receiver: String,
        pub value: Value,
    }
}

/// Networking through the browser. Every call returns a [`net::Handle`] at
/// once; results arrive as [`net::NetEvent`]s in [`Page::net`]. Each needs a
/// permission declared in underweb.toml and granted by the user.
pub mod net {
    use super::{cstring, host_v2};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Handle(pub u32);

    pub fn tcp_connect(host: &str, port: u16) -> Handle {
        let h = host_v2();
        Handle((h.net_tcp_connect)(h.ctx, cstring(host).as_ptr(), port))
    }
    pub fn tcp_listen(port: u16) -> Handle {
        let h = host_v2();
        Handle((h.net_tcp_listen)(h.ctx, port))
    }
    /// Port 0: an ephemeral port, for sending.
    pub fn udp_open(port: u16) -> Handle {
        let h = host_v2();
        Handle((h.net_udp_open)(h.ctx, port))
    }
    /// An HTTP(S) request. Redirects are not followed.
    pub fn fetch(method: &str, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Handle {
        let h = host_v2();
        let headers: String = headers.iter().map(|(k, v)| format!("{k}: {v}\n")).collect();
        Handle((h.fetch)(
            h.ctx,
            cstring(method).as_ptr(),
            cstring(url).as_ptr(),
            cstring(&headers).as_ptr(),
            body.as_ptr(),
            body.len(),
        ))
    }

    impl Handle {
        /// Write to a TCP connection.
        pub fn send(&self, data: &[u8]) {
            let h = host_v2();
            (h.net_send)(h.ctx, self.0, data.as_ptr(), data.len())
        }
        pub fn udp_send(&self, host: &str, port: u16, data: &[u8]) {
            let h = host_v2();
            (h.net_udp_send)(
                h.ctx,
                self.0,
                cstring(host).as_ptr(),
                port,
                data.as_ptr(),
                data.len(),
            )
        }
        pub fn close(&self) {
            let h = host_v2();
            (h.net_close)(h.ctx, self.0)
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Kind {
        /// Connected, listening, or UDP socket open; `text` is the address.
        Connected,
        /// `data` arrived; for UDP, `text` is the sender.
        Data,
        Closed,
        /// `text` says why, including refused permissions.
        Error,
        /// A listener accepted `new_handle`; `text` is the peer.
        Accepted,
        /// A fetch finished: `status`, headers in `text`, body in `data`.
        Response,
    }

    #[derive(Debug, Clone, Copy)]
    pub struct NetEvent<'a> {
        pub kind: Kind,
        pub handle: Handle,
        pub new_handle: Option<Handle>,
        pub status: u32,
        pub data: &'a [u8],
        pub text: &'a str,
    }
}

/// An ordered list of shapes. Later layouts draw on top.
pub struct Layout(u32);

impl Layout {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Layout {
        let h = host();
        Layout((h.layout_new)(h.ctx))
    }
    pub fn append(&self, shape: &Shape) -> &Self {
        let h = host();
        (h.layout_append)(h.ctx, self.0, shape.0);
        self
    }
    /// Paint the window under this layout's shapes (`None`: nothing).
    pub fn set_background(&self, color: Option<(f64, f64, f64, f64)>) -> &Self {
        let h = host_v5();
        let (r, g, b, a) = color.unwrap_or_default();
        (h.layout_set_background)(h.ctx, self.0, r, g, b, a);
        self
    }
}

fn host_v5() -> &'static UwHost {
    let h = host();
    assert!(
        h.abi_version >= 5,
        "this browser is too old for transforms and images"
    );
    h
}

/// An affine transform, as `cairo_matrix_t`: (x, y) lands on
/// (xx·x + xy·y + x0, yx·x + yy·y + y0). Like cairo, each step happens in
/// the coordinates made by the steps before it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub xx: f64,
    pub yx: f64,
    pub xy: f64,
    pub yy: f64,
    pub x0: f64,
    pub y0: f64,
}

impl Default for Matrix {
    fn default() -> Self {
        Matrix::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix {
        xx: 1.0,
        yx: 0.0,
        xy: 0.0,
        yy: 1.0,
        x0: 0.0,
        y0: 0.0,
    };

    pub const fn new(xx: f64, yx: f64, xy: f64, yy: f64, x0: f64, y0: f64) -> Matrix {
        Matrix {
            xx,
            yx,
            xy,
            yy,
            x0,
            y0,
        }
    }

    /// `self` applied after `first`.
    pub fn multiply(&self, first: &Matrix) -> Matrix {
        Matrix {
            xx: self.xx * first.xx + self.xy * first.yx,
            yx: self.yx * first.xx + self.yy * first.yx,
            xy: self.xx * first.xy + self.xy * first.yy,
            yy: self.yx * first.xy + self.yy * first.yy,
            x0: self.xx * first.x0 + self.xy * first.y0 + self.x0,
            y0: self.yx * first.x0 + self.yy * first.y0 + self.y0,
        }
    }
    pub fn translate(&self, x: f64, y: f64) -> Matrix {
        self.multiply(&Matrix::new(1.0, 0.0, 0.0, 1.0, x, y))
    }
    pub fn scale(&self, sx: f64, sy: f64) -> Matrix {
        self.multiply(&Matrix::new(sx, 0.0, 0.0, sy, 0.0, 0.0))
    }
    /// Radians, clockwise on screen.
    pub fn rotate(&self, angle: f64) -> Matrix {
        let (s, c) = angle.sin_cos();
        self.multiply(&Matrix::new(c, s, -s, c, 0.0, 0.0))
    }
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.xx * x + self.xy * y + self.x0,
            self.yx * x + self.yy * y + self.y0,
        )
    }
    pub fn invert(&self) -> Option<Matrix> {
        let d = self.xx * self.yy - self.xy * self.yx;
        if d == 0.0 || !d.is_finite() {
            return None;
        }
        let (xx, xy, yx, yy) = (self.yy / d, -self.xy / d, -self.yx / d, self.xx / d);
        Some(Matrix::new(
            xx,
            yx,
            xy,
            yy,
            -(xx * self.x0 + xy * self.y0),
            -(yx * self.x0 + yy * self.y0),
        ))
    }
}

/// A decoded picture (PNG, JPEG or GIF), shown with [`Shape::set_image`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Image(pub u32);

impl Image {
    /// A file in the package. `None` if it can't be read or decoded (the
    /// reason goes to the page's log).
    pub fn open(path: &str) -> Option<Image> {
        let h = host_v5();
        let id = (h.image_open)(h.ctx, cstring(path).as_ptr());
        (id != 0).then_some(Image(id))
    }
    pub fn from_bytes(data: &[u8]) -> Option<Image> {
        let h = host_v5();
        let id = (h.image_from_bytes)(h.ctx, data.as_ptr(), data.len());
        (id != 0).then_some(Image(id))
    }
    pub fn size(&self) -> (u32, u32) {
        let h = host_v5();
        let (mut w, mut hh) = (0, 0);
        (h.image_size)(h.ctx, self.0, &mut w, &mut hh);
        (w, hh)
    }
}

/// A path with fill, stroke and text. Cheap to copy: it's a handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shape(u32);

macro_rules! shape_fn {
    ($(#[$m:meta])* $name:ident => $f:ident ( $($arg:ident: $t:ty),* )) => {
        $(#[$m])*
        pub fn $name(&self, $($arg: $t),*) -> &Self {
            let h = host();
            (h.$f)(h.ctx, self.0, $($arg),*);
            self
        }
    };
}

impl Shape {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Shape {
        let h = host();
        Shape((h.shape_new)(h.ctx))
    }
    shape_fn!(clear_path => shape_clear_path());
    shape_fn!(rectangle => shape_rectangle(x: f64, y: f64, w: f64, h: f64));
    shape_fn!(move_to => shape_move_to(x: f64, y: f64));
    shape_fn!(line_to => shape_line_to(x: f64, y: f64));
    shape_fn!(close_path => shape_close_path());
    shape_fn!(set_fill_color => shape_set_fill_color(r: f64, g: f64, b: f64, a: f64));
    shape_fn!(set_stroke_color => shape_set_stroke_color(r: f64, g: f64, b: f64, a: f64));
    shape_fn!(set_stroke_width => shape_set_stroke_width(w: f64));
    shape_fn!(set_text_color => shape_set_text_color(r: f64, g: f64, b: f64, a: f64));

    #[allow(clippy::too_many_arguments)]
    pub fn curve_to(&self, x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) -> &Self {
        let h = host();
        (h.shape_curve_to)(h.ctx, self.0, x1, y1, x2, y2, x3, y3);
        self
    }
    pub fn set_text(&self, text: &str) -> &Self {
        let h = host();
        (h.shape_set_text)(h.ctx, self.0, cstring(text).as_ptr());
        self
    }
    /// Show a video inside the shape, or none.
    pub fn set_video(&self, video: Option<video::Video>) -> &Self {
        let h = host_v4();
        (h.shape_set_video)(h.ctx, self.0, video.map_or(0, |v| v.0));
        self
    }
    /// A pango font description such as `"Serif Bold 18"`.
    pub fn set_font(&self, font: &str) -> &Self {
        let h = host();
        (h.shape_set_font)(h.ctx, self.0, cstring(font).as_ptr());
        self
    }
    /// A circle made of four Bezier arcs.
    pub fn circle(&self, cx: f64, cy: f64, r: f64) -> &Self {
        let k = 0.5523 * r;
        self.move_to(cx + r, cy)
            .curve_to(cx + r, cy + k, cx + k, cy + r, cx, cy + r)
            .curve_to(cx - k, cy + r, cx - r, cy + k, cx - r, cy)
            .curve_to(cx - r, cy - k, cx - k, cy - r, cx, cy - r)
            .curve_to(cx + k, cy - r, cx + r, cy - k, cx + r, cy)
            .close_path()
    }
    /// Shown in the layout editor.
    pub fn set_name(&self, name: &str) -> &Self {
        let h = host_v5();
        (h.shape_set_name)(h.ctx, self.0, cstring(name).as_ptr());
        self
    }
    /// From the shape's own coordinates to the window's.
    pub fn set_matrix(&self, m: Matrix) -> &Self {
        let h = host_v5();
        (h.shape_set_matrix)(h.ctx, self.0, m.xx, m.yx, m.xy, m.yy, m.x0, m.y0);
        self
    }
    /// `markup`: the text is Pango markup. `flow`: it flows word by word
    /// inside the outline instead of its bounding box.
    pub fn set_text_mode(&self, markup: bool, flow: bool) -> &Self {
        let h = host_v5();
        let flags =
            if markup { abi::UW_TEXT_MARKUP } else { 0 } | if flow { abi::UW_TEXT_FLOW } else { 0 };
        (h.shape_set_text_flags)(h.ctx, self.0, flags);
        self
    }
    /// Draw an image at the shape's origin, clipped to the outline. A
    /// shape with no outline yet gets the image's rectangle.
    pub fn set_image(&self, image: Option<Image>) -> &Self {
        let h = host_v5();
        (h.shape_set_image)(h.ctx, self.0, image.map_or(0, |i| i.0));
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    Move,
    Press,
    Release,
}

#[derive(Debug, Clone, Copy)]
pub struct PointerEvent {
    pub kind: PointerKind,
    pub button: u32,
    pub x: f64,
    pub y: f64,
    /// Topmost shape under the pointer.
    pub shape: Option<Shape>,
}

/// A page. Only `activate` is required.
pub trait Page: Default + 'static {
    fn activate(&mut self, window: &Window);
    /// Called every display frame while animating; `dt` in seconds.
    fn update(&mut self, _window: &Window, _dt: f64) {}
    fn pointer(&mut self, _window: &Window, _ev: &PointerEvent) {}
    /// A network event for a handle from [`net`].
    fn net(&mut self, _window: &Window, _ev: &net::NetEvent) {}
    /// Something Pd sent to a receiver you subscribed to.
    fn pd(&mut self, _window: &Window, _ev: &pd::PdEvent) {}
    /// Something happened to a video stream.
    fn media(&mut self, _window: &Window, _ev: &video::MediaEvent) {}
    /// Draw with raw cairo on top of the layouts, in logical pixels.
    #[cfg(feature = "cairo")]
    fn draw(&mut self, _window: &Window, _cr: &cairo::Context, _width: f64, _height: f64) {}
    fn deactivate(&mut self, _window: &Window) {}
}

#[doc(hidden)]
pub mod __private {
    use super::*;
    use std::cell::RefCell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::thread::LocalKey;
    use underweb_abi::UwPointerEvent;

    pub type Slot<P> = LocalKey<RefCell<Option<P>>>;

    /// Panics must not unwind into the runner: log and abort, and the
    /// browser shows the tab as crashed.
    fn guard(f: impl FnOnce()) {
        if let Err(e) = catch_unwind(AssertUnwindSafe(f)) {
            let msg = e
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| e.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            if !HOST.load(Ordering::Acquire).is_null() {
                Window(()).log(&format!("package panicked: {msg}"));
            }
            std::process::abort();
        }
    }

    fn with<P: Page>(slot: &'static Slot<P>, f: impl FnOnce(&mut P, &Window)) {
        guard(|| {
            slot.with(|p| {
                if let Some(page) = p.borrow_mut().as_mut() {
                    f(page, &Window(()))
                }
            })
        })
    }

    pub unsafe fn activate<P: Page>(slot: &'static Slot<P>, host: *const UwHost) {
        HOST.store(host as *mut UwHost, Ordering::Release);
        guard(|| slot.with(|p| *p.borrow_mut() = Some(P::default())));
        with(slot, |p, w| p.activate(w));
    }

    pub unsafe fn update<P: Page>(slot: &'static Slot<P>, dt: f64) {
        with(slot, |p, w| p.update(w, dt));
    }

    pub unsafe fn pointer<P: Page>(slot: &'static Slot<P>, ev: *const UwPointerEvent) {
        let ev = unsafe { *ev };
        let kind = match ev.kind {
            underweb_abi::UW_POINTER_PRESS => PointerKind::Press,
            underweb_abi::UW_POINTER_RELEASE => PointerKind::Release,
            _ => PointerKind::Move,
        };
        let ev = PointerEvent {
            kind,
            button: ev.button,
            x: ev.x,
            y: ev.y,
            shape: (ev.shape != 0).then_some(Shape(ev.shape)),
        };
        with(slot, |p, w| p.pointer(w, &ev));
    }

    #[cfg(feature = "cairo")]
    pub unsafe fn draw<P: Page>(
        slot: &'static Slot<P>,
        cr: *mut std::ffi::c_void,
        width: f64,
        height: f64,
    ) {
        let cr = unsafe { cairo::Context::from_raw_none(cr as *mut cairo::ffi::cairo_t) };
        with(slot, |p, w| p.draw(w, &cr, width, height));
    }

    pub unsafe fn net<P: Page>(slot: &'static Slot<P>, ev: *const underweb_abi::UwNetEvent) {
        use underweb_abi::*;
        let ev = unsafe { *ev };
        let kind = match ev.kind {
            UW_NET_CONNECTED => net::Kind::Connected,
            UW_NET_DATA => net::Kind::Data,
            UW_NET_CLOSED => net::Kind::Closed,
            UW_NET_ACCEPTED => net::Kind::Accepted,
            UW_NET_RESPONSE => net::Kind::Response,
            _ => net::Kind::Error,
        };
        let data = if ev.data.is_null() || ev.len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(ev.data, ev.len) }
        };
        let text = if ev.text.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(ev.text) }
                .to_string_lossy()
                .into_owned()
        };
        let e = net::NetEvent {
            kind,
            handle: net::Handle(ev.handle),
            new_handle: (ev.new_handle != 0).then_some(net::Handle(ev.new_handle)),
            status: ev.status,
            data,
            text: &text,
        };
        with(slot, |p, w| p.net(w, &e));
    }

    pub unsafe fn pd<P: Page>(slot: &'static Slot<P>, ev: *const underweb_abi::UwPdEvent) {
        use underweb_abi::*;
        let ev = unsafe { *ev };
        let s = |p: *const std::ffi::c_char| {
            if p.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
            }
        };
        let value = match ev.kind {
            UW_PD_FLOAT => pd::Value::Float(ev.value),
            UW_PD_SYMBOL => pd::Value::Symbol(s(ev.symbol)),
            UW_PD_LIST if !ev.list.is_null() => {
                pd::Value::List(unsafe { std::slice::from_raw_parts(ev.list, ev.len) }.to_vec())
            }
            UW_PD_LIST => pd::Value::List(Vec::new()),
            _ => pd::Value::Bang,
        };
        let e = pd::PdEvent {
            receiver: s(ev.receiver),
            value,
        };
        with(slot, |p, w| p.pd(w, &e));
    }

    pub unsafe fn media<P: Page>(slot: &'static Slot<P>, ev: *const underweb_abi::UwMediaEvent) {
        use underweb_abi::*;
        let ev = unsafe { *ev };
        let kind = match ev.kind {
            UW_MEDIA_READY => video::Kind::Ready,
            UW_MEDIA_ENDED => video::Kind::Ended,
            UW_MEDIA_BROADCASTING => video::Kind::Broadcasting,
            _ => video::Kind::Error,
        };
        let text = if ev.text.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(ev.text) }
                .to_string_lossy()
                .into_owned()
        };
        let e = video::MediaEvent {
            video: video::Video(ev.id),
            kind,
            width: ev.width,
            height: ev.height,
            text,
        };
        with(slot, |p, w| p.media(w, &e));
    }

    pub unsafe fn deactivate<P: Page>(slot: &'static Slot<P>) {
        with(slot, |p, w| p.deactivate(w));
        guard(|| slot.with(|p| *p.borrow_mut() = None));
    }
}

/// Export a [`Page`] type as this library's page.
#[macro_export]
macro_rules! export_page {
    ($page:ty) => {
        thread_local! {
            static __UW_PAGE: ::std::cell::RefCell<Option<$page>> = const { ::std::cell::RefCell::new(None) };
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn uw_abi_version() -> u32 {
            $crate::abi::UW_ABI_VERSION
        }
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn uw_activate(host: *const $crate::abi::UwHost) {
            unsafe { $crate::__private::activate::<$page>(&__UW_PAGE, host) }
        }
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn uw_update(_host: *const $crate::abi::UwHost, dt: f64) {
            unsafe { $crate::__private::update::<$page>(&__UW_PAGE, dt) }
        }
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn uw_pointer(
            _host: *const $crate::abi::UwHost,
            ev: *const $crate::abi::UwPointerEvent,
        ) {
            unsafe { $crate::__private::pointer::<$page>(&__UW_PAGE, ev) }
        }
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn uw_net(
            _host: *const $crate::abi::UwHost,
            ev: *const $crate::abi::UwNetEvent,
        ) {
            unsafe { $crate::__private::net::<$page>(&__UW_PAGE, ev) }
        }
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn uw_pd(_host: *const $crate::abi::UwHost, ev: *const $crate::abi::UwPdEvent) {
            unsafe { $crate::__private::pd::<$page>(&__UW_PAGE, ev) }
        }
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn uw_media(_host: *const $crate::abi::UwHost, ev: *const $crate::abi::UwMediaEvent) {
            unsafe { $crate::__private::media::<$page>(&__UW_PAGE, ev) }
        }
        $crate::__export_draw!($page, __UW_PAGE);
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn uw_deactivate(_host: *const $crate::abi::UwHost) {
            unsafe { $crate::__private::deactivate::<$page>(&__UW_PAGE) }
        }
    };
}

#[doc(hidden)]
#[cfg(feature = "cairo")]
#[macro_export]
macro_rules! __export_draw {
    ($page:ty, $slot:ident) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn uw_draw(
            _host: *const $crate::abi::UwHost,
            cr: *mut ::std::ffi::c_void,
            w: f64,
            h: f64,
        ) {
            unsafe { $crate::__private::draw::<$page>(&$slot, cr, w, h) }
        }
    };
}

#[doc(hidden)]
#[cfg(not(feature = "cairo"))]
#[macro_export]
macro_rules! __export_draw {
    ($page:ty, $slot:ident) => {};
}
