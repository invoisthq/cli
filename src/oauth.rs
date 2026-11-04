//! Signing in through the browser: OAuth 2.0 with PKCE and a loopback
//! redirect (RFC 8252), the way desktop apps and other CLIs do it.
//!
//! 1. Read the API's OAuth settings from `/.well-known/oauth-authorization-server`.
//! 2. Register the CLI as a public client, once per profile.
//! 3. Listen on a free port on 127.0.0.1, open Invoist's approval page in the
//!    browser, and wait for it to send the person back with a code.
//! 4. Swap the code (and the PKCE verifier) for an access token and a
//!    refresh token. The access token lasts an hour; the CLI refreshes it.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use invoist::reqwest;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::config::Credential;
use crate::error::{CliError, Result};

/// How long the CLI waits for the person to approve in the browser.
const APPROVAL_WAIT: Duration = Duration::from_secs(300);
/// Refresh this long before the access token actually expires.
const REFRESH_MARGIN: u64 = 60;

#[derive(Debug, Clone, Deserialize)]
pub struct Metadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub revocation_endpoint: Option<String>,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Registration {
    client_id: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    #[serde(default)]
    scope: String,
}

#[derive(Debug, Deserialize)]
struct OAuthError {
    error: String,
    error_description: Option<String>,
}

fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(crate::USER_AGENT)
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| CliError::Failed(format!("couldn't start an HTTPS client: {e}")))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn network(e: reqwest::Error) -> CliError {
    CliError::Failed(format!("couldn't reach Invoist to sign in: {e}"))
}

/// Reads a JSON response, turning an OAuth error body into a plain message.
async fn read<T: for<'de> Deserialize<'de>>(res: reqwest::Response, what: &str) -> Result<T> {
    let status = res.status();
    let body = res.text().await.map_err(network)?;
    if status.is_success() {
        return serde_json::from_str(&body).map_err(|e| {
            CliError::Failed(format!("Invoist sent an unexpected answer to {what}: {e}"))
        });
    }
    let detail = serde_json::from_str::<OAuthError>(&body)
        .map(|e| e.error_description.unwrap_or(e.error))
        .unwrap_or_else(|_| format!("status {status}"));
    Err(CliError::Failed(format!("{what} failed: {detail}")))
}

pub async fn discover(base_url: &str) -> Result<Metadata> {
    let url = format!(
        "{}/.well-known/oauth-authorization-server",
        base_url.trim_end_matches('/')
    );
    let res = http()?.get(url).send().await.map_err(network)?;
    read(res, "reading Invoist's sign-in settings").await
}

