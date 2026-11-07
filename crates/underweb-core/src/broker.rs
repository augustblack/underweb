//! The permission broker: the only way a package reaches the network.
//!
//! Runners are sandboxed and cannot open sockets. They send [`NetOp`]s;
//! the broker checks each against the package's declared permissions and
//! the user's grants, asks the embedder (the browser shows a prompt) when
//! a declared permission hasn't been decided yet, and does the I/O itself,
//! relaying data both ways. Undeclared requests are refused without asking.
//!
//! Grants are per declared entry ("net.tcp.connect example.org:7000"), for
//! this session or always; "always" grants are stored per package identity
//! (its origin), so a package with the same name from someone else doesn't
//! inherit them.

use std::collections::{HashMap, HashSet};
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use underweb_ipc::{Channel, HOST_HANDLE_BASE, HostMsg, NetEvent, NetEventKind, NetOp};
use underweb_pkg::{Permission, Permissions};

use crate::GuestEvent;

/// What to do with declared permissions nobody has decided on yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPolicy {
    /// Emit [`GuestEvent::PermissionRequest`] and wait for
    /// [`crate::Guest::answer`].
    Ask,
    /// Grant everything the package declared (tests, trusted embedders).
    AllowDeclared,
    /// Refuse everything.
    DenyAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Until the package is closed.
    AllowOnce,
    /// Remembered for this package.
    AllowAlways,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantState {
    Undecided,
    AllowedOnce,
    AllowedAlways,
    Denied,
}

const MAX_FETCH: u64 = 32 << 20;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

enum Sock {
    Tcp {
        stream: TcpStream,
        writer: mpsc::Sender<Vec<u8>>,
    },
    Udp {
        socket: Arc<UdpSocket>,
        closed: Arc<AtomicBool>,
    },
    Listener {
        closed: Arc<AtomicBool>,
    },
}

struct Grants {
    always: HashSet<String>,
    session: HashSet<String>,
    denied: HashSet<String>,
    file: Option<PathBuf>,
}

impl Grants {
    fn load(file: Option<PathBuf>) -> Grants {
        let always = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|t| {
                t.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        Grants {
            always,
            session: HashSet::new(),
            denied: HashSet::new(),
            file,
        }
    }

    fn save(&self) {
        let Some(f) = &self.file else { return };
        let mut lines: Vec<&String> = self.always.iter().collect();
        lines.sort();
        let text = std::iter::once("# permissions granted with \"always allow\"".to_string())
            .chain(lines.into_iter().cloned())
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(f, text + "\n");
    }

    fn state(&self, key: &str) -> GrantState {
        if self.denied.contains(key) {
            GrantState::Denied
        } else if self.always.contains(key) {
            GrantState::AllowedAlways
        } else if self.session.contains(key) {
            GrantState::AllowedOnce
        } else {
            GrantState::Undecided
        }
    }
}

struct Pending {
    permission: Permission,
    ops: Vec<(u32, NetOp)>,
}

pub(crate) struct Broker {
    chan: Arc<Channel>,
    perms: Permissions,
    policy: PermissionPolicy,
    grants: Mutex<Grants>,
    pending: Mutex<HashMap<u64, Pending>>,
    next_prompt: AtomicU64,
    sockets: Mutex<HashMap<u32, Sock>>,
    next_host_handle: AtomicU32,
    events: mpsc::Sender<GuestEvent>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    agent: ureq::Agent,
    closed: AtomicBool,
}

impl Broker {
    pub(crate) fn new(
        chan: Arc<Channel>,
        perms: Permissions,
        policy: PermissionPolicy,
        grants_file: Option<PathBuf>,
        events: mpsc::Sender<GuestEvent>,
        wake: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Arc<Broker> {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(60)))
            .max_redirects(0) // a redirect could leave the allowed prefix
            .http_status_as_error(false)
            .user_agent(concat!("underweb/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Arc::new(Broker {
            chan,
            perms,
            policy,
            grants: Mutex::new(Grants::load(grants_file)),
            pending: Mutex::new(HashMap::new()),
            next_prompt: AtomicU64::new(1),
            sockets: Mutex::new(HashMap::new()),
            next_host_handle: AtomicU32::new(HOST_HANDLE_BASE),
            events,
            wake,
            agent,
            closed: AtomicBool::new(false),
        })
    }

