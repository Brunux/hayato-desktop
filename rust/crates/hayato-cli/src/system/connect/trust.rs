//! Persistent trusted-device store (`~/.config/hayato/connect/devices.json`)
//! with mode `0600` atomic writes and per-device capability controls.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::crypto::{mint_device_secret, sha256_hex};
use crate::system::share::server::{constant_time_eq, generate_token};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    pub files: bool,
    pub clipboard: bool,
    pub links: bool,
    pub media: bool,
    pub camera: bool,
    pub notifications: bool,
    pub folder_sync: bool,
}

impl Default for DeviceCapabilities {
    fn default() -> Self {
        Self {
            files: true,
            clipboard: true,
            links: true,
            media: true,
            camera: true,
            notifications: true,
            folder_sync: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedDevice {
    pub id: String,
    pub name: String,
    pub user_agent: String,
    pub secret_hash: String,
    pub paired_at: u64,
    pub last_seen: u64,
    #[serde(default)]
    pub capabilities: DeviceCapabilities,
}

#[derive(Default, Serialize, Deserialize)]
struct PersistedStore {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    devices: Vec<TrustedDevice>,
}

fn default_version() -> u32 {
    1
}

pub struct TrustStore {
    path: PathBuf,
    devices: Vec<TrustedDevice>,
}

impl TrustStore {
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                PathBuf::from(home).join(".config")
            })
            .join("hayato/connect/devices.json")
    }

    pub fn load_default() -> Self {
        Self::load_from(Self::default_path())
    }

    pub fn load_from(path: PathBuf) -> Self {
        let devices = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<PersistedStore>(&s).ok())
            .map(|p| p.devices)
            .unwrap_or_default();
        Self { path, devices }
    }

    pub fn devices(&self) -> &[TrustedDevice] {
        &self.devices
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create trust dir: {e}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
            }
        }
        let payload = PersistedStore {
            version: 1,
            devices: self.devices.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&payload)
            .map_err(|e| format!("serialize trust store: {e}"))?;
        let tmp = self
            .path
            .with_extension(format!("json.tmp.{}", std::process::id()));
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(|e| format!("open temp trust store: {e}"))?;
            file.write_all(&bytes)
                .map_err(|e| format!("write temp trust store: {e}"))?;
            let _ = file.sync_all();
        }
        fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            format!("commit trust store: {e}")
        })?;
        Ok(())
    }

    /// Create and persist a newly paired trusted device, returning `(device, raw_secret)`.
    pub fn pair_device(
        &mut self,
        name: &str,
        user_agent: &str,
    ) -> Result<(TrustedDevice, String), String> {
        let now = now_epoch();
        let id = format!("dev-{}", &generate_token()[..16]);
        let secret = mint_device_secret();
        let dev = TrustedDevice {
            id,
            name: name.trim().to_string(),
            user_agent: user_agent.trim().to_string(),
            secret_hash: sha256_hex(secret.as_bytes()),
            paired_at: now,
            last_seen: now,
            capabilities: DeviceCapabilities::default(),
        };
        self.devices.push(dev.clone());
        self.save()?;
        Ok((dev, secret))
    }

    pub fn verify(&self, id: &str, raw_secret: &str) -> Option<&TrustedDevice> {
        let got_hash = sha256_hex(raw_secret.as_bytes());
        self.devices
            .iter()
            .find(|d| d.id == id && constant_time_eq(d.secret_hash.as_bytes(), got_hash.as_bytes()))
    }

    pub fn touch(&mut self, id: &str) {
        if let Some(d) = self.devices.iter_mut().find(|d| d.id == id) {
            d.last_seen = now_epoch();
            let _ = self.save();
        }
    }

    pub fn unpair(&mut self, id: &str) -> Result<bool, String> {
        let before = self.devices.len();
        self.devices.retain(|d| d.id != id);
        if self.devices.len() != before {
            self.save()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn trust_store_pairs_verifies_persists_0600_and_unpairs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let mut store = TrustStore::load_from(path.clone());
        let (dev, secret) = store.pair_device("Pixel 8", "Android 15 · Chrome").unwrap();
        assert_eq!(store.devices().len(), 1);
        assert!(store.verify(&dev.id, &secret).is_some());
        assert!(store.verify(&dev.id, "wrong-secret").is_none());

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let mut reloaded = TrustStore::load_from(path);
        assert!(reloaded.verify(&dev.id, &secret).is_some());
        assert!(reloaded.unpair(&dev.id).unwrap());
        assert!(reloaded.devices().is_empty());
    }
}
