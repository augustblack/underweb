//! What can go in the address bar.
//!
//! - a package directory or `file://` URL: `examples/hello-ts`
//! - `uw://<domain>[:port]/<name>[@version]`: the release index at
//!   `https://<domain>/.well-known/underweb/<name>.toml` (plain http only
//!   for `localhost`/`127.0.0.1`, for testing)
//!
//! Either may end in `?key=value&...` parameters for the page.

use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::valid_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    Local(PathBuf),
    Remote {
        host: String,
        name: String,
        version: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub address: Address,
    pub params: Vec<(String, String)>,
}

impl Address {
    /// For remote packages, where the release index lives.
    pub fn index_url(&self) -> Option<String> {
        match self {
            Address::Remote { host, name, .. } => {
                let scheme = if is_loopback(host) { "http" } else { "https" };
                Some(format!(
                    "{scheme}://{host}/.well-known/underweb/{name}.toml"
                ))
            }
            Address::Local(_) => None,
        }
    }
}

fn is_loopback(host: &str) -> bool {
    let h = host.rsplit_once(':').map_or(host, |(h, _)| h);
    h == "localhost" || h == "127.0.0.1"
}

pub fn parse_address(input: &str) -> Result<Parsed> {
    let input = input.trim();
    let (main, query) = input.split_once('?').unwrap_or((input, ""));
    let params = query
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k.to_string(), v.to_string())
        })
        .collect();

    let address = if let Some(rest) = main.strip_prefix("uw://") {
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let (name, version) = match path.trim_end_matches('/').split_once('@') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (path.trim_end_matches('/'), None),
        };
        let host_ok = !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':');
        if !host_ok {
            bail!("bad host in {main:?}");
        }
        if !valid_name(name) {
            bail!("bad package name in {main:?}: use uw://<domain>/<name>[@version]");
        }
        Address::Remote {
            host: host.to_ascii_lowercase(),
            name: name.to_string(),
            version,
        }
    } else if main.starts_with("http://") || main.starts_with("https://") {
        bail!(
            "web pages come with the web engine (phase 5); use uw://<domain>/<name> for packages"
        );
    } else {
        let path = main.strip_prefix("file://").unwrap_or(main);
        if path.is_empty() {
            bail!("empty address");
        }
        Address::Local(PathBuf::from(path))
    };
    Ok(Parsed { address, params })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        let p = parse_address("uw://Example.org/hello-ts@0.2?x=1&y").unwrap();
        assert_eq!(
            p.address,
            Address::Remote {
                host: "example.org".into(),
                name: "hello-ts".into(),
                version: Some("0.2".into())
            }
        );
        assert_eq!(
            p.params,
            vec![("x".into(), "1".into()), ("y".into(), "".into())]
        );
        assert_eq!(
            p.address.index_url().unwrap(),
            "https://example.org/.well-known/underweb/hello-ts.toml"
        );
        let local = parse_address("uw://127.0.0.1:8000/a").unwrap().address;
        assert_eq!(
            local.index_url().unwrap(),
            "http://127.0.0.1:8000/.well-known/underweb/a.toml"
        );
        assert_eq!(
            parse_address("file:///x/y").unwrap().address,
            Address::Local("/x/y".into())
        );
        assert!(parse_address("uw://example.org/../etc").is_err());
        assert!(parse_address("uw://exa mple/x").is_err());
        assert!(parse_address("https://example.org").is_err());
    }
}
