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
    /// A pango font description such as `"Serif Bold 18"`.
    pub fn set_font(&self, font: &str) -> &Self {
        let h = host();
        (h.shape_set_font)(h.ctx, self.0, cstring(font).as_ptr());
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
