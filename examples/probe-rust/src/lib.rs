//! Sandbox probe. Tries operations a package must not be able to do and
//! logs one line per attempt:
//!
//! ```text
//! PROBE <name> allowed|denied <detail>
//! PROBE done
//! ```
//!
//! Paths and ports come from `?key=value` parameters so the test can point
//! it at real targets: `pkg` (its own directory, must stay readable),
//! `secret` (a file outside it), `home` (a directory to list), `write_to`
//! (a path outside it), `port` (a local TCP listener). `crash=1` aborts.

use std::io::Write;
use std::net::TcpStream;
use std::time::Duration;

use underweb::{Page, Window};

#[derive(Default)]
struct Probe;

fn report(w: &Window, name: &str, result: Result<String, String>) {
    match result {
        Ok(detail) => w.log(&format!("PROBE {name} allowed {detail}")),
        Err(detail) => w.log(&format!("PROBE {name} denied {detail}")),
    }
}

impl Page for Probe {
    fn activate(&mut self, w: &Window) {
        w.set_title("sandbox probe");
        if w.param("crash").is_some() {
            std::process::abort();
        }
        if let Some(pkg) = w.param("pkg") {
            let r = std::fs::read_to_string(format!("{pkg}/underweb.toml"));
            report(
                w,
                "read_own_package",
                r.map(|s| format!("{} bytes", s.len()))
                    .map_err(|e| e.to_string()),
            );
        }
        if let Some(secret) = w.param("secret") {
            let r = std::fs::read_to_string(&secret);
            report(
                w,
                "read_outside",
                r.map(|s| format!("{} bytes", s.len()))
                    .map_err(|e| e.to_string()),
            );
        }
        if let Some(home) = w.param("home") {
            let r = std::fs::read_dir(&home);
            report(
                w,
                "list_home",
                r.map(|d| format!("{} entries", d.count()))
                    .map_err(|e| e.to_string()),
            );
        }
        if let Some(path) = w.param("write_to") {
            let r = std::fs::File::create(&path).and_then(|mut f| f.write_all(b"escaped"));
            report(
                w,
                "write_outside",
                r.map(|_| path.clone()).map_err(|e| e.to_string()),
            );
        }
        if let Some(port) = w.param("port") {
            let addr = format!("127.0.0.1:{port}").parse().unwrap();
            let r = TcpStream::connect_timeout(&addr, Duration::from_secs(2));
            report(
                w,
                "tcp_connect",
                r.map(|_| addr.to_string()).map_err(|e| e.to_string()),
            );
        }
        let r = if cfg!(windows) {
            std::process::Command::new("cmd.exe")
                .args(["/C", "exit 0"])
                .status()
        } else {
            std::process::Command::new("/bin/sh")
                .args(["-c", "true"])
                .status()
        };
        report(
            w,
            "exec",
            r.map(|s| s.to_string()).map_err(|e| e.to_string()),
        );
        w.log("PROBE done");
    }
}

underweb::export_page!(Probe);
