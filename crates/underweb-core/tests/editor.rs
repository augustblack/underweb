//! The layout editor's protocol, and the ported demos, with real runner
//! processes. Needs `cargo xtask examples`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use underweb_core::scene::{EditOp, Matrix, PathOp, Rgba, SceneDoc, ShapeDoc};
use underweb_core::{
    Frame, Guest, GuestEvent, PermissionPolicy, SpawnOptions, UW_POINTER_MOVE, UW_POINTER_PRESS,
    UW_POINTER_RELEASE,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn example(name: &str) -> PathBuf {
    root().join("examples").join(name)
}

struct Page {
    guest: Guest,
    doc: Option<SceneDoc>,
    frame: Option<Frame>,
    logs: Vec<String>,
}

impl Page {
    fn open(dir: &Path, permissions: PermissionPolicy, params: &[(&str, &str)]) -> Page {
        assert!(
            underweb_core::runner_path().exists(),
            "runner not built: run `cargo xtask examples` first"
        );
        let mut guest = Guest::spawn(
            dir,
            SpawnOptions {
                permissions,
                params: params
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                remember_grants: false,
                data_dir: Some(std::env::temp_dir().join("underweb-test-data/editor")),
                mixer: Some(underweb_core::Mixer::new(48000, 2)),
                ..Default::default()
            },
        )
        .unwrap();
        guest.configure(800, 500, 1.0).unwrap();
        Page {
            guest,
            doc: None,
            frame: None,
            logs: Vec::new(),
        }
    }

    /// Handle events until `done` (or 20 s).
    #[track_caller]
    fn until(&mut self, what: &str, done: impl Fn(&Page) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(self) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; logs: {:?}",
                self.logs
            );
            match self.guest.next_event(Duration::from_millis(50)) {
                Some(GuestEvent::Scene(d)) => self.doc = Some(d),
                Some(GuestEvent::Frame(f)) => self.frame = Some(f),
                Some(GuestEvent::Log(l)) => self.logs.push(l),
                Some(GuestEvent::Error(e)) => panic!("page failed: {e}"),
                _ => {}
            }
        }
    }

    /// Let events settle for a moment.
    fn settle(&mut self) {
        let end = Instant::now() + Duration::from_millis(400);
        self.until("nothing", |_| Instant::now() >= end);
    }

    fn click(&self, x: f64, y: f64) {
        self.guest.pointer(UW_POINTER_PRESS, 1, x, y).unwrap();
        self.guest.pointer(UW_POINTER_RELEASE, 1, x, y).unwrap();
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let f = self.frame.as_ref().expect("a frame");
        let i = ((y * f.width + x) * 4) as usize;
        f.rgba[i..i + 4].try_into().unwrap()
    }

    fn shape(&self, name: &str) -> ShapeDoc {
        self.doc
            .as_ref()
            .and_then(|d| d.shapes().find(|s| s.name == name).cloned())
            .unwrap_or_else(|| panic!("no shape {name:?}"))
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        let _ = self.guest.set_edit_mode(false);
    }
}

