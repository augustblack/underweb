use std::path::Path;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{SigningKey, VerifyingKey};

/// An Ed25519 key pair for signing statements.
pub struct KeyPair {
    pub(crate) signing: SigningKey,
}

impl KeyPair {
    pub fn generate() -> KeyPair {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).expect("system randomness");
        KeyPair {
            signing: SigningKey::from_bytes(&seed),
        }
    }

    pub fn public_b64(&self) -> String {
        B64.encode(self.signing.verifying_key().to_bytes())
    }

    /// The bare-key identity, `key:<public key>`.
    pub fn identity(&self) -> String {
        format!("key:{}", self.public_b64())
    }

    /// Load, or create (readable only by the user) if missing.
    pub fn load_or_create(path: &Path) -> Result<KeyPair> {
        if path.exists() {
            return KeyPair::load(path);
        }
        let k = KeyPair::generate();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_private(path, &B64.encode(k.signing.to_bytes()))?;
        Ok(k)
    }

    pub fn load(path: &Path) -> Result<KeyPair> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let bytes = B64.decode(text.trim()).context("key file is not base64")?;
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("key file has the wrong length"))?;
        Ok(KeyPair {
            signing: SigningKey::from_bytes(&seed),
        })
    }
}

#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(text.as_bytes())?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> Result<()> {
    Ok(std::fs::write(path, text)?)
}

/// A base64 Ed25519 public key.
pub fn decode_key(b64: &str) -> Result<VerifyingKey> {
    let bytes = B64
        .decode(b64)
        .map_err(|_| anyhow::anyhow!("public key is not base64"))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("public key has the wrong length"))?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| anyhow::anyhow!("not an Ed25519 public key"))
}

/// Reject anything that isn't a valid key early.
pub fn check_key(b64: &str) -> Result<()> {
    if decode_key(b64).is_err() {
        bail!("{b64:?} is not an Ed25519 public key");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("k");
        let a = KeyPair::load_or_create(&p).unwrap();
        let b = KeyPair::load_or_create(&p).unwrap();
        assert_eq!(a.public_b64(), b.public_b64());
        check_key(&a.public_b64()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
