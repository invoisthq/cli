//! Profiles and where their credentials live.
//!
//! `config.toml` holds each profile's settings: which kind of account it
//! reaches, test or live, how it signed in, and the API address. Secrets (an
//! API key, or a sign-in's tokens) go in the system keychain. Where there is
//! no keychain (a server, a container, CI), they are kept in the config file
//! instead, which is then readable by its owner only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CliError, Result};

pub const DEFAULT_PROFILE: &str = "default";
const KEYCHAIN_SERVICE: &str = "invoist-cli";

/// Which side of Invoist a profile reaches. Decides which commands and
/// screens make sense.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Business,
    Household,
}

impl AccountKind {
    pub fn label(self) -> &'static str {
        match self {
            AccountKind::Business => "business",
            AccountKind::Household => "household",
        }
    }
}

/// Test keys read a fixed sample result and are never charged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Live,
    Test,
}

impl Mode {
    /// Test keys start `inv_sk_test_`; everything else is live.
    pub fn of_key(key: &str) -> Mode {
        if key.starts_with("inv_sk_test_") {
            Mode::Test
        } else {
            Mode::Live
        }
    }
}

/// A profile's secret: an API key, or the tokens from signing in.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Credential {
    Key {
        key: String,
    },
    Browser {
        access_token: String,
        refresh_token: String,
        /// Unix seconds when the access token stops working.
        expires_at: u64,
        client_id: String,
        token_endpoint: String,
        #[serde(default)]
        scope: String,
    },
}

impl std::fmt::Debug for Credential {
    // Never print a secret, not even in a debug log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credential::Key { key } => write!(f, "Key({})", redact(key)),
            Credential::Browser { expires_at, .. } => {
                write!(f, "Browser(expires_at: {expires_at})")
            }
        }
    }
}

/// The last four characters, which is all the CLI ever shows of a key.
pub fn redact(secret: &str) -> String {
    let tail: String = secret
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<AccountKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    /// The API's address, when it isn't the default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// How the profile signed in: `browser` or `key`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_in_with: Option<String>,
    /// The last four characters of the key, to recognise it by.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// The CLI's OAuth client, registered once per profile and reused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Only when there's no keychain: the credential itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<Credential>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

/// Where the config file lives: `INVOIST_CONFIG_DIR` when set, otherwise
/// the platform's config folder (`~/.config/invoist` on Linux).
pub fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("INVOIST_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    directories::ProjectDirs::from("st", "Invoist", "invoist")
        .map(|d| d.config_dir().to_path_buf())
        .ok_or_else(|| CliError::Failed("couldn't find a home folder to keep settings in".into()))
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        Ok(config_dir()?.join("config.toml"))
    }

    pub fn load() -> Result<Config> {
        let path = Config::path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| {
                CliError::Failed(format!(
                    "{} isn't a valid settings file: {e}",
                    path.display()
                ))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(CliError::io(format!("couldn't read {}", path.display()), e)),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Config::path()?;
        let text = toml::to_string_pretty(self)
            .map_err(|e| CliError::Failed(format!("couldn't write settings: {e}")))?;
        write_private(&path, text.as_bytes())
    }

    /// The profile to use: the one named, else the default, else `default`.
    pub fn profile_name(&self, requested: Option<&str>) -> String {
        requested
            .map(str::to_string)
            .or_else(|| self.default_profile.clone())
            .unwrap_or_else(|| DEFAULT_PROFILE.to_string())
    }
}

/// Writes a file only its owner can read, replacing it in one step so a
/// crash never leaves half a settings file.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)
        .map_err(|e| CliError::io(format!("couldn't create {}", dir.display()), e))?;
    let tmp = path.with_extension("toml.tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|e| CliError::io(format!("couldn't write {}", tmp.display()), e))?;
        std::io::Write::write_all(&mut file, bytes)
            .map_err(|e| CliError::io(format!("couldn't write {}", tmp.display()), e))?;
    }
    std::fs::rename(&tmp, path)
        .map_err(|e| CliError::io(format!("couldn't write {}", path.display()), e))
}

/// Where a profile's secret ended up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    Keychain,
    ConfigFile,
}

fn keychain_disabled() -> bool {
    std::env::var_os("INVOIST_NO_KEYCHAIN").is_some_and(|v| !v.is_empty() && v != "0")
}

fn keychain_entry(profile: &str) -> Option<keyring::Entry> {
    if keychain_disabled() {
        return None;
    }
    keyring::Entry::new(KEYCHAIN_SERVICE, profile).ok()
}

/// Saves a profile's secret: in the keychain when there is one, otherwise in
/// the (owner-only) config file. Returns where it went.
pub fn store_credential(
    config: &mut Config,
    profile: &str,
    credential: &Credential,
) -> Result<Storage> {
    let json = serde_json::to_string(credential).map_err(|e| CliError::Failed(e.to_string()))?;
    let entry = config.profiles.entry(profile.to_string()).or_default();
    if let Some(keychain) = keychain_entry(profile)
        && keychain.set_password(&json).is_ok()
    {
        entry.credential = None;
        return Ok(Storage::Keychain);
    }
    entry.credential = Some(credential.clone());
    Ok(Storage::ConfigFile)
}

/// Reads a profile's secret from wherever it was saved.
pub fn load_credential(config: &Config, profile: &str) -> Option<Credential> {
    if let Some(c) = config
        .profiles
        .get(profile)
        .and_then(|p| p.credential.clone())
    {
        return Some(c);
    }
    let json = keychain_entry(profile)?.get_password().ok()?;
    serde_json::from_str(&json).ok()
}

/// Removes a profile's secret from both places.
pub fn delete_credential(config: &mut Config, profile: &str) {
    if let Some(keychain) = keychain_entry(profile) {
        let _ = keychain.delete_credential();
    }
    if let Some(p) = config.profiles.get_mut(profile) {
        p.credential = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keys_are_recognised_by_their_prefix() {
        assert_eq!(Mode::of_key("inv_sk_test_abcd"), Mode::Test);
        assert_eq!(Mode::of_key("inv_sk_abcd"), Mode::Live);
        assert_eq!(Mode::of_key("inv_at_abcd"), Mode::Live);
    }

    #[test]
    fn secrets_never_appear_in_debug_output() {
        let key = Credential::Key {
            key: "inv_sk_test_0000000000000000wxyz".into(),
        };
        assert_eq!(format!("{key:?}"), "Key(…wxyz)");
        let browser = Credential::Browser {
            access_token: "inv_at_secret".into(),
            refresh_token: "inv_rt_secret".into(),
            expires_at: 10,
            client_id: "c".into(),
            token_endpoint: "t".into(),
            scope: String::new(),
        };
        assert!(!format!("{browser:?}").contains("secret"));
    }

    #[test]
    fn config_round_trips_through_toml() {
        let mut config = Config::default();
        config.profiles.insert(
            "default".into(),
            Profile {
                kind: Some(AccountKind::Household),
                mode: Some(Mode::Live),
                signed_in_with: Some("browser".into()),
                ..Default::default()
            },
        );
        let text = toml::to_string_pretty(&config).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.profiles["default"].kind, Some(AccountKind::Household));
    }
}
