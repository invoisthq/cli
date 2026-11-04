//! Everything that can stop a command, the exit code it ends with, and how
//! it is shown: one plain line saying what went wrong, then the API's code
//! and request id so a support request has what it needs.

use std::io::Write;

use serde_json::json;

/// Exit codes. Documented in the README; scripts rely on them.
pub mod exit {
    /// Everything worked.
    pub const OK: u8 = 0;
    /// The API refused or failed, or the network did.
    pub const FAILED: u8 = 1;
    /// The command was used wrongly (clap uses 2 as well).
    pub const USAGE: u8 = 2;
    /// Not signed in, or the key or sign-in was refused.
    pub const AUTH: u8 = 3;
    /// The document, bill or endpoint doesn't exist.
    pub const NOT_FOUND: u8 = 4;
    /// An extraction finished but needs a person to check it (only with
    /// `--fail-on-review`).
    pub const NEEDS_REVIEW: u8 = 5;
    /// Stopped with Ctrl-C.
    pub const INTERRUPTED: u8 = 130;
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Api(#[from] invoist::Error),
    #[error("{0}")]
    NotSignedIn(String),
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    NeedsReview(String),
    #[error("{0}")]
    Failed(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("stopped")]
    Interrupted,
}

pub type Result<T, E = CliError> = std::result::Result<T, E>;

impl CliError {
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        CliError::Io {
            context: context.into(),
            source,
        }
    }

    pub fn exit_code(&self) -> u8 {
        match self {
            CliError::Api(e) if e.is_auth_error() => exit::AUTH,
            CliError::Api(e) if e.is_not_found() => exit::NOT_FOUND,
            CliError::Api(_) => exit::FAILED,
            CliError::NotSignedIn(_) => exit::AUTH,
            CliError::Usage(_) => exit::USAGE,
            CliError::NotFound(_) => exit::NOT_FOUND,
            CliError::NeedsReview(_) => exit::NEEDS_REVIEW,
            CliError::Failed(_) | CliError::Io { .. } => exit::FAILED,
            CliError::Interrupted => exit::INTERRUPTED,
        }
    }

    /// What went wrong, said plainly. For an API error this explains the
    /// common codes; anything else shows the API's own message.
    pub fn headline(&self) -> String {
        match self {
            CliError::Api(e) => explain(e),
            other => other.to_string(),
        }
    }

    /// Prints the error to stderr: as a line of text, or as JSON with `--json`.
    pub fn report(&self, json_output: bool) {
        let mut err = anstream::stderr();
        if json_output {
            let body = match self {
                CliError::Api(e) => json!({
                    "error": {
                        "message": self.headline(),
                        "code": e.code(),
                        "status": e.status(),
                        "request_id": e.request_id(),
                    }
                }),
                _ => json!({ "error": { "message": self.headline(), "code": self.code() } }),
            };
            let _ = writeln!(err, "{body}");
            return;
        }
        let red = anstyle::AnsiColor::Red.on_default().bold();
        let dim = anstyle::Style::new().dimmed();
        let _ = writeln!(err, "{red}error:{red:#} {}", self.headline());
        if let CliError::Api(e) = self {
            let mut detail = format!("code {}", e.code());
            if e.status() != 0 {
                detail = format!("status {}, {detail}", e.status());
            }
            if let Some(id) = e.request_id() {
                detail.push_str(&format!(", request {id}"));
            }
            let _ = writeln!(err, "  {dim}{detail}{dim:#}");
        }
    }

    fn code(&self) -> &'static str {
        match self {
            CliError::Api(_) => "api_error",
            CliError::NotSignedIn(_) => "not_signed_in",
            CliError::Usage(_) => "usage",
            CliError::NotFound(_) => "not_found",
            CliError::NeedsReview(_) => "needs_review",
            CliError::Failed(_) => "failed",
            CliError::Io { .. } => "io",
            CliError::Interrupted => "interrupted",
        }
    }
}

/// The API's common error codes in plain words, with what to do next.
fn explain(e: &invoist::Error) -> String {
    match e.code() {
        "invalid_api_key" | "invalid_token" | "unauthorized" => {
            "Invoist refused the key or sign-in. Run `invoist login` to sign in again.".into()
        }
        "business_feature" => {
            "This is for Invoist for businesses, and you're signed in to a household. Try `invoist bills`.".into()
        }
        "household_feature" => {
            "This is for households on Invoist Personal, and you're signed in to a business. Try `invoist documents list`.".into()
        }
        "insufficient_scope" => format!(
            "Your sign-in doesn't allow this ({}). Sign in again with `invoist login` and approve it, or use a key that has it.",
            e.message()
        ),
        "rate_limited" => "Invoist is getting too many requests from you. Wait a moment and try again.".into(),
        "upload_too_large" => "That file is too big. Invoist reads files up to 20 MB.".into(),
        "upload_type" => "Invoist reads PDF, PNG and JPEG files only.".into(),
        "included_quota_exhausted" => {
            "Your plan's invoices for this month are used up. Buy credits or change plan in Invoist under Billing.".into()
        }
        "subscription_required" => "Your workspace needs an active plan to read invoices. Choose one in Invoist under Billing.".into(),
        "idempotency_key_reused" => {
            "That idempotency key was already used for a different file. Use a new key for a new upload.".into()
        }
        "extraction_timeout" => {
            "Invoist is still reading this invoice. Check it later with `invoist documents get <id>`.".into()
        }
        "timeout" => "Invoist didn't answer in time. Check your connection and try again.".into(),
        "network_error" => "Couldn't reach Invoist. Check your connection and try again.".into(),
        _ => e.message().to_string(),
    }
}
