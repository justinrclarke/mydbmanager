//! Connection profiles, shared between the CLI and the desktop app through a
//! single TOML file in the platform config directory.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use directories::BaseDirs;
use serde::{Deserialize, Serialize};

use crate::error::{MyDbError, Result};
use crate::provider::ProviderKind;

/// Environment label for a connection, driving UI badges and safety checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentTag {
    /// Local/development database (🟢).
    #[default]
    Development,
    /// Staging database (🟡).
    Staging,
    /// Production database (🔴) — destructive operations require confirmation.
    Production,
    /// Test/QA database (🔵).
    Testing,
    /// Anything else (⚪).
    Other,
}

impl EnvironmentTag {
    /// Canonical lowercase label.
    pub fn label(self) -> &'static str {
        match self {
            EnvironmentTag::Development => "development",
            EnvironmentTag::Staging => "staging",
            EnvironmentTag::Production => "production",
            EnvironmentTag::Testing => "testing",
            EnvironmentTag::Other => "other",
        }
    }

    /// True when destructive operations need explicit confirmation.
    pub fn is_production(self) -> bool {
        matches!(self, EnvironmentTag::Production)
    }
}

impl fmt::Display for EnvironmentTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for EnvironmentTag {
    type Err = MyDbError;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "prod" | "production" => Ok(EnvironmentTag::Production),
            "staging" | "stage" => Ok(EnvironmentTag::Staging),
            "dev" | "development" | "local" => Ok(EnvironmentTag::Development),
            "test" | "testing" | "qa" => Ok(EnvironmentTag::Testing),
            "other" => Ok(EnvironmentTag::Other),
            other => Err(MyDbError::InvalidValue(format!(
                "unknown environment '{other}' (expected prod, staging, dev, test or other)"
            ))),
        }
    }
}

/// SSH tunnel configuration for reaching databases behind a bastion host.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SshConfig {
    /// Bastion hostname.
    pub host: String,
    /// SSH port (default 22).
    pub port: u16,
    /// SSH user.
    pub user: String,
    /// Password auth (mutually exclusive with key auth in practice).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Path to a private key (OpenSSH format).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key_path: Option<PathBuf>,
    /// Passphrase for the private key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
}

fn default_ssh_port() -> u16 {
    22
}

/// A saved connection profile. Either `url` or the discrete host/port/user/
/// database fields must be set; `url` wins when both are present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ConnectionProfile {
    /// Unique profile name, used on the command line and in the UI.
    pub name: String,
    /// Environment label.
    pub env: EnvironmentTag,
    /// Database engine.
    pub kind: ProviderKind,
    /// Full connection URL (e.g. `postgres://user:pass@host:5432/db`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Hostname (when not using `url`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// TCP port (defaults to the engine's conventional port).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// User name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Password. TODO(Phase 1): move to the OS keyring via `keyring`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Database name, or file path for SQLite.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    /// Optional SSH tunnel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh: Option<SshConfig>,
    /// Override path for `pg_dump` (dump engine tool detection).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pg_dump_path: Option<PathBuf>,
    /// Override path for `pg_restore`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pg_restore_path: Option<PathBuf>,
    /// Override path for `mysqldump`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mysqldump_path: Option<PathBuf>,
}

impl Default for ConnectionProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            env: EnvironmentTag::default(),
            kind: ProviderKind::Postgres,
            url: None,
            host: None,
            port: None,
            user: None,
            password: None,
            database: None,
            ssh: None,
            pg_dump_path: None,
            pg_restore_path: None,
            mysqldump_path: None,
        }
    }
}

impl ConnectionProfile {
    /// Validate the profile has enough information to connect.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(MyDbError::InvalidValue(
                "connection name must not be empty".into(),
            ));
        }
        if self.url.is_none() && self.host.is_none() && self.database.is_none() {
            return Err(MyDbError::InvalidValue(format!(
                "connection '{}' needs either a --url or --host/--database",
                self.name
            )));
        }
        if let Some(ssh) = &self.ssh {
            if ssh.host.is_empty() || ssh.user.is_empty() {
                return Err(MyDbError::InvalidValue(format!(
                    "connection '{}' has an incomplete ssh section (host and user required)",
                    self.name
                )));
            }
        }
        Ok(())
    }

    /// Human-readable target with any password masked.
    pub fn masked_target(&self) -> String {
        if let Some(url) = &self.url {
            mask_url_password(url)
        } else if let Some(host) = &self.host {
            let port = self.port.unwrap_or_else(|| self.kind.default_port());
            match (&self.user, &self.database) {
                (Some(user), Some(db)) => format!("{user}@{host}:{port}/{db}"),
                (Some(user), None) => format!("{user}@{host}:{port}"),
                (None, Some(db)) => format!("{host}:{port}/{db}"),
                (None, None) => format!("{host}:{port}"),
            }
        } else {
            self.database.clone().unwrap_or_default()
        }
    }

    /// Effective SSH port.
    pub fn ssh_port(ssh: &SshConfig) -> u16 {
        if ssh.port == 0 {
            default_ssh_port()
        } else {
            ssh.port
        }
    }
}

/// Replace the password component of a URL with `***`.
pub fn mask_url_password(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let (scheme, rest) = url.split_at(scheme_end + 3);
    let Some(at) = rest.rfind('@') else {
        return url.to_string();
    };
    let credentials = &rest[..at];
    let Some(colon) = credentials.find(':') else {
        return url.to_string();
    };
    format!("{scheme}{}:***@{}", &credentials[..colon], &rest[at + 1..])
}

