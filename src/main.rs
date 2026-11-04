//! `invoist`: the Invoist command line and terminal app.
//!
//! Every command talks to the API through the official Rust SDK (`invoist`
//! on crates.io), so retries, errors and webhook checks behave exactly as
//! they do in every other Invoist library.

mod cli;
mod commands;
mod config;
mod error;
mod ids;
mod oauth;
mod output;
mod session;
mod tui;

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use crate::cli::{Cli, Command};
use crate::error::{CliError, exit};
use crate::output::Out;

/// `invoist --version`: the CLI's version, and the SDK it was built with,
/// read from the SDK's own user agent (`invoist-rust/1.0.1 (rust)`).
pub fn version_line() -> &'static str {
    static LINE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LINE.get_or_init(|| {
        let sdk = invoist::USER_AGENT
            .split_whitespace()
            .next()
            .and_then(|p| p.split('/').nth(1))
            .unwrap_or("unknown");
        format!("{} (Rust SDK {sdk})", env!("CARGO_PKG_VERSION"))
    })
}

/// The `User-Agent` the CLI's own requests (signing in) send.
pub const USER_AGENT: &str = concat!("invoist-cli/", env!("CARGO_PKG_VERSION"));

fn main() -> ExitCode {
    let cli = Cli::parse();
    output::init_color(cli.global.no_color);
    let json_errors = cli.global.format() != output::Format::Table;

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            CliError::io("couldn't start", e).report(json_errors);
            return ExitCode::from(exit::FAILED);
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::from(exit::OK),
        Err(e) => {
            e.report(json_errors);
            ExitCode::from(e.exit_code())
        }
    }
}

async fn run(cli: Cli) -> error::Result<()> {
    let out = Out {
        format: cli.global.format(),
        quiet: cli.global.quiet,
    };
    let g = &cli.global;
    let profile = g.profile.as_deref();
    let base_url = g.base_url.as_deref();

    let Some(command) = cli.command else {
        // On its own in a terminal: the full-screen app. Anywhere else (a
        // pipe, CI), help, so a script never ends up in an interactive screen.
        if std::io::stdout().is_terminal()
            && std::io::stdin().is_terminal()
            && std::env::var_os("CI").is_none()
        {
            return tui::run(profile, base_url).await;
        }
        let _ = Cli::command().print_help();
        return Ok(());
    };

    match command {
        Command::Login(args) => commands::auth::login(profile, base_url, args, out).await,
        Command::Logout => commands::auth::logout(profile, base_url, out).await,
        Command::Status => commands::auth::status(profile, base_url, out).await,
        Command::Profile(cmd) => commands::auth::profile(cmd, out),
        Command::Extract(args) => commands::extract::run(profile, base_url, args, out).await,
        Command::Documents(cmd) => commands::documents::run(profile, base_url, cmd, out).await,
        Command::Export(args) => commands::documents::export(profile, base_url, args, out).await,
        Command::Review(cmd) => commands::review::run(profile, base_url, cmd, out).await,
        Command::Categories => commands::workspace::categories(profile, base_url, out).await,
        Command::Usage => commands::workspace::usage(profile, base_url, out).await,
        Command::Stats => commands::workspace::stats(profile, base_url, out).await,
        Command::Webhooks(cmd) => commands::webhooks::run(profile, base_url, cmd, out).await,
        Command::Bills(args) => commands::bills::run(profile, base_url, args, out).await,
        Command::HeldEmails => commands::bills::held_emails(profile, base_url, out).await,
        Command::Ui => tui::run(profile, base_url).await,
        Command::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "invoist",
                &mut std::io::stdout(),
            );
            Ok(())
        }
        Command::Man { dir } => commands::man(&dir, out),
    }
}