    fn emit(&self, e: GuestEvent) {
        let _ = self.events.send(e);
        if let Some(w) = &self.wake {
            w();
        }
    }

    fn reply(&self, ev: NetEvent) {
        let _ = self.chan.send(&HostMsg::Net(ev), None);
    }

    /// Declared permissions and what the user decided about each.
    pub(crate) fn permissions(&self) -> Vec<(Permission, GrantState)> {
        let g = self.grants.lock().unwrap();
        self.perms
            .declared()
            .into_iter()
            .map(|p| {
                let s = g.state(&p.key());
                (p, s)
            })
            .collect()
    }

    /// Forget any decision about `permission`.
    pub(crate) fn forget(&self, permission: &Permission) {
        let key = permission.key();
        let mut g = self.grants.lock().unwrap();
        g.session.remove(&key);
        g.denied.remove(&key);
        if g.always.remove(&key) {
            g.save();
        }
    }

    /// A request from the runner.
    pub(crate) fn request(self: &Arc<Self>, handle: u32, op: NetOp) {
        let needed = match &op {
            NetOp::TcpConnect { host, port } => self.perms.tcp_connect(host, *port),
            NetOp::TcpListen { port } => self.perms.tcp_listen(*port),
            NetOp::UdpOpen { port: 0 } => {
                if self.perms.may_open_udp() {
                    return self.perform(handle, op);
                }
                None
            }
            NetOp::UdpOpen { port } => self.perms.udp_bind(*port),
            NetOp::UdpSend { host, port, .. } => self.perms.udp_send(host, *port),
            NetOp::Fetch { url, .. } => self.perms.fetch(url),
            NetOp::Send { .. } | NetOp::Close => return self.perform(handle, op),
        };
        let Some(permission) = needed else {
            return self.reply(NetEvent::error(
                handle,
                format!(
                    "permission denied: {} is not declared in underweb.toml",
                    describe_op(&op)
                ),
            ));
        };
        let key = permission.key();
        let state = self.grants.lock().unwrap().state(&key);
        match (state, self.policy) {
            (GrantState::AllowedOnce | GrantState::AllowedAlways, _)
            | (GrantState::Undecided, PermissionPolicy::AllowDeclared) => self.perform(handle, op),
            (GrantState::Denied, _) | (GrantState::Undecided, PermissionPolicy::DenyAll) => self
                .reply(NetEvent::error(
                    handle,
                    format!("permission denied: {}", permission.describe()),
                )),
            (GrantState::Undecided, PermissionPolicy::Ask) => {
                let mut pending = self.pending.lock().unwrap();
                if let Some(p) = pending.values_mut().find(|p| p.permission == permission) {
                    p.ops.push((handle, op)); // already asking
                    return;
                }
                let id = self.next_prompt.fetch_add(1, Ordering::Relaxed);
                let description = permission.describe();
                pending.insert(
                    id,
                    Pending {
                        permission: permission.clone(),
                        ops: vec![(handle, op)],
                    },
                );
                drop(pending);
                self.emit(GuestEvent::PermissionRequest {
                    id,
                    permission,
                    description,
                });
            }
        }
    }

    /// The user's answer to a [`GuestEvent::PermissionRequest`].
    pub(crate) fn answer(self: &Arc<Self>, id: u64, decision: Decision) {
        let Some(p) = self.pending.lock().unwrap().remove(&id) else {
            return;
        };
        let key = p.permission.key();
        {
            let mut g = self.grants.lock().unwrap();
            match decision {
                Decision::AllowOnce => {
                    g.session.insert(key);
                }
                Decision::AllowAlways => {
                    g.always.insert(key);
                    g.save();
                }
                Decision::Deny => {
                    g.denied.insert(key);
                }
            }
        }
        for (handle, op) in p.ops {
            if decision == Decision::Deny {
                self.reply(NetEvent::error(
                    handle,
                    format!("permission denied: {}", p.permission.describe()),
                ));
            } else {
                self.perform(handle, op);
            }
        }
    }

