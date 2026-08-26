//! Configuration and credential handling.
//!
//! v1 stores everything in `~/.config/inode-vpn/config.toml` (mode 0600).
//! `Config::migrate_legacy_auth()` converts the old `.auth` key=value file.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG_DIR: &str = ".config/inode-vpn";
pub const DEFAULT_CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub gateway: GatewayConfig,
    pub credentials: Credentials,
    pub network: NetworkConfig,
    pub service: ServiceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GatewayConfig {
    /// `host:port`, same semantics as the legacy `.auth` `gateway=` key.
    pub url: String,
    /// `pin-sha256:` SPKI pin (RFC 7469 style). Empty means TOFU discover.
    pub servercert: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    /// `auto` uses the gateway's KEEPALIVETIME; an integer overrides it.
    pub keepalive: Keepalive,
    /// Extra CIDRs that must stay reachable (added as more-specific routes).
    pub preserve_cidrs: Vec<String>,
    /// `server` applies VPN DNS, `ignore` keeps the system DNS.
    pub dns: DnsMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Keepalive {
    Seconds(u64),
    /// Serialized/deserialized as the string `"auto"`.
    Auto(String),
}

impl Default for Keepalive {
    fn default() -> Self {
        Self::Auto("auto".into())
    }
}

impl Keepalive {
    pub fn seconds(&self, advertised: u64) -> u64 {
        match self {
            Self::Auto(_) => advertised,
            Self::Seconds(s) => *s,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DnsMode {
    Server,
    Ignore,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    pub autostart: bool,
    pub restart_delay: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            gateway: GatewayConfig {
                url: String::new(),
                servercert: String::new(),
            },
            credentials: Credentials::default(),
            network: NetworkConfig {
                keepalive: Keepalive::Auto("auto".into()),
                preserve_cidrs: Vec::new(),
                dns: DnsMode::Server,
            },
            service: ServiceConfig {
                autostart: true,
                restart_delay: 10,
            },
        }
    }
}

impl Config {
    pub fn default_path() -> Result<PathBuf> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| Error::Config("HOME is not set".into()))?;
        Ok(home.join(DEFAULT_CONFIG_DIR).join(DEFAULT_CONFIG_FILE))
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(Error::Config(format!(
                "configuration file {} does not exist; run `inode config migrate` or create it (mode 0600)",
                path.display()
            )));
        }
        Self::enforce_mode(path)?;
        let raw = fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("failed to read {}: {e}", path.display())))?;
        let cfg: Config = toml::from_str(&raw)
            .map_err(|e| Error::Config(format!("failed to parse {}: {e}", path.display())))?;
        if cfg.gateway.url.trim().is_empty() {
            return Err(Error::Config(format!(
                "{}: gateway.url must not be empty",
                path.display()
            )));
        }
        Ok(cfg)
    }

    /// Config and legacy `.auth` files contain the password; refuse to load
    /// anything group/world readable.
    pub fn enforce_mode(path: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::metadata(path)
                .map_err(|e| Error::Config(format!("failed to stat {}: {e}", path.display())))?;
            if meta.permissions().mode() & 0o077 != 0 {
                return Err(Error::Config(format!(
                    "{} has insecure permissions; run: chmod 600 {}",
                    path.display(),
                    path.display()
                )));
            }
        }
        Ok(())
    }

    fn save_private(path: &Path, contents: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                Error::Config(format!("failed to create {}: {e}", parent.display()))
            })?;
        }
        let mut file = fs::File::create(path)
            .map_err(|e| Error::Config(format!("failed to create {}: {e}", path.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|e| {
                    Error::Config(format!("failed to chmod 600 {}: {e}", path.display()))
                })?;
        }
        file.write_all(contents.as_bytes())
            .map_err(|e| Error::Config(format!("failed to write {}: {e}", path.display())))?;
        Ok(())
    }

    /// Serialize to `path` with mode 0600.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("failed to serialize config: {e}")))?;
        Self::save_private(path, &text)
    }

    /// Update a whitelisted scalar key. Keeps the file interface narrow and
    /// avoids accepting arbitrary TOML writes from the CLI.
    pub fn with_key_set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "gateway.url" => self.gateway.url = value.to_string(),
            "servercert" => self.gateway.servercert = value.to_string(),
            "username" => self.credentials.username = value.to_string(),
            "password" => self.credentials.password = value.to_string(),
            "keepalive" => {
                self.network.keepalive = if value == "auto" {
                    Keepalive::Auto("auto".into())
                } else {
                    let secs = value.parse::<u64>().map_err(|_| {
                        Error::Config(format!("keepalive must be 'auto' or seconds: {value}"))
                    })?;
                    Keepalive::Seconds(secs)
                }
            }
            "dns" => {
                self.network.dns = match value {
                    "server" => DnsMode::Server,
                    "ignore" => DnsMode::Ignore,
                    other => {
                        return Err(Error::Config(format!("dns must be server|ignore: {other}")))
                    }
                }
            }
            "preserve_cidrs" => {
                self.network.preserve_cidrs = value
                    .split([',', ';'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            }
            other => return Err(Error::Config(format!("unsupported config key: {other}"))),
        }
        Ok(())
    }

    /// Migrate a legacy `.auth` key=value file to the TOML config.
    ///
    /// `ping_target` is intentionally ignored: liveness no longer uses ping.
    pub fn migrate_legacy_auth(auth_path: &Path, dest: &Path) -> Result<Self> {
        Self::enforce_mode(auth_path)?;
        let raw = fs::read_to_string(auth_path)
            .map_err(|e| Error::Config(format!("failed to read {}: {e}", auth_path.display())))?;
        let mut values = std::collections::HashMap::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = line.split_once('=').ok_or_else(|| {
                Error::Config(format!("invalid line in {}: {line:?}", auth_path.display()))
            })?;
            values.insert(k.trim().to_string(), v.trim().to_string());
        }

        let cfg = Self {
            gateway: GatewayConfig {
                url: values.get("gateway").cloned().unwrap_or_default(),
                servercert: values.get("servercert").cloned().unwrap_or_default(),
            },
            credentials: Credentials {
                username: values.get("username").cloned().unwrap_or_default(),
                password: values.get("password").cloned().unwrap_or_default(),
            },
            ..Self::default()
        };
        if cfg.gateway.url.is_empty() || cfg.credentials.username.is_empty() {
            return Err(Error::Config(format!(
                "{} is missing gateway/username",
                auth_path.display()
            )));
        }
        let toml_text = toml::to_string_pretty(&cfg)
            .map_err(|e| Error::Config(format!("failed to serialize config: {e}")))?;
        Self::save_private(dest, &toml_text)?;
        Ok(cfg)
    }

    /// Masked view used by `inode status/config show` and diagnostics.
    pub fn redacted(&self) -> ConfigRedacted {
        ConfigRedacted {
            gateway_url: self.gateway.url.clone(),
            servercert_present: !self.gateway.servercert.is_empty(),
            username: self.credentials.username.clone(),
            keepalive: self.network.keepalive.clone(),
            preserve_cidrs: self.network.preserve_cidrs.clone(),
            dns: self.network.dns,
            autostart: self.service.autostart,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigRedacted {
    pub gateway_url: String,
    pub servercert_present: bool,
    pub username: String,
    pub keepalive: Keepalive,
    pub preserve_cidrs: Vec<String>,
    pub dns: DnsMode,
    pub autostart: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("inode-test-{name}-{}", std::process::id()));
        let _ = fs::remove_file(&p);
        p
    }

    #[test]
    fn migrate_legacy_auth_round_trip() {
        let auth = tmp("migrate.auth");
        let dest = tmp("migrate.toml");
        fs::write(
            &auth,
            "username=alice\ngateway=vpn.example.com:2000\npassword=s3cret\nservercert=pin-sha256:abc\nping_target=10.0.0.1\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&auth, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let cfg = Config::migrate_legacy_auth(&auth, &dest).unwrap();
        assert_eq!(cfg.gateway.url, "vpn.example.com:2000");
        assert_eq!(cfg.credentials.password, "s3cret");
        let loaded = Config::load(&dest).unwrap();
        assert_eq!(loaded, cfg);
        let _ = fs::remove_file(auth);
        let _ = fs::remove_file(dest);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_world_readable_config() {
        use std::os::unix::fs::PermissionsExt;
        let path = tmp("badmode.toml");
        fs::write(&path, "gateway_url = \"x\"").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Config::load(&path).is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn with_key_set_whitelists_keys() {
        let mut cfg = Config::default();
        cfg.with_key_set("gateway.url", "vpn.example.com:2000")
            .unwrap();
        cfg.with_key_set("username", "alice").unwrap();
        cfg.with_key_set("password", "s3cret").unwrap();
        cfg.with_key_set("keepalive", "45").unwrap();
        cfg.with_key_set("dns", "ignore").unwrap();
        cfg.with_key_set("preserve_cidrs", "192.168.0.0/23,10.0.0.0/8")
            .unwrap();
        assert_eq!(cfg.gateway.url, "vpn.example.com:2000");
        assert_eq!(cfg.credentials.password, "s3cret");
        assert_eq!(cfg.network.keepalive, Keepalive::Seconds(45));
        assert_eq!(cfg.network.dns, DnsMode::Ignore);
        assert_eq!(
            cfg.network.preserve_cidrs,
            vec!["192.168.0.0/23", "10.0.0.0/8"]
        );
        assert!(cfg.with_key_set("nonsense", "x").is_err());
    }
}
