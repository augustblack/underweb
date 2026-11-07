//! The frame loop shared by all runner modes.

use std::cell::RefCell;

use anyhow::{Context, Result};
use underweb_abi::UwPointerEvent;
use underweb_guest::{Scene, cairo};
use underweb_ipc::{Channel, GuestMsg, HostMsg, Shm};

/// A loaded package, whatever language it is written in.
pub trait Page {
    fn activate(&mut self) -> Result<()>;
    fn update(&mut self, dt: f64) -> Result<()>;
    fn pointer(&mut self, ev: &UwPointerEvent) -> Result<()>;
    /// Raw cairo drawing on top of the scene (native packages only).
    fn draw(&mut self, _cr: &cairo::Context, _w: f64, _h: f64) {}
    fn deactivate(&mut self) {}
}

struct Frames {
    generation: u32,
    shm: Shm,
    width: u32,
    height: u32,
    stride: u32,
    scale: f64,
    free: [bool; 2],
}

pub fn run(chan: &Channel, scene: &RefCell<Scene>, page: &mut dyn Page) -> Result<()> {
    page.activate().context("activate")?;
    let mut frames: Option<Frames> = None;
    let mut title = String::new();
    let mut animating = false;

    loop {
        // report window state changes
        {
            let s = scene.borrow();
            if s.title != title {
                title = s.title.clone();
                chan.send(&GuestMsg::Title(title.clone()), None)?;
            }
            if s.animating != animating {
                animating = s.animating;
                chan.send(&GuestMsg::Animating(animating), None)?;
            }
        }
        draw_if_needed(chan, scene, page, frames.as_mut())?;

        let Some((msg, fd)) = chan.recv::<HostMsg>()? else {
            break;
        };
        match msg {
            HostMsg::Configure {
                generation,
                width,
                height,
                stride,
                scale,
            } => {
                let fd = fd.context("Configure without a frame buffer")?;
                let len = stride as usize * height as usize * 2;
                frames = Some(Frames {
                    generation,
                    shm: Shm::from_fd(fd, len)?,
                    width,
                    height,
                    stride,
                    scale,
                    free: [true, true],
                });
                let mut s = scene.borrow_mut();
                s.width = width as f64 / scale;
                s.height = height as f64 / scale;
                s.request_redraw();
            }
            HostMsg::Tick { dt } => page.update(dt).context("update")?,
            HostMsg::Pointer { kind, button, x, y } => {
                let shape = scene.borrow().hit_test(x, y);
                page.pointer(&UwPointerEvent {
                    kind,
                    button,
                    x,
                    y,
                    shape,
                })
                .context("pointer")?;
            }
            HostMsg::Release { generation, index } => {
                if let Some(f) = frames.as_mut().filter(|f| f.generation == generation) {
                    f.free[(index & 1) as usize] = true;
                }
            }
            HostMsg::Shutdown => break,
        }
    }
    page.deactivate();
    Ok(())
}

fn draw_if_needed(
    chan: &Channel,
    scene: &RefCell<Scene>,
    page: &mut dyn Page,
    frames: Option<&mut Frames>,
) -> Result<()> {
    let Some(f) = frames else { return Ok(()) };
    if !scene.borrow_mut().take_dirty() {
        return Ok(());
    }
    let Some(index) = f.free.iter().position(|&free| free) else {
        // both buffers are with the host; draw when one comes back
        scene.borrow_mut().request_redraw();
        return Ok(());
    };
    let buf_len = f.stride as usize * f.height as usize;
    let ptr = unsafe { f.shm.as_ptr().add(index * buf_len) };
    let (w, h) = (f.width as f64 / f.scale, f.height as f64 / f.scale);
    {
        let s = scene.borrow();
        unsafe {
            underweb_guest::render_frame(&s, ptr, f.width, f.height, f.stride, f.scale, |cr| {
                page.draw(cr, w, h)
            })
        };
    }
    f.free[index] = false;
    chan.send(
        &GuestMsg::Frame {
            generation: f.generation,
            index: index as u32,
        },
        None,
    )
}