/// Registers the CLI as a public client. Loopback redirects may use any
/// port (RFC 8252 §7.3), so one registration serves every sign-in.
pub async fn register(meta: &Metadata) -> Result<String> {
    let endpoint = meta
        .registration_endpoint
        .as_deref()
        .ok_or_else(|| CliError::Failed("this Invoist server doesn't allow signing in from the CLI; use `invoist login --with-key`".into()))?;
    let body = serde_json::json!({
        "client_name": "Invoist CLI",
        "client_uri": "https://invoi.st/docs/cli",
        "redirect_uris": ["http://127.0.0.1/callback"],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let res = http()?
        .post(endpoint)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(network)?;
    Ok(read::<Registration>(res, "registering the CLI")
        .await?
        .client_id)
}

fn random_token(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf)
        .map_err(|e| CliError::Failed(format!("couldn't make a random value: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Everything the sign-in page needs, ready before the browser opens.
pub struct Pending {
    listener: TcpListener,
    redirect_uri: String,
    verifier: String,
    state: String,
    pub authorize_url: String,
}

pub async fn begin(meta: &Metadata, client_id: &str) -> Result<Pending> {
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| {
        CliError::io(
            "couldn't listen on this computer for the browser to come back",
            e,
        )
    })?;
    let port = listener
        .local_addr()
        .map_err(|e| CliError::io("couldn't listen locally", e))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let verifier = random_token(48)?;
    let state = random_token(24)?;

    let mut url = reqwest::Url::parse(&meta.authorization_endpoint)
        .map_err(|e| CliError::Failed(format!("Invoist's sign-in address is invalid: {e}")))?;
    // Ask for every scope the server offers; it grants only what this kind
    // of account can have, and the approval page shows the person exactly that.
    let scope = meta.scopes_supported.join(" ");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", &scope)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge(&verifier))
        .append_pair("code_challenge_method", "S256");
    Ok(Pending {
        listener,
        redirect_uri,
        verifier,
        state,
        authorize_url: url.to_string(),
    })
}

const DONE_PAGE: &str = r##"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Signed in · Invoist CLI</title><style>body{margin:0;min-height:100vh;display:grid;place-items:center;background:#f4f5f9;color:#12141f;font:16px/1.6 system-ui,-apple-system,"Segoe UI",sans-serif}main{max-width:420px;padding:40px;text-align:center}.mark{width:56px;height:56px;margin:0 auto 20px;border-radius:16px;background:#3452eb;display:grid;place-items:center}h1{margin:0 0 8px;font-size:24px;letter-spacing:-.02em}p{margin:0;color:#5b6072}</style></head><body><main><div class="mark"><svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg></div><h1>You're signed in</h1><p>Go back to your terminal. You can close this tab.</p></main></body></html>"##;

const FAILED_PAGE: &str = r##"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Not signed in · Invoist CLI</title><style>body{margin:0;min-height:100vh;display:grid;place-items:center;background:#f4f5f9;color:#12141f;font:16px/1.6 system-ui,-apple-system,"Segoe UI",sans-serif}main{max-width:420px;padding:40px;text-align:center}h1{margin:0 0 8px;font-size:24px;letter-spacing:-.02em}p{margin:0;color:#5b6072}</style></head><body><main><h1>Not signed in</h1><p>Go back to your terminal to see why, then try again.</p></main></body></html>"##;

/// What came back on the redirect.
struct Callback {
    code: Option<String>,
    state: Option<String>,
    iss: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

fn parse_callback(request_line: &str) -> Option<Callback> {
    // "GET /callback?code=...&state=... HTTP/1.1"
    let target = request_line.split_whitespace().nth(1)?;
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    if url.path() != "/callback" {
        return None;
    }
    let mut cb = Callback {
        code: None,
        state: None,
        iss: None,
        error: None,
        error_description: None,
    };
    for (k, v) in url.query_pairs() {
        let v = Some(v.into_owned());
        match k.as_ref() {
            "code" => cb.code = v,
            "state" => cb.state = v,
            "iss" => cb.iss = v,
            "error" => cb.error = v,
            "error_description" => cb.error_description = v,
            _ => {}
        }
    }
    Some(cb)
}

/// How long one connection may take to send its request.
const REQUEST_WAIT: Duration = Duration::from_secs(10);
/// The most a request's line and headers may take up.
const MAX_REQUEST: usize = 16 * 1024;

/// Reads one request up to the end of its headers. A request can arrive in
/// several pieces, and the whole head is read before answering so that
/// closing never cuts off a browser that is still sending.
async fn read_head(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut head = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        head.extend_from_slice(&chunk[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            return Some(String::from_utf8_lossy(&head).into_owned());
        }
        if head.len() > MAX_REQUEST {
            return None;
        }
    }
}

/// Hands the redirect back to `finish`, and turns away anything else (a
/// favicon request, a stray visit) with a 404.
async fn read_callback(
    mut stream: tokio::net::TcpStream,
    found: tokio::sync::mpsc::Sender<(tokio::net::TcpStream, Callback)>,
) {
    let Ok(Some(head)) = tokio::time::timeout(REQUEST_WAIT, read_head(&mut stream)).await else {
        return;
    };
    match head.lines().next().and_then(parse_callback) {
        Some(cb) => {
            let _ = found.send((stream, cb)).await;
        }
        None => respond(&mut stream, "404 Not Found", "").await,
    }
}

async fn respond(stream: &mut tokio::net::TcpStream, status: &str, page: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Waits for the browser to come back, then swaps the code for tokens.
pub async fn finish(meta: &Metadata, client_id: &str, pending: Pending) -> Result<Credential> {
    let wait = async {
        // Each connection is read on its own task: browsers open spare
        // connections that send nothing, and one of those must never hold
        // up the real redirect behind it.
        let (found_tx, mut found_rx) = tokio::sync::mpsc::channel(4);
        let (mut stream, cb) = loop {
            tokio::select! {
                accepted = pending.listener.accept() => {
                    let (stream, _) = accepted
                        .map_err(|e| CliError::io("lost the local sign-in listener", e))?;
                    tokio::spawn(read_callback(stream, found_tx.clone()));
                }
                Some(found) = found_rx.recv() => break found,
            }
        };
        if let Some(error) = cb.error {
            respond(&mut stream, "200 OK", FAILED_PAGE).await;
            let why = cb.error_description.unwrap_or_else(|| error.clone());
            return Err(if error == "access_denied" {
                CliError::Failed(
                    "You didn't approve the sign-in in the browser, so nothing changed.".into(),
                )
            } else {
                CliError::Failed(format!("Signing in didn't work: {why}"))
            });
        }
        if cb.state.as_deref() != Some(pending.state.as_str()) {
            respond(&mut stream, "400 Bad Request", FAILED_PAGE).await;
            return Err(CliError::Failed(
                "The browser came back from a different sign-in. Run `invoist login` again.".into(),
            ));
        }
        if let Some(iss) = &cb.iss
            && iss.trim_end_matches('/') != meta.issuer.trim_end_matches('/')
        {
            respond(&mut stream, "400 Bad Request", FAILED_PAGE).await;
            return Err(CliError::Failed("The sign-in came back from a different server than the one asked. Nothing was saved.".into()));
        }
        let Some(code) = cb.code else {
            respond(&mut stream, "400 Bad Request", FAILED_PAGE).await;
            return Err(CliError::Failed(
                "The browser came back without a sign-in code. Run `invoist login` again.".into(),
            ));
        };
        respond(&mut stream, "200 OK", DONE_PAGE).await;
        Ok(code)
    };
    let code = tokio::select! {
        r = tokio::time::timeout(APPROVAL_WAIT, wait) => r.map_err(|_| CliError::Failed(
            "Nothing came back from the browser in 5 minutes. Run `invoist login` again when you're ready.".into()))??,
        _ = tokio::signal::ctrl_c() => return Err(CliError::Interrupted),
    };

    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "redirect_uri": pending.redirect_uri,
        "client_id": client_id,
        "code_verifier": pending.verifier,
    });
    let res = http()?
        .post(&meta.token_endpoint)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(network)?;
    let tokens: TokenResponse = read(res, "finishing the sign-in").await?;
    Ok(Credential::Browser {
        refresh_token: tokens.refresh_token.ok_or_else(|| {
            CliError::Failed(
                "Invoist didn't send a refresh token, so the sign-in couldn't be kept.".into(),
            )
        })?,
        expires_at: now() + tokens.expires_in.unwrap_or(3600),
        access_token: tokens.access_token,
        client_id: client_id.to_string(),
        token_endpoint: meta.token_endpoint.clone(),
        scope: tokens.scope,
    })
}

/// Whether a signed-in profile's access token needs refreshing.
pub fn needs_refresh(credential: &Credential) -> bool {
    matches!(credential, Credential::Browser { expires_at, .. } if now() + REFRESH_MARGIN >= *expires_at)
}

/// Swaps the refresh token for a new access token (and a new refresh token,
/// which the server rotates).
pub async fn refresh(credential: &Credential) -> Result<Credential> {
    let Credential::Browser {
        refresh_token,
        client_id,
        token_endpoint,
        scope,
        ..
    } = credential
    else {
        return Ok(credential.clone());
    };
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": client_id,
    });
    let res = http()?
        .post(token_endpoint)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(network)?;
    let tokens: TokenResponse = read(res, "renewing your sign-in").await.map_err(|_| {
        CliError::NotSignedIn(
            "Your sign-in has ended. Run `invoist login` to sign in again.".into(),
        )
    })?;
    Ok(Credential::Browser {
        access_token: tokens.access_token,
        refresh_token: tokens
            .refresh_token
            .unwrap_or_else(|| refresh_token.clone()),
        expires_at: now() + tokens.expires_in.unwrap_or(3600),
        client_id: client_id.clone(),
        token_endpoint: token_endpoint.clone(),
        scope: if tokens.scope.is_empty() {
            scope.clone()
        } else {
            tokens.scope
        },
    })
}

/// Tells the server to forget a sign-in. Best effort: logging out locally
/// always works, even when the server can't be reached.
pub async fn revoke(base_url: &str, credential: &Credential) {
    let Credential::Browser {
        refresh_token,
        client_id,
        ..
    } = credential
    else {
        return;
    };
    let Ok(meta) = discover(base_url).await else {
        return;
    };
    let Some(endpoint) = meta.revocation_endpoint else {
        return;
    };
    let body = serde_json::json!({ "token": refresh_token, "token_type_hint": "refresh_token", "client_id": client_id });
    if let Ok(client) = http() {
        let _ = client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await;
    }
}

/// Opens a URL in the person's browser. Returns false when it couldn't,
/// such as over SSH, so the caller can print the link instead.
pub fn open_browser(url: &str) -> bool {
    if std::env::var_os("INVOIST_NO_BROWSER").is_some() {
        return false;
    }
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).status();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    matches!(result, Ok(status) if status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc7636_example() {
        // RFC 7636, appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn callback_reads_code_state_and_issuer() {
        let cb = parse_callback(
            "GET /callback?code=abc&state=xyz&iss=https%3A%2F%2Fapi.invoi.st HTTP/1.1",
        )
        .unwrap();
        assert_eq!(cb.code.as_deref(), Some("abc"));
        assert_eq!(cb.state.as_deref(), Some("xyz"));
        assert_eq!(cb.iss.as_deref(), Some("https://api.invoi.st"));
        assert!(parse_callback("GET /favicon.ico HTTP/1.1").is_none());
    }

    #[test]
    fn refresh_is_due_a_minute_before_expiry() {
        let mk = |expires_at| Credential::Browser {
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at,
            client_id: String::new(),
            token_endpoint: String::new(),
            scope: String::new(),
        };
        assert!(needs_refresh(&mk(now() + 30)));
        assert!(!needs_refresh(&mk(now() + 3000)));
        assert!(!needs_refresh(&Credential::Key { key: "k".into() }));
    }
}