#[test]
fn the_editor_sees_and_changes_the_scene() {
    let mut p = Page::open(&example("demo-03-shapes"), PermissionPolicy::DenyAll, &[]);
    p.until("a frame", |p| p.frame.is_some());
    p.guest.set_edit_mode(true).unwrap();
    p.until("the scene", |p| p.doc.is_some());
    let names: Vec<String> = p
        .doc
        .as_ref()
        .unwrap()
        .shapes()
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(names, ["red", "green", "blue"]);
    let blue = p.shape("blue");
    assert_eq!(
        blue.matrix,
        Matrix::IDENTITY.translate(199.489502, 273.130676)
    );
    assert!(blue.flow);

    // paint the red shape solid black and move it to the far corner
    let mut red = p.shape("red");
    red.fill = Some(Rgba(0.0, 0.0, 0.0, 1.0));
    red.stroke = None;
    red.text = None;
    red.matrix = Matrix::IDENTITY.translate(450.0, 300.0);
    p.guest.edit(EditOp::SetShape(red.clone())).unwrap();
    p.until("the edit", |p| {
        p.shape("red").fill == Some(Rgba(0.0, 0.0, 0.0, 1.0))
    });
    assert_eq!(p.shape("red"), red);
    // a point inside the red outline, moved clear of the others:
    // (300, 150) + (450, 300)
    p.until("the frame", |p| {
        p.frame.is_some() && p.pixel(750, 450) == [0, 0, 0, 255]
    });

    // a new shape with a picture from the package... from another package:
    // the editor can only use files of the page it edits
    p.guest
        .edit(EditOp::NewShape {
            layout: 0,
            shape: ShapeDoc {
                name: "picture".into(),
                image: Some("../demo-06-image/void.png".into()),
                ..Default::default()
            },
        })
        .unwrap();
    p.until("the refusal", |p| {
        p.logs.iter().any(|l| l.contains("relative to the package"))
    });

    // restack and delete
    let green = p.shape("green").id;
    p.guest
        .edit(EditOp::Restack {
            id: green,
            up: true,
        })
        .unwrap();
    p.until("restacking", |p| {
        p.doc.as_ref().unwrap().shapes().last().map(|s| s.id) == Some(green)
    });
    p.guest.edit(EditOp::Delete(green)).unwrap();
    p.until("deleting", |p| {
        p.doc.as_ref().unwrap().shape(green).is_none()
    });
}

#[test]
fn the_page_gets_no_clicks_while_editing() {
    let mut p = Page::open(&example("hello-js"), PermissionPolicy::DenyAll, &[]);
    p.until("a frame", |p| p.frame.is_some());
    p.guest.set_edit_mode(true).unwrap();
    p.until("the scene", |p| p.doc.is_some());
    p.click(250.0, 285.0);
    p.settle();
    assert!(
        !p.logs.iter().any(|l| l.starts_with("clicked")),
        "{:?}",
        p.logs
    );
    p.guest.set_edit_mode(false).unwrap();
    p.click(250.0, 285.0);
    p.until("the click", |p| {
        p.logs.iter().any(|l| l.starts_with("clicked"))
    });
}

#[test]
fn the_editor_adds_pictures_from_the_package() {
    let mut p = Page::open(&example("demo-06-image"), PermissionPolicy::DenyAll, &[]);
    p.until("a frame", |p| p.frame.is_some());
    p.guest.set_edit_mode(true).unwrap();
    p.until("the scene", |p| p.doc.is_some());
    assert_eq!(
        p.doc.as_ref().unwrap().layouts[0].background,
        Some(Rgba(1.0, 0.9, 1.0, 1.0))
    );
    assert_eq!(p.shape("leap").image.as_deref(), Some("leap.jpg"));
    p.guest
        .edit(EditOp::NewShape {
            layout: p.doc.as_ref().unwrap().layouts[0].id,
            shape: ShapeDoc {
                name: "copy".into(),
                image: Some("void.png".into()),
                stroke: None,
                matrix: Matrix::IDENTITY.translate(700.0, 400.0),
                ..Default::default()
            },
        })
        .unwrap();
    p.until("the picture", |p| {
        p.doc.as_ref().unwrap().shapes().any(|s| s.name == "copy")
    });
    // the shape took the picture's size
    assert_eq!(
        p.shape("copy").path,
        vec![PathOp::Rectangle(0.0, 0.0, 320.0, 320.0)]
    );
    // and shows it: deep blue at (750, 450)
    p.until("the frame", |p| {
        p.frame.is_some() && {
            let [r, g, b, _] = p.pixel(750, 450);
            b > 100 && r < 80 && g < 100
        }
    });
}