    fn perform(self: &Arc<Self>, handle: u32, op: NetOp) {
        if self.closed.load(Ordering::Relaxed) {
            return;
        }
        match op {
            NetOp::TcpConnect { host, port } => {
                let b = self.clone();
                std::thread::spawn(move || match connect(&host, port) {
                    Ok(stream) => {
                        let mut ev = NetEvent::new(handle, NetEventKind::Connected);
                        ev.text = stream
                            .peer_addr()
                            .map(|a| a.to_string())
                            .unwrap_or_default();
                        b.add_tcp(handle, stream);
                        b.reply(ev);
                    }
                    Err(e) => b.reply(NetEvent::error(
                        handle,
                        format!("connecting to {host}:{port}: {e}"),
                    )),
                });
            }
            NetOp::TcpListen { port } => match TcpListener::bind(("0.0.0.0", port)) {
                Ok(listener) => {
                    let closed = Arc::new(AtomicBool::new(false));
                    self.sockets.lock().unwrap().insert(
                        handle,
                        Sock::Listener {
                            closed: closed.clone(),
                        },
                    );
                    let mut ev = NetEvent::new(handle, NetEventKind::Connected);
                    ev.text = listener
                        .local_addr()
                        .map(|a| a.to_string())
                        .unwrap_or_default();
                    self.reply(ev);
                    let b = self.clone();
                    std::thread::spawn(move || b.accept_loop(handle, listener, closed));
                }
                Err(e) => self.reply(NetEvent::error(
                    handle,
                    format!("listening on port {port}: {e}"),
                )),
            },
            NetOp::UdpOpen { port } => match UdpSocket::bind(("0.0.0.0", port)) {
                Ok(socket) => {
                    let socket = Arc::new(socket);
                    let closed = Arc::new(AtomicBool::new(false));
                    self.sockets.lock().unwrap().insert(
                        handle,
                        Sock::Udp {
                            socket: socket.clone(),
                            closed: closed.clone(),
                        },
                    );
                    let mut ev = NetEvent::new(handle, NetEventKind::Connected);
                    ev.text = socket
                        .local_addr()
                        .map(|a| a.to_string())
                        .unwrap_or_default();
                    self.reply(ev);
                    let b = self.clone();
                    std::thread::spawn(move || b.udp_loop(handle, socket, closed));
                }
                Err(e) => self.reply(NetEvent::error(
                    handle,
                    format!("opening UDP port {port}: {e}"),
                )),
            },
            NetOp::Send { data } => {
                let sockets = self.sockets.lock().unwrap();
                match sockets.get(&handle) {
                    Some(Sock::Tcp { writer, .. }) => {
                        let _ = writer.send(data);
                    }
                    _ => self.reply(NetEvent::error(handle, "not an open TCP connection")),
                }
            }
            NetOp::UdpSend { host, port, data } => {
                let socket = match self.sockets.lock().unwrap().get(&handle) {
                    Some(Sock::Udp { socket, .. }) => Some(socket.clone()),
                    _ => None,
                };
                match socket {
                    Some(s) => {
                        if let Err(e) = s.send_to(&data, (host.as_str(), port)) {
                            self.reply(NetEvent::error(
                                handle,
                                format!("sending to {host}:{port}: {e}"),
                            ));
                        }
                    }
                    None => self.reply(NetEvent::error(handle, "not an open UDP socket")),
                }
            }
            NetOp::Close => {
                if let Some(s) = self.sockets.lock().unwrap().remove(&handle) {
                    close(s);
                }
            }
            NetOp::Fetch {
                method,
                url,
                headers,
                body,
            } => {
                let b = self.clone();
                std::thread::spawn(move || {
                    let ev = match b.fetch(&method, &url, &headers, body) {
                        Ok(ev) => NetEvent { handle, ..ev },
                        Err(e) => NetEvent::error(handle, format!("fetching {url}: {e:#}")),
                    };
                    b.reply(ev);
                });
            }
        }
    }

    fn add_tcp(self: &Arc<Self>, handle: u32, stream: TcpStream) {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let (Ok(read), Ok(mut write)) = (stream.try_clone(), stream.try_clone()) else {
            return self.reply(NetEvent::error(handle, "socket clone failed"));
        };
        self.sockets
            .lock()
            .unwrap()
            .insert(handle, Sock::Tcp { stream, writer: tx });
        std::thread::spawn(move || {
            for chunk in rx {
                if write.write_all(&chunk).is_err() {
                    break;
                }
            }
        });
        let b = self.clone();
        std::thread::spawn(move || b.read_loop(handle, read));
    }

