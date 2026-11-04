//! One module per group of commands. Each fetches through the SDK and hands
//! the result to `output`, which decides how it's shown.

pub mod auth;
pub mod bills;
pub mod documents;
pub mod extract;
pub mod review;
pub mod webhooks;
pub mod workspace;

use std::path::Path;

use clap::CommandFactory;

use crate::cli::Cli;
use crate::error::{CliError, Result};
use crate::output::Out;

/// Writes `invoist.1` and a page per command into `dir`.
pub fn man(dir: &Path, out: Out) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| CliError::io(format!("couldn't create {}", dir.display()), e))?;
    clap_mangen::generate_to(Cli::command(), dir)
        .map_err(|e| CliError::io(format!("couldn't write man pages to {}", dir.display()), e))?;
    out.note(format!("Wrote the man pages to {}", dir.display()));
    Ok(())
}

/// Asks a yes or no question on the terminal. Without a terminal it refuses
/// rather than waiting forever, and tells the person to pass `--yes`.
pub fn confirm(question: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !crate::output::interactive() {
        return Err(CliError::Usage(format!(
            "{question} Add --yes to confirm when there's no terminal to ask on."
        )));
    }
    dialoguer::Confirm::new()
        .with_prompt(question)
        .default(false)
        .interact()
        .map_err(|e| CliError::Failed(format!("couldn't ask: {e}")))
}
