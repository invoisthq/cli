//! Turns the command line's flags, the environment and the saved profile into
//! a working SDK client.
//!
//! The key comes from, first match wins: `INVOIST_API_KEY`, then the profile
//! (the system keychain, or the config file where there's no keychain). A
//! browser sign-in is refreshed a minute before its hour runs out, and the
//! new tokens are saved, so a long session in the terminal app never stops.

use invoist::Client;

use crate::config::{self, AccountKind, Config, Credential, Mode};
use crate::error::{CliError, Result};
use crate::oauth;

pub struct Session {
    pub profile: String,
    pub base_url: String,
    credential: Credential,
    /// True when the key came from `INVOIST_API_KEY`: nothing is saved then.
    from_env: bool,
    pub kind: Option<AccountKind>,
    pub mode: Mode,
}

impl Session {
    /// The profile's base URL: the flag, then `INVOIST_BASE_URL`, then the
    /// profile's own, then the SDK's default.
    pub fn base_url_for(config: &Config, profile: &str, flag: Option<&str>) -> String {
        flag.map(str::to_string)
            .or_else(|| {
                std::env::var("INVOIST_BASE_URL")
                    .ok()
                    .filter(|s| !s.trim().is_empty())
            })
            .or_else(|| {
                config
                    .profiles
                    .get(profile)
                    .and_then(|p| p.base_url.clone())
            })
            .unwrap_or_else(|| invoist::DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }

    pub fn open(profile_flag: Option<&str>, base_url_flag: Option<&str>) -> Result<Session> {
        let config = Config::load()?;
        let profile = config.profile_name(profile_flag);
        let base_url = Session::base_url_for(&config, &profile, base_url_flag);

        if let Some(key) = std::env::var("INVOIST_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
        {
            let key = key.trim().to_string();
            return Ok(Session {
                profile,
                base_url,
                mode: Mode::of_key(&key),
                credential: Credential::Key { key },
                from_env: true,
                kind: None,
            });
        }

        let saved = config.profiles.get(&profile);
        let credential = config::load_credential(&config, &profile).ok_or_else(|| {
            CliError::NotSignedIn(if profile == config::DEFAULT_PROFILE {
                "You're not signed in. Run `invoist login`, or set INVOIST_API_KEY.".into()
            } else {
                format!("The profile `{profile}` isn't signed in. Run `invoist login --profile {profile}`.")
            })
        })?;
        Ok(Session {
            kind: saved.and_then(|p| p.kind),
            mode: saved.and_then(|p| p.mode).unwrap_or(Mode::Live),
            profile,
            base_url,
            credential,
            from_env: false,
        })
    }

    /// An SDK client with a working token, refreshing the sign-in first when
    /// it's about to expire.
    pub async fn client(&mut self) -> Result<Client> {
        if oauth::needs_refresh(&self.credential) {
            self.credential = oauth::refresh(&self.credential).await?;
            if !self.from_env {
                let mut config = Config::load()?;
                config::store_credential(&mut config, &self.profile, &self.credential)?;
                config.save()?;
            }
        }
        let token = match &self.credential {
            Credential::Key { key } => key.clone(),
            Credential::Browser { access_token, .. } => access_token.clone(),
        };
        Ok(Client::builder()
            .api_key(token)
            .base_url(&self.base_url)
            .build()?)
    }

    pub fn credential(&self) -> &Credential {
        &self.credential
    }

    pub fn is_from_env(&self) -> bool {
        self.from_env
    }

    /// The account's kind: saved at sign-in, or found out now by asking.
    pub async fn kind(&mut self) -> Result<AccountKind> {
        if let Some(kind) = self.kind {
            return Ok(kind);
        }
        let client = self.client().await?;
        let kind = detect_kind(&client).await?;
        self.kind = Some(kind);
        Ok(kind)
    }
}

/// Works out whether a key reaches a business or a household. There's no
/// "who am I" call, so it asks a business question first: a household key
/// is refused with 403, and then the household question must succeed.
pub async fn detect_kind(client: &Client) -> Result<AccountKind> {
    match client.get_usage().await {
        Ok(_) => Ok(AccountKind::Business),
        Err(e) if e.is_permission_error() => match client.bills().summary(Default::default()).await {
            Ok(_) => Ok(AccountKind::Household),
            Err(household) if household.is_permission_error() => Err(CliError::Failed(
                "This key works, but it can't read invoices or bills. Give it the documents:read or bills:read permission in Invoist.".into(),
            )),
            Err(household) => Err(household.into()),
        },
        Err(e) => Err(e.into()),
    }
}

/// The kind a browser sign-in granted, from its scopes: households get the
/// `bills:` scopes, businesses everything else.
pub fn kind_from_scopes(scope: &str) -> Option<AccountKind> {
    if scope.split_whitespace().any(|s| s.starts_with("bills:")) {
        Some(AccountKind::Household)
    } else if scope.split_whitespace().next().is_some() {
        Some(AccountKind::Business)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_tell_households_from_businesses() {
        assert_eq!(
            kind_from_scopes("bills:read bills:write"),
            Some(AccountKind::Household)
        );
        assert_eq!(
            kind_from_scopes("documents:read review:read"),
            Some(AccountKind::Business)
        );
        assert_eq!(kind_from_scopes(""), None);
    }
}
