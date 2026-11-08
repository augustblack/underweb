//! `[permissions]`: what a package may ask for.
//!
//! ```toml
//! [permissions]
//! net.tcp.connect = ["example.org:7000", "*.example.org:443", "127.0.0.1:*"]
//! net.tcp.listen  = [9000]                      # accept connections on this port
//! net.udp.bind    = [57120]                     # receive UDP on this port
//! net.udp.send    = ["192.168.1.20:57110", "*:9000"]
//! fetch           = ["https://api.example.org/v1/"]   # URL prefixes
//! camera          = true
//! ```
//!
//! Declaring is necessary but not sufficient: the user (or, from phase 4,
//! the trust policy) grants each declared entry. A request outside every
//! declared entry is refused without asking. Storage needs no permission:
//! every package has a private data directory.

use anyhow::{Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    #[serde(default)]
    pub net: Net,
    #[serde(default)]
    pub fetch: Vec<String>,
    #[serde(default)]
    pub camera: bool,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Net {
    #[serde(default)]
    pub tcp: Tcp,
    #[serde(default)]
    pub udp: Udp,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Tcp {
    #[serde(default)]
    pub connect: Vec<String>,
    #[serde(default)]
    pub listen: Vec<u16>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Udp {
    #[serde(default)]
    pub bind: Vec<u16>,
    #[serde(default)]
    pub send: Vec<String>,
}

/// One declared entry: the unit the user grants.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Permission {
    TcpConnect(String),
    TcpListen(u16),
    UdpBind(u16),
    UdpSend(String),
    Fetch(String),
    Camera,
}

impl Permission {
    /// Stable key, used to remember grants.
    pub fn key(&self) -> String {
        match self {
            Permission::TcpConnect(p) => format!("net.tcp.connect {p}"),
            Permission::TcpListen(p) => format!("net.tcp.listen {p}"),
            Permission::UdpBind(p) => format!("net.udp.bind {p}"),
            Permission::UdpSend(p) => format!("net.udp.send {p}"),
            Permission::Fetch(p) => format!("fetch {p}"),
            Permission::Camera => "camera".into(),
        }
    }

    /// For permission prompts: "… wants to <describe>".
    pub fn describe(&self) -> String {
        let endpoint = |p: &str| {
            let (host, port) = p.rsplit_once(':').unwrap_or((p, "*"));
            let host = match host {
                "*" => "any computer".to_string(),
                h if h.starts_with("*.") => format!("any computer in {}", &h[2..]),
                h => h.to_string(),
            };
            let port = if port == "*" {
                "any port".to_string()
            } else {
                format!("port {port}")
            };
            format!("{host} on {port}")
        };
        match self {
            Permission::TcpConnect(p) => format!("connect to {}", endpoint(p)),
            Permission::TcpListen(p) => format!("accept connections from your network on port {p}"),
            Permission::UdpBind(p) => format!("receive UDP messages from your network on port {p}"),
            Permission::UdpSend(p) => format!("send UDP messages to {}", endpoint(p)),
            Permission::Fetch(p) => format!("load data from {p}"),
            Permission::Camera => "use your camera".into(),
        }
    }
}

impl Permissions {
    pub fn validate(&self) -> Result<()> {
        for p in self.net.tcp.connect.iter().chain(&self.net.udp.send) {
            parse_endpoint(p)?;
        }
        for p in &self.fetch {
            if !(p.starts_with("https://") || p.starts_with("http://")) || url_host(p).is_none() {
                bail!(
                    "fetch entry {p:?} must be an http(s) URL prefix like \"https://api.example.org/\""
                );
            }
        }
        if self.net.tcp.listen.contains(&0) || self.net.udp.bind.contains(&0) {
            bail!(
                "listen/bind ports must be fixed numbers (ephemeral UDP ports need only net.udp.send)"
            );
        }
        Ok(())
    }

    pub fn declared(&self) -> Vec<Permission> {
        let mut v = Vec::new();
        v.extend(
            self.net
                .tcp
                .connect
                .iter()
                .cloned()
                .map(Permission::TcpConnect),
        );
        v.extend(
            self.net
                .tcp
                .listen
                .iter()
                .copied()
                .map(Permission::TcpListen),
        );
        v.extend(self.net.udp.bind.iter().copied().map(Permission::UdpBind));
        v.extend(self.net.udp.send.iter().cloned().map(Permission::UdpSend));
        v.extend(self.fetch.iter().cloned().map(Permission::Fetch));
        if self.camera {
            v.push(Permission::Camera);
        }
        v
    }

    pub fn tcp_connect(&self, host: &str, port: u16) -> Option<Permission> {
        self.net
            .tcp
            .connect
            .iter()
            .find(|p| endpoint_matches(p, host, port))
            .map(|p| Permission::TcpConnect(p.clone()))
    }

    pub fn tcp_listen(&self, port: u16) -> Option<Permission> {
        self.net
            .tcp
            .listen
            .contains(&port)
            .then_some(Permission::TcpListen(port))
    }

    pub fn udp_bind(&self, port: u16) -> Option<Permission> {
        self.net
            .udp
            .bind
            .contains(&port)
            .then_some(Permission::UdpBind(port))
    }

    /// Opening an ephemeral UDP socket needs no grant of its own, only
    /// something to send to.
    pub fn may_open_udp(&self) -> bool {
        !self.net.udp.send.is_empty()
    }

    pub fn udp_send(&self, host: &str, port: u16) -> Option<Permission> {
        self.net
            .udp
            .send
            .iter()
            .find(|p| endpoint_matches(p, host, port))
            .map(|p| Permission::UdpSend(p.clone()))
    }

    pub fn camera(&self) -> Option<Permission> {
        self.camera.then_some(Permission::Camera)
    }

    pub fn fetch(&self, url: &str) -> Option<Permission> {
        self.fetch
            .iter()
            .find(|p| url_matches(p, url))
            .map(|p| Permission::Fetch(p.clone()))
    }
}

fn parse_endpoint(p: &str) -> Result<(&str, Option<u16>)> {
    let Some((host, port)) = p.rsplit_once(':') else {
        bail!("{p:?}: use host:port")
    };
    let port = match port {
        "*" => None,
        n => Some(
            n.parse::<u16>()
                .map_err(|_| anyhow::anyhow!("{p:?}: bad port"))?,
        ),
    };
    let host_ok = host == "*"
        || (!host.is_empty()
            && host
                .trim_start_matches("*.")
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'));
    if !host_ok || host[1..].contains('*') {
        bail!("{p:?}: host must be a name, an address, *.domain or *");
    }
    Ok((host, port))
}

fn endpoint_matches(pattern: &str, host: &str, port: u16) -> bool {
    let Ok((h, p)) = parse_endpoint(pattern) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let h = h.to_ascii_lowercase();
    let host_ok = h == "*"
        || h == host
        || h.strip_prefix("*.")
            .is_some_and(|suffix| host.ends_with(&format!(".{suffix}")));
    host_ok && p.is_none_or(|p| p == port)
}

fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then_some(host)
}

/// `url` is under `prefix`, on a path boundary: "https://a.org" covers
/// "https://a.org/x" but not "https://a.org.evil.com/".
fn url_matches(prefix: &str, url: &str) -> bool {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return false;
    }
    let (Some(ph), Some(uh)) = (url_host(prefix), url_host(url)) else {
        return false;
    };
    if !ph.eq_ignore_ascii_case(uh) || url.contains('@') {
        return false;
    }
    match url.strip_prefix(prefix) {
        Some(rest) => prefix.ends_with('/') || rest.is_empty() || rest.starts_with(['/', '?', '#']),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perms(text: &str) -> Permissions {
        let p: Permissions = toml::from_str(text).unwrap();
        p.validate().unwrap();
        p
    }

    #[test]
    fn matching() {
        let p = perms(
            "net.tcp.connect = ['example.org:7000', '*.osc.net:*']\n\
             net.udp.send = ['*:57120']\n\
             net.udp.bind = [9000]\n\
             fetch = ['https://api.example.org/v1/', 'http://127.0.0.1:8000']\n",
        );
        assert!(p.tcp_connect("example.org", 7000).is_some());
        assert!(p.tcp_connect("EXAMPLE.org", 7000).is_some());
        assert!(p.tcp_connect("example.org", 7001).is_none());
        assert!(p.tcp_connect("a.osc.net", 1).is_some());
        assert!(p.tcp_connect("osc.net.evil.com", 1).is_none());
        assert!(p.tcp_connect("evilosc.net", 1).is_none());
        assert!(p.udp_send("10.0.0.1", 57120).is_some());
        assert!(p.udp_send("10.0.0.1", 57121).is_none());
        assert!(p.udp_bind(9000).is_some() && p.udp_bind(9001).is_none());
        assert!(p.fetch("https://api.example.org/v1/users?x=1").is_some());
        assert!(p.fetch("https://api.example.org/v2/").is_none());
        assert!(p.fetch("https://api.example.org.evil.com/v1/").is_none());
        assert!(p.fetch("https://api.example.org/v1/@evil.com/").is_none());
        assert!(p.fetch("http://127.0.0.1:8000/x").is_some());
        assert!(p.fetch("http://127.0.0.1:80001/x").is_none());
        assert_eq!(p.declared().len(), 6);
        assert_eq!(
            Permission::TcpConnect("*.osc.net:*".into()).describe(),
            "connect to any computer in osc.net on any port"
        );
    }

    #[test]
    fn rejects_bad_entries() {
        for bad in [
            "net.tcp.connect = ['example.org']",
            "net.tcp.connect = ['ex*ample.org:1']",
            "fetch = ['example.org/']",
            "net.udp.bind = [0]",
        ] {
            let p: Permissions = toml::from_str(bad).unwrap();
            assert!(p.validate().is_err(), "{bad}");
        }
        assert!(toml::from_str::<Permissions>("microphone = true").is_err());
    }
}