    fn read_loop(&self, handle: u32, mut stream: TcpStream) {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut ev = NetEvent::new(handle, NetEventKind::Data);
                    ev.data = buf[..n].to_vec();
                    self.reply(ev);
                }
            }
        }
        if self.sockets.lock().unwrap().remove(&handle).is_some() {
            self.reply(NetEvent::new(handle, NetEventKind::Closed));
        }
    }

    fn accept_loop(self: &Arc<Self>, handle: u32, listener: TcpListener, closed: Arc<AtomicBool>) {
        let _ = listener.set_nonblocking(true);
        while !closed.load(Ordering::Relaxed) && !self.closed.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, peer)) => {
                    let _ = stream.set_nonblocking(false);
                    let new_handle = self.next_host_handle.fetch_add(1, Ordering::Relaxed);
                    self.add_tcp(new_handle, stream);
                    let mut ev = NetEvent::new(handle, NetEventKind::Accepted);
                    ev.new_handle = new_handle;
                    ev.text = peer.to_string();
                    self.reply(ev);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(POLL),
                Err(_) => std::thread::sleep(POLL),
            }
        }
    }

    fn udp_loop(&self, handle: u32, socket: Arc<UdpSocket>, closed: Arc<AtomicBool>) {
        let _ = socket.set_read_timeout(Some(POLL));
        let mut buf = vec![0u8; 65536];
        while !closed.load(Ordering::Relaxed) && !self.closed.load(Ordering::Relaxed) {
            if let Ok((n, from)) = socket.recv_from(&mut buf) {
                let mut ev = NetEvent::new(handle, NetEventKind::Data);
                ev.data = buf[..n].to_vec();
                ev.text = from.to_string();
                self.reply(ev);
            }
        }
    }

    fn fetch(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Vec<u8>,
    ) -> anyhow::Result<NetEvent> {
        let method: ureq::http::Method = method.parse()?;
        let mut req = ureq::http::Request::builder().method(method).uri(url);
        for (k, v) in headers {
            if !matches!(
                k.to_ascii_lowercase().as_str(),
                "host" | "content-length" | "cookie"
            ) {
                req = req.header(k, v);
            }
        }
        let mut resp = self.agent.run(req.body(body)?)?;
        let mut ev = NetEvent::new(0, NetEventKind::Response);
        ev.status = resp.status().as_u16() as u32;
        ev.text = resp
            .headers()
            .iter()
            .map(|(k, v)| format!("{}: {}", k, v.to_str().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n");
        ev.data = resp
            .body_mut()
            .with_config()
            .limit(MAX_FETCH)
            .read_to_vec()?;
        Ok(ev)
    }

    /// The package is gone: close everything it had open.
    pub(crate) fn close_all(&self) {
        self.closed.store(true, Ordering::Relaxed);
        for (_, s) in self.sockets.lock().unwrap().drain() {
            close(s);
        }
    }
}

fn close(s: Sock) {
    match s {
        Sock::Tcp { stream, .. } => {
            let _ = stream.shutdown(Shutdown::Both);
        }
        Sock::Udp { closed, .. } | Sock::Listener { closed } => {
            closed.store(true, Ordering::Relaxed)
        }
    }
}

fn connect(host: &str, port: u16) -> std::io::Result<TcpStream> {
    let mut last = std::io::Error::new(ErrorKind::NotFound, "no addresses");
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn describe_op(op: &NetOp) -> String {
    match op {
        NetOp::TcpConnect { host, port } => format!("connecting to {host}:{port}"),
        NetOp::TcpListen { port } => format!("listening on TCP port {port}"),
        NetOp::UdpOpen { port } => format!("opening UDP port {port}"),
        NetOp::UdpSend { host, port, .. } => format!("sending UDP to {host}:{port}"),
        NetOp::Fetch { url, .. } => format!("fetching {url}"),
        NetOp::Send { .. } => "sending".into(),
        NetOp::Close => "closing".into(),
    }
}