/// On-disk layout of `connections.toml`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    connections: Vec<ConnectionProfile>,
}

/// Reads and writes the shared connection profile store.
#[derive(Debug)]
pub struct ConfigStore {
    path: PathBuf,
    file: ConfigFile,
}

/// Default config path: `$MYDB_CONFIG_DIR/connections.toml` when set,
/// otherwise the platform config directory (`~/.config/mydb`,
/// `~/Library/Application Support/mydb`, `%APPDATA%\mydb`).
pub fn default_config_path() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("MYDB_CONFIG_DIR") {
        return Ok(PathBuf::from(dir).join("connections.toml"));
    }
    let base = BaseDirs::new()
        .ok_or_else(|| MyDbError::Config("cannot determine home directory".into()))?;
    Ok(base.config_dir().join("mydb").join("connections.toml"))
}

impl ConfigStore {
    /// Open a config file at an explicit path (used by tests and `--config`).
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let file = if path.exists() {
            let text = fs::read_to_string(&path)?;
            toml::from_str(&text)
                .map_err(|e| MyDbError::Config(format!("cannot parse {}: {e}", path.display())))?
        } else {
            ConfigFile::default()
        };
        Ok(Self { path, file })
    }

    /// Open the shared config at the platform default location.
    pub fn load_default() -> Result<Self> {
        Self::open(default_config_path()?)
    }

    /// Path of the underlying file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// All saved connection profiles.
    pub fn connections(&self) -> &[ConnectionProfile] {
        &self.file.connections
    }

    /// Look up a profile by exact name.
    pub fn get(&self, name: &str) -> Result<&ConnectionProfile> {
        self.file
            .connections
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| MyDbError::ConnectionNotFound(name.to_string()))
    }

    /// Add a new profile; fails when the name is taken.
    pub fn add(&mut self, profile: ConnectionProfile) -> Result<()> {
        profile.validate()?;
        if self.file.connections.iter().any(|c| c.name == profile.name) {
            return Err(MyDbError::ConnectionExists(profile.name));
        }
        self.file.connections.push(profile);
        Ok(())
    }

    /// Remove a profile by name.
    pub fn remove(&mut self, name: &str) -> Result<()> {
        let before = self.file.connections.len();
        self.file.connections.retain(|c| c.name != name);
        if self.file.connections.len() == before {
            return Err(MyDbError::ConnectionNotFound(name.to_string()));
        }
        Ok(())
    }

    /// Persist changes, creating the parent directory when needed.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(&self.file)
            .map_err(|e| MyDbError::Config(format!("cannot serialize config: {e}")))?;
        fs::write(&self.path, text)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_config_path() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir()
            .join(format!("mydb-core-test-{}-{}", std::process::id(), id))
            .join("connections.toml")
    }

    fn sample_profile(name: &str) -> ConnectionProfile {
        ConnectionProfile {
            name: name.into(),
            env: EnvironmentTag::Production,
            kind: ProviderKind::Postgres,
            url: Some("postgres://app:s3cret@db.example.com:5432/app".into()),
            ..Default::default()
        }
    }

    #[test]
    fn roundtrip_profiles() {
        let path = temp_config_path();
        let mut store = ConfigStore::open(&path).unwrap();
        store.add(sample_profile("prod")).unwrap();
        store.save().unwrap();

        let reloaded = ConfigStore::open(&path).unwrap();
        let prod = reloaded.get("prod").unwrap();
        assert_eq!(prod.kind, ProviderKind::Postgres);
        assert_eq!(prod.env, EnvironmentTag::Production);
        assert_eq!(
            prod.url.as_deref(),
            Some("postgres://app:s3cret@db.example.com:5432/app")
        );

        fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn duplicate_and_missing_names_error() {
        let path = temp_config_path();
        let mut store = ConfigStore::open(&path).unwrap();
        store.add(sample_profile("prod")).unwrap();
        assert!(matches!(
            store.add(sample_profile("prod")),
            Err(MyDbError::ConnectionExists(_))
        ));
        assert!(matches!(
            store.get("dev"),
            Err(MyDbError::ConnectionNotFound(_))
        ));
        store.remove("prod").unwrap();
        assert!(store.connections().is_empty());
        fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn validation_requires_target() {
        let profile = ConnectionProfile {
            name: "broken".into(),
            kind: ProviderKind::Mysql,
            ..Default::default()
        };
        assert!(matches!(
            profile.validate(),
            Err(MyDbError::InvalidValue(_))
        ));
    }

    #[test]
    fn passwords_are_masked() {
        assert_eq!(
            mask_url_password("postgres://app:s3cret@db.example.com:5432/app"),
            "postgres://app:***@db.example.com:5432/app"
        );
        assert_eq!(
            mask_url_password("postgres://db.example.com/app"),
            "postgres://db.example.com/app"
        );
        let profile = sample_profile("prod");
        assert!(profile.masked_target().contains("***"));
        assert!(!profile.masked_target().contains("s3cret"));
    }

    #[test]
    fn environment_tags_parse() {
        assert_eq!(
            "prod".parse::<EnvironmentTag>().unwrap(),
            EnvironmentTag::Production
        );
        assert_eq!(
            "DEV".parse::<EnvironmentTag>().unwrap(),
            EnvironmentTag::Development
        );
        assert_eq!(
            "staging".parse::<EnvironmentTag>().unwrap(),
            EnvironmentTag::Staging
        );
        assert!("prod".parse::<EnvironmentTag>().unwrap().is_production());
        assert!("banana".parse::<EnvironmentTag>().is_err());
    }
}