#[test]
fn tree_rows_open_and_close() {
    let mut p = Page::open(&example("demo-13-tree"), PermissionPolicy::DenyAll, &[]);
    p.until("a frame", |p| p.frame.is_some());
    p.guest.set_edit_mode(true).unwrap();
    p.until("the scene", |p| p.doc.is_some());
    let rows = |p: &Page| {
        p.doc
            .as_ref()
            .unwrap()
            .shapes()
            .filter(|s| s.text.as_deref().is_some_and(|t| !t.is_empty()) && !s.path.is_empty())
            .count()
    };
    let open = rows(&p);
    p.guest.set_edit_mode(false).unwrap();
    // "Books" is the second row: y = 40 + 30 + 30
    p.click(100.0, 115.0);
    p.settle();
    p.guest.set_edit_mode(true).unwrap();
    p.doc = None;
    p.until("the scene", |p| p.doc.is_some());
    // four books (two cells each) are gone
    assert_eq!(rows(&p), open - 8);
}

#[test]
fn sliders_follow_the_pointer() {
    let mut p = Page::open(&example("demo-09-sliders"), PermissionPolicy::DenyAll, &[]);
    p.until("a frame", |p| p.frame.is_some());
    // drag the shape's red slider (x 60..78) from the bottom to the top
    p.guest.pointer(UW_POINTER_PRESS, 1, 69.0, 310.0).unwrap();
    p.guest.pointer(UW_POINTER_MOVE, 0, 69.0, 200.0).unwrap();
    p.guest.pointer(UW_POINTER_MOVE, 0, 69.0, 100.0).unwrap();
    p.guest.pointer(UW_POINTER_RELEASE, 1, 69.0, 100.0).unwrap();
    p.guest.set_edit_mode(true).unwrap();
    p.until("the scene", |p| p.doc.is_some());
    let fill = p.shape("target").fill.unwrap();
    assert_eq!((fill.0, fill.1, fill.2), (1.0, 0.6, 1.0), "red went to 1");
}

/// A free TCP port for the chat demo.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn chat_server_and_client_talk() {
    // a copy of the demo allowed to use a free port
    let port = free_port();
    let dir = std::env::temp_dir().join(format!("underweb-chat-{port}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(example("demo-10-chat/main.ts"), dir.join("main.ts")).unwrap();
    let manifest = std::fs::read_to_string(example("demo-10-chat/underweb.toml"))
        .unwrap()
        .replace("7007", &port.to_string());
    std::fs::write(dir.join("underweb.toml"), manifest).unwrap();

    let mut p = Page::open(
        &dir,
        PermissionPolicy::AllowDeclared,
        &[("port", &port.to_string())],
    );
    p.until("a frame", |p| p.frame.is_some());
    p.guest.set_edit_mode(true).unwrap();
    p.until("the connection", |p| {
        p.doc
            .as_ref()
            .and_then(|d| d.shapes().find(|s| s.name == "client"))
            .and_then(|s| s.text.clone())
            .is_some_and(|t| t.contains("connected"))
    });
    p.guest.set_edit_mode(false).unwrap();
    p.click(450.0, 410.0); // "Send a line"
    p.settle();
    p.guest.set_edit_mode(true).unwrap();
    p.until("the echo", |p| {
        p.doc
            .as_ref()
            .and_then(|d| d.shapes().find(|s| s.name == "client"))
            .and_then(|s| s.text.clone())
            .is_some_and(|t| t.contains("echo: 1# olleh"))
    });
    let server = p.shape(&format!("server, port {port}")).text.unwrap();
    assert!(server.contains("got: hello #1"), "{server}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn chat_without_permission_says_so() {
    let mut p = Page::open(&example("demo-10-chat"), PermissionPolicy::DenyAll, &[]);
    p.until("a frame", |p| p.frame.is_some());
    p.guest.set_edit_mode(true).unwrap();
    p.until("the refusal", |p| {
        p.doc
            .as_ref()
            .and_then(|d| d.shapes().find(|s| s.name.starts_with("server")))
            .and_then(|s| s.text.clone())
            .is_some_and(|t| t.contains("can't listen"))
    });
}
