//! Pure Data through libpd. One Pd per process (libpd is global), which
//! suits Underweb: one package per process.
//!
//! Every libpd call happens under [`PD`]. The audio thread only ever
//! `try_lock`s it, so a page opening a patch makes at most a block of
//! silence, never a stall. Messages Pd sends to subscribed receivers are
//! queued for the page.

use std::ffi::{CStr, CString, c_char, c_int};
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use libpd_sys as pd;

use crate::{BLOCK, CHANNELS};

static PD: Mutex<Option<PdState>> = Mutex::new(None);
static INBOX: Mutex<Vec<PdMessage>> = Mutex::new(Vec::new());

/// A cap so a patch spamming messages can't eat memory.
const MAX_QUEUED: usize = 10_000;

struct PdState {
    rate: u32,
    patches: Vec<usize>,
    bound: Vec<(String, usize)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PdValue {
    Bang,
    Float(f32),
    Symbol(String),
    List(Vec<PdValue>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PdMessage {
    /// The receiver name, or "print" for Pd's console.
    pub receiver: String,
    pub value: PdValue,
}

fn push(receiver: *const c_char, value: PdValue) {
    let receiver = unsafe { CStr::from_ptr(receiver) }
        .to_string_lossy()
        .into_owned();
    let mut q = INBOX.lock().unwrap();
    if q.len() < MAX_QUEUED {
        q.push(PdMessage { receiver, value });
    }
}

unsafe extern "C" fn print_hook(s: *const c_char) {
    let text = unsafe { CStr::from_ptr(s) }
        .to_string_lossy()
        .trim_end()
        .to_string();
    if !text.is_empty() {
        let mut q = INBOX.lock().unwrap();
        if q.len() < MAX_QUEUED {
            q.push(PdMessage {
                receiver: "print".into(),
                value: PdValue::Symbol(text),
            });
        }
    }
}
unsafe extern "C" fn bang_hook(r: *const c_char) {
    push(r, PdValue::Bang)
}
unsafe extern "C" fn float_hook(r: *const c_char, x: f32) {
    push(r, PdValue::Float(x))
}
unsafe extern "C" fn symbol_hook(r: *const c_char, s: *const c_char) {
    let s = unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
    push(r, PdValue::Symbol(s))
}
unsafe extern "C" fn list_hook(r: *const c_char, argc: c_int, argv: *mut pd::t_atom) {
    let mut items = Vec::new();
    let mut a = argv;
    for _ in 0..argc {
        unsafe {
            if pd::libpd_is_float(a) != 0 {
                items.push(PdValue::Float(pd::libpd_get_float(a)));
            } else if pd::libpd_is_symbol(a) != 0 {
                items.push(PdValue::Symbol(
                    CStr::from_ptr(pd::libpd_get_symbol(a))
                        .to_string_lossy()
                        .into_owned(),
                ));
            }
            a = pd::libpd_next_atom(a);
        }
    }
    push(r, PdValue::List(items))
}

fn cstr(s: &str) -> Result<CString> {
    CString::new(s).context("name contains a NUL byte")
}

/// libpd is built with per-thread instances; every thread that calls it
/// must select the one we use.
fn use_main_instance() {
    unsafe { pd::libpd_set_instance(pd::libpd_main_instance()) }
}

fn with_pd<R>(f: impl FnOnce(&mut PdState) -> Result<R>) -> Result<R> {
    let mut g = PD.lock().unwrap();
    let state = g.as_mut().context("Pd is not started")?;
    use_main_instance();
    f(state)
}

/// Start Pd (once) at `rate`, stereo out, DSP on.
pub fn init(rate: u32) -> Result<()> {
    let mut g = PD.lock().unwrap();
    if let Some(s) = g.as_ref() {
        if s.rate != rate {
            bail!("Pd already runs at {} Hz", s.rate);
        }
        return Ok(());
    }
    unsafe {
        pd::libpd_set_banghook(Some(bang_hook));
        pd::libpd_set_floathook(Some(float_hook));
        pd::libpd_set_symbolhook(Some(symbol_hook));
        pd::libpd_set_listhook(Some(list_hook));
        pd::libpd_init();
        // whole lines, not fragments
        pd::libpd_set_printhook(Some(pd::libpd_print_concatenator));
        pd::libpd_set_concatenated_printhook(Some(print_hook));
        if pd::libpd_init_audio(0, CHANNELS as c_int, rate as c_int) != 0 {
            bail!("libpd_init_audio failed");
        }
        pd::libpd_start_message(1);
        pd::libpd_add_float(1.0);
        pd::libpd_finish_message(c"pd".as_ptr(), c"dsp".as_ptr());
    }
    *g = Some(PdState {
        rate,
        patches: Vec::new(),
        bound: Vec::new(),
    });
    // libpd's start-up chatter isn't the page's business
    INBOX.lock().unwrap().clear();
    Ok(())
}

pub fn is_started() -> bool {
    PD.lock().unwrap().is_some()
}

/// Open a patch file (abstractions next to it are found too).
pub fn open(path: &Path) -> Result<()> {
    let dir = path.parent().context("patch path has no directory")?;
    let name = path.file_name().context("patch path has no file name")?;
    if !path.exists() {
        bail!("no patch at {}", path.display());
    }
    let (name, dir) = (
        cstr(&name.to_string_lossy())?,
        cstr(&dir.to_string_lossy())?,
    );
    with_pd(|s| {
        let p = unsafe { pd::libpd_openfile(name.as_ptr(), dir.as_ptr()) };
        if p.is_null() {
            bail!("Pd could not open {}", path.display());
        }
        s.patches.push(p as usize);
        Ok(())
    })
}

/// Receive what Pd sends to `[s receiver]`.
pub fn subscribe(receiver: &str) -> Result<()> {
    let r = cstr(receiver)?;
    with_pd(|s| {
        if !s.bound.iter().any(|(n, _)| n == receiver) {
            let p = unsafe { pd::libpd_bind(r.as_ptr()) };
            s.bound.push((receiver.to_string(), p as usize));
        }
        Ok(())
    })
}

/// Send to `[r receiver]`. Unknown receivers are ignored, as in Pd.
pub fn send(receiver: &str, value: &PdValue) -> Result<()> {
    let r = cstr(receiver)?;
    with_pd(|_| {
        unsafe {
            match value {
                PdValue::Bang => {
                    pd::libpd_bang(r.as_ptr());
                }
                PdValue::Float(x) => {
                    pd::libpd_float(r.as_ptr(), *x);
                }
                PdValue::Symbol(sym) => {
                    let sym = cstr(sym)?;
                    pd::libpd_symbol(r.as_ptr(), sym.as_ptr());
                }
                PdValue::List(items) => {
                    let syms: Vec<CString> = items
                        .iter()
                        .filter_map(|i| {
                            if let PdValue::Symbol(s) = i {
                                cstr(s).ok()
                            } else {
                                None
                            }
                        })
                        .collect();
                    let mut sym = syms.iter();
                    pd::libpd_start_message(items.len() as c_int);
                    for i in items {
                        match i {
                            PdValue::Float(x) => pd::libpd_add_float(*x),
                            PdValue::Symbol(_) => {
                                pd::libpd_add_symbol(sym.next().unwrap().as_ptr())
                            }
                            _ => {}
                        }
                    }
                    pd::libpd_finish_list(r.as_ptr());
                }
            }
        }
        Ok(())
    })
}

/// Messages Pd sent since the last call.
pub fn take_messages() -> Vec<PdMessage> {
    std::mem::take(&mut *INBOX.lock().unwrap())
}

/// Audio thread: add one block of Pd's output (interleaved stereo) to
/// `out`. Returns false (and adds nothing) if Pd is busy or not started.
pub fn process_into(out: &mut [f32; BLOCK * CHANNELS]) -> bool {
    let Ok(g) = PD.try_lock() else { return false };
    if g.is_none() {
        return false;
    }
    use_main_instance();
    let mut buf = [0f32; BLOCK * CHANNELS];
    let input = [0f32; 1];
    unsafe { pd::libpd_process_float(1, input.as_ptr(), buf.as_mut_ptr()) };
    for (o, x) in out.iter_mut().zip(buf) {
        *o += x;
    }
    true
}
