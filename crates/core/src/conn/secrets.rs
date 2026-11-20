//! Password storage: OS keyring first, ChaCha20-Poly1305 encrypted file fallback.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::RngCore;

use crate::error::{MyDbError, Result};

const KEYRING_SERVICE: &str = "mydb-manager";

/// Encrypted-file secret store living next to `connections.toml`.
#[derive(Debug)]
pub struct Store {
    key_path: PathBuf,
    data_path: PathBuf,
    key: [u8; 32],
    map: BTreeMap<String, String>,
}

impl Store {
    /// Open (or create) the encrypted store in `dir`.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        fs::create_dir_all(dir)?;
        let key_path = dir.join(".secrets-key");
        let data_path = dir.join("secrets.enc");
        let key = load_or_create_key(&key_path)?;
        let map = if data_path.exists() {
            decrypt_map(&key, &fs::read(&data_path)?)?
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            key_path,
            data_path,
            key,
            map,
        })
    }

    /// Persist `password` for `profile`, preferring the OS keyring.
    pub fn set(&mut self, profile: &str, password: &str) -> Result<()> {
        if try_keyring_set(profile, password) {
            self.map.remove(profile);
            self.persist()?;
            return Ok(());
        }
        self.map.insert(profile.to_string(), password.to_string());
        self.persist()
    }

    /// Look up a password: keyring, then encrypted file.
    pub fn get(&self, profile: &str) -> Result<Option<String>> {
        if let Some(pw) = try_keyring_get(profile) {
            return Ok(Some(pw));
        }
        Ok(self.map.get(profile).cloned())
    }

    /// Drop a stored password from both backends.
    pub fn delete(&mut self, profile: &str) -> Result<()> {
        let _ = try_keyring_delete(profile);
        self.map.remove(profile);
        self.persist()
    }

    fn persist(&self) -> Result<()> {
        if self.map.is_empty() {
            if self.data_path.exists() {
                fs::remove_file(&self.data_path)?;
            }
            return Ok(());
        }
        let blob = encrypt_map(&self.key, &self.map)?;
        fs::write(&self.data_path, blob)?;
        restrict_mode(&self.data_path);
        restrict_mode(&self.key_path);
        Ok(())
    }
}

fn try_keyring_set(profile: &str, password: &str) -> bool {
    keyring::Entry::new(KEYRING_SERVICE, profile)
        .and_then(|e| e.set_password(password))
        .is_ok()
}

fn try_keyring_get(profile: &str) -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, profile)
        .ok()?
        .get_password()
        .ok()
}

fn try_keyring_delete(profile: &str) -> bool {
    keyring::Entry::new(KEYRING_SERVICE, profile)
        .and_then(|e| e.delete_credential())
        .is_ok()
}

fn load_or_create_key(path: &Path) -> Result<[u8; 32]> {
    if path.exists() {
        let bytes = fs::read(path)?;
        if bytes.len() != 32 {
            return Err(MyDbError::Config(format!(
                "secrets key {} is not 32 bytes",
                path.display()
            )));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(key);
    }
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    fs::write(path, key)?;
    restrict_mode(path);
    Ok(key)
}

fn encrypt_map(key: &[u8; 32], map: &BTreeMap<String, String>) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(map)
        .map_err(|e| MyDbError::Config(format!("cannot encode secrets: {e}")))?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let mut out = nonce_bytes.to_vec();
    let ct = cipher
        .encrypt(nonce, json.as_ref())
        .map_err(|e| MyDbError::Config(format!("cannot encrypt secrets: {e}")))?;
    out.extend(ct);
    Ok(out)
}

fn decrypt_map(key: &[u8; 32], blob: &[u8]) -> Result<BTreeMap<String, String>> {
    if blob.len() < 13 {
        return Err(MyDbError::Config("secrets file is truncated".into()));
    }
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = Nonce::from_slice(&blob[..12]);
    let pt = cipher
        .decrypt(nonce, &blob[12..])
        .map_err(|e| MyDbError::Config(format!("cannot decrypt secrets: {e}")))?;
    serde_json::from_slice(&pt).map_err(|e| MyDbError::Config(format!("cannot parse secrets: {e}")))
}

fn restrict_mode(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        let id = N.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("mydb-secrets-{}-{}", std::process::id(), id));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn file_roundtrip() {
        let dir = temp_dir();
        let mut store = Store::open(&dir).unwrap();
        store.set("prod", "s3cret").unwrap();
        drop(store);
        let store = Store::open(&dir).unwrap();
        // keyring may also have stored it; either backend is fine
        assert_eq!(store.get("prod").unwrap().as_deref(), Some("s3cret"));
        fs::remove_dir_all(dir).ok();
    }
}
