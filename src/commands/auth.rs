//! `login`, `logout`, `status` and `profile`.

use std::io::{IsTerminal, Read};

use serde::Serialize;

use crate::cli::{LoginArgs, ProfileCmd};
use crate::config::{self, AccountKind, Config, Credential, Mode, Storage};
use crate::error::{CliError, Result};
use crate::oauth;
use crate::output::{ACCENT, BOLD, DIM, GOOD, Out, WARN, styled};
use crate::session::{self, Session};

pub async fn login(
    profile_flag: Option<&str>,
    base_url_flag: Option<&str>,
    args: LoginArgs,
    out: Out,
) -> Result<()> {
    let mut config = Config::load()?;
    let profile = config.profile_name(profile_flag);
    let base_url = Session::base_url_for(&config, &profile, base_url_flag);

    let (credential, kind, mode, how) = if args.with_key {
        let key = read_key()?;
        let client = invoist::Client::builder()
            .api_key(&key)
            .base_url(&base_url)
            .build()?;
        out.note(styled(DIM, "Checking the key…"));
        let kind = session::detect_kind(&client).await.map_err(|e| match e {
            CliError::Api(api) if api.is_auth_error() => {
                CliError::NotSignedIn("Invoist didn't accept that key. Check you copied all of it, and that it hasn't been revoked.".into())
            }
            other => other,
        })?;
        (
            Credential::Key { key: key.clone() },
            kind,
            Mode::of_key(&key),
            "key",
        )
    } else {
        let meta = oauth::discover(&base_url).await?;
        // One registration per profile, reused for every later sign-in.
        let client_id = match config
            .profiles
            .get(&profile)
            .and_then(|p| p.client_id.clone())
        {
            Some(id) => id,
            None => oauth::register(&meta).await?,
        };
        let pending = oauth::begin(&meta, &client_id).await?;
        let opened = !args.no_browser && oauth::open_browser(&pending.authorize_url);
        if opened {
            out.note(format!(
                "{} Approve the CLI in your browser to sign in.",
                styled(ACCENT, "→")
            ));
            out.note(styled(
                DIM,
                format!("If no browser opened, go to:\n  {}", pending.authorize_url),
            ));
        } else {
            out.note(format!(
                "{} Open this link to sign in, on this computer:",
                styled(ACCENT, "→")
            ));
            out.note(format!("  {}", pending.authorize_url));
        }
        out.note(styled(
            DIM,
            "Waiting for you to approve… (Ctrl-C to cancel)",
        ));
        let credential = oauth::finish(&meta, &client_id, pending).await?;
        let scope = match &credential {
            Credential::Browser { scope, .. } => scope.clone(),
            Credential::Key { .. } => String::new(),
        };
        let kind = match session::kind_from_scopes(&scope) {
            Some(kind) => kind,
            None => {
                let token = match &credential {
                    Credential::Browser { access_token, .. } => access_token.clone(),
                    Credential::Key { key } => key.clone(),
                };
                let client = invoist::Client::builder()
                    .api_key(token)
                    .base_url(&base_url)
                    .build()?;
                session::detect_kind(&client).await?
            }
        };
        let entry = config.profiles.entry(profile.clone()).or_default();
        entry.client_id = Some(client_id);
        (credential, kind, Mode::Live, "browser")
    };

    let storage = config::store_credential(&mut config, &profile, &credential)?;
    let entry = config.profiles.entry(profile.clone()).or_default();
    entry.kind = Some(kind);
    entry.mode = Some(mode);
    entry.signed_in_with = Some(how.to_string());
    entry.hint = match &credential {
        Credential::Key { key } => Some(config::redact(key)),
        Credential::Browser { .. } => None,
    };
    if base_url != invoist::DEFAULT_BASE_URL {
        entry.base_url = Some(base_url.clone());
    }
    if config.default_profile.is_none() {
        config.default_profile = Some(profile.clone());
    }
    config.save()?;

    let what = match kind {
        AccountKind::Business => "a business",
        AccountKind::Household => "a household",
    };
    let test = if mode == Mode::Test {
        styled(WARN, " in test mode")
    } else {
        String::new()
    };
    out.note(format!(
        "{} Signed in to {what}{test} as profile {}.",
        styled(GOOD, "✓"),
        styled(BOLD, &profile)
    ));
    if storage == Storage::ConfigFile {
        out.note(styled(
            DIM,
            format!(
                "There's no keychain here, so the sign-in is kept in {} (readable only by you).",
                Config::path()?.display()
            ),
        ));
    }
    let next = match kind {
        AccountKind::Business => "invoist extract invoice.pdf",
        AccountKind::Household => "invoist bills",
    };
    out.note(styled(
        DIM,
        format!("Try `{next}`, or run `invoist` on its own for the full-screen app."),
    ));
    Ok(())
}

/// The key, from standard input when it's piped, otherwise asked for with
/// the typing hidden. Never a flag: flags end up in shell history.
fn read_key() -> Result<String> {
    let key = if std::io::stdin().is_terminal() {
        dialoguer::Password::new()
            .with_prompt("Paste your Invoist API key")
            .interact()
            .map_err(|e| CliError::Failed(format!("couldn't read the key: {e}")))?
    } else {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| CliError::io("couldn't read the key from standard input", e))?;
        s
    };
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(CliError::Usage("no key was given".into()));
    }
    if !key.starts_with("inv_") {
        return Err(CliError::Usage(
            "that doesn't look like an Invoist key: keys start with inv_sk_. Create one in Invoist under Integrations › API (a business) or Settings › AI assistants (a household)."
                .into(),
        ));
    }
    Ok(key)
}

