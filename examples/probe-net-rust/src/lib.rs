//! Network and storage from a native package: writes a file in its data
//! directory, connects to `?tcp=` and echoes, listens on `?listen=` and
//! answers whoever connects. Logs "PROBE ..." lines.

use underweb::net::{self, Handle, Kind, NetEvent};
use underweb::{Page, Window};

#[derive(Default)]
struct Probe {
    conn: Option<Handle>,
    listener: Option<Handle>,
}

impl Page for Probe {
    fn activate(&mut self, w: &Window) {
        w.set_title("net probe (rust)");
        let file = w.data_dir().join("visits");
        let n: u32 = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
            + 1;
        let r = std::fs::write(&file, n.to_string());
        w.log(&format!(
            "PROBE storage {n} {}",
            if r.is_ok() { "ok" } else { "failed" }
        ));

        if let Some(port) = w.param("tcp").and_then(|p| p.parse().ok()) {
            self.conn = Some(net::tcp_connect("127.0.0.1", port));
        }
        if let Some(port) = w.param("listen").and_then(|p| p.parse().ok()) {
            self.listener = Some(net::tcp_listen(port));
        }
    }

    fn net(&mut self, w: &Window, ev: &NetEvent) {
        match ev.kind {
            Kind::Connected if Some(ev.handle) == self.conn => ev.handle.send(b"hello"),
            Kind::Connected if Some(ev.handle) == self.listener => w.log("PROBE listening"),
            Kind::Data if Some(ev.handle) == self.conn => {
                w.log(&format!("PROBE tcp {}", String::from_utf8_lossy(ev.data)));
            }
            Kind::Accepted => {
                let conn = ev.new_handle.unwrap();
                conn.send(b"welcome");
                w.log("PROBE accepted");
            }
            Kind::Error => w.log(&format!("PROBE error {}", ev.text)),
            _ => {}
        }
    }
}

underweb::export_page!(Probe);