pub async fn logout(
    profile_flag: Option<&str>,
    base_url_flag: Option<&str>,
    out: Out,
) -> Result<()> {
    let mut config = Config::load()?;
    let profile = config.profile_name(profile_flag);
    let Some(credential) = config::load_credential(&config, &profile) else {
        out.note(format!(
            "Profile {} wasn't signed in.",
            styled(BOLD, &profile)
        ));
        return Ok(());
    };
    let base_url = Session::base_url_for(&config, &profile, base_url_flag);
    oauth::revoke(&base_url, &credential).await;
    config::delete_credential(&mut config, &profile);
    if let Some(p) = config.profiles.get_mut(&profile) {
        p.kind = None;
        p.mode = None;
        p.signed_in_with = None;
        p.hint = None;
    }
    config.save()?;
    out.note(format!(
        "{} Signed out of profile {}.",
        styled(GOOD, "✓"),
        styled(BOLD, &profile)
    ));
    if std::env::var_os("INVOIST_API_KEY").is_some() {
        out.note(styled(
            WARN,
            "INVOIST_API_KEY is still set in this shell, so commands keep using it.",
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct StatusReport {
    profile: String,
    signed_in: bool,
    source: &'static str,
    kind: Option<AccountKind>,
    mode: Mode,
    base_url: String,
    key_hint: Option<String>,
    works: bool,
    problem: Option<String>,
}

pub async fn status(
    profile_flag: Option<&str>,
    base_url_flag: Option<&str>,
    out: Out,
) -> Result<()> {
    let mut session = Session::open(profile_flag, base_url_flag)?;
    let source = if session.is_from_env() {
        "INVOIST_API_KEY"
    } else {
        match session.credential() {
            Credential::Key { .. } => "saved key",
            Credential::Browser { .. } => "browser sign-in",
        }
    };
    let key_hint = match session.credential() {
        Credential::Key { key } => Some(config::redact(key)),
        Credential::Browser { .. } => None,
    };
    let checked = async {
        let client = session.client().await?;
        session::detect_kind(&client).await
    }
    .await;
    let (works, kind, problem) = match checked {
        Ok(kind) => (true, Some(kind), None),
        Err(e) => (false, session.kind, Some(e.headline())),
    };
    let report = StatusReport {
        profile: session.profile.clone(),
        signed_in: true,
        source,
        kind,
        mode: session.mode,
        base_url: session.base_url.clone(),
        key_hint,
        works,
        problem,
    };
    out.one(&report, || {
        let mut s = String::new();
        let line = |label: &str, value: String| {
            format!("  {}  {value}\n", styled(DIM, format!("{label:<9}")))
        };
        s.push_str(&line("profile", styled(BOLD, &report.profile)));
        s.push_str(&line(
            "account",
            report
                .kind
                .map(|k| k.label().to_string())
                .unwrap_or_else(|| "unknown".into()),
        ));
        s.push_str(&line(
            "mode",
            match report.mode {
                Mode::Live => "live".into(),
                Mode::Test => styled(WARN, "test (sample results, never charged)"),
            },
        ));
        s.push_str(&line(
            "signed in",
            match &report.key_hint {
                Some(hint) => format!("{} ({hint})", report.source),
                None => report.source.to_string(),
            },
        ));
        if report.base_url != invoist::DEFAULT_BASE_URL {
            s.push_str(&line("server", report.base_url.clone()));
        }
        s.push_str(&line(
            "works",
            if report.works {
                styled(GOOD, "yes")
            } else {
                styled(
                    WARN,
                    format!("no: {}", report.problem.clone().unwrap_or_default()),
                )
            },
        ));
        s
    })?;
    if !report.works {
        return Err(CliError::NotSignedIn(
            "The saved sign-in doesn't work any more. Run `invoist login`.".into(),
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct ProfileRow {
    name: String,
    default: bool,
    kind: Option<AccountKind>,
    mode: Option<Mode>,
    signed_in_with: Option<String>,
}

pub fn profile(cmd: ProfileCmd, out: Out) -> Result<()> {
    let mut config = Config::load()?;
    match cmd {
        ProfileCmd::List => {
            let default = config.profile_name(None);
            let rows: Vec<ProfileRow> = config
                .profiles
                .iter()
                .map(|(name, p)| ProfileRow {
                    name: name.clone(),
                    default: *name == default,
                    kind: p.kind,
                    mode: p.mode,
                    signed_in_with: p.signed_in_with.clone(),
                })
                .collect();
            out.list(
                &rows,
                &["", "profile", "account", "mode", "signed in with"],
                |r| {
                    vec![
                        if r.default { "*".into() } else { String::new() },
                        r.name.clone(),
                        r.kind
                            .map(|k| k.label().to_string())
                            .unwrap_or_else(|| "—".into()),
                        match r.mode {
                            Some(Mode::Test) => "test".into(),
                            Some(Mode::Live) => "live".into(),
                            None => "—".into(),
                        },
                        r.signed_in_with
                            .clone()
                            .unwrap_or_else(|| "not signed in".into()),
                    ]
                },
                "No profiles yet. Run `invoist login`.",
            )
        }
        ProfileCmd::Use { name } => {
            if !config.profiles.contains_key(&name) {
                return Err(CliError::NotFound(format!(
                    "there's no profile called `{name}`. Sign in to it with `invoist login --profile {name}`."
                )));
            }
            config.default_profile = Some(name.clone());
            config.save()?;
            out.note(format!(
                "{} Commands now use profile {}.",
                styled(GOOD, "✓"),
                styled(BOLD, &name)
            ));
            Ok(())
        }
    }
}
