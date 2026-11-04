//! `invoist extract`: upload files, wait for Invoist to read them, and show
//! what it read.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use invoist::models::{Document, Extraction, ExtractionStatus, ReviewQueueItem, ReviewStatus};
use invoist::{Client, ExtractOptions, UploadFile, WaitOptions};
use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;

use crate::cli::ExtractArgs;
use crate::error::{CliError, Result};
use crate::output::{self, BAD, DIM, GOOD, Out, WARN, money, reason_label, short_id, styled};
use crate::session::Session;

/// The API's limit: files must be under 20 MB.
const MAX_BYTES: u64 = 20 * 1024 * 1024;

/// What happened to one file. With `--json`, the SDK's own document and
/// extraction are included unchanged.
#[derive(Debug, Serialize)]
struct FileResult {
    file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    document: Option<Document>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extraction: Option<Extraction>,
    duplicate: bool,
    /// Why the invoice was set aside for a person, when it was.
    needs_check: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// One thing to upload: a file on disk, or standard input.
enum Source {
    Path(PathBuf),
    Stdin(String),
}

impl Source {
    fn name(&self) -> String {
        match self {
            Source::Path(p) => p.display().to_string(),
            Source::Stdin(name) => name.clone(),
        }
    }
}

fn readable_type(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("pdf" | "png" | "jpg" | "jpeg")
    )
}

/// Expands the arguments: folders are searched (in name order) for files
/// Invoist can read; files are checked before anything is uploaded.
fn collect(args: &ExtractArgs) -> Result<Vec<Source>> {
    let mut sources = Vec::new();
    let mut stdin_seen = false;
    for path in &args.paths {
        if path.as_os_str() == "-" {
            if stdin_seen {
                return Err(CliError::Usage(
                    "standard input (`-`) can be read only once".into(),
                ));
            }
            stdin_seen = true;
            sources.push(Source::Stdin(args.name.clone()));
        } else if path.is_dir() {
            let mut found = Vec::new();
            walk(path, &mut found)?;
            found.sort();
            if found.is_empty() {
                return Err(CliError::Usage(format!(
                    "{} has no PDF, PNG or JPEG files",
                    path.display()
                )));
            }
            sources.extend(found.into_iter().map(Source::Path));
        } else if path.is_file() {
            if !readable_type(path) {
                return Err(CliError::Usage(format!(
                    "{} isn't a PDF, PNG or JPEG file, which is what Invoist reads",
                    path.display()
                )));
            }
            sources.push(Source::Path(path.clone()));
        } else {
            return Err(CliError::NotFound(format!(
                "{} doesn't exist",
                path.display()
            )));
        }
    }
    for source in &sources {
        if let Source::Path(p) = source {
            let size = std::fs::metadata(p)
                .map_err(|e| CliError::io(format!("couldn't read {}", p.display()), e))?
                .len();
            if size > MAX_BYTES {
                return Err(CliError::Usage(format!(
                    "{} is {:.1} MB; Invoist reads files up to 20 MB",
                    p.display(),
                    size as f64 / 1_048_576.0
                )));
            }
        }
    }
    Ok(sources)
}

fn walk(dir: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| CliError::io(format!("couldn't read {}", dir.display()), e))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk(&path, found)?;
        } else if readable_type(&path) {
            found.push(path);
        }
    }
    Ok(())
}

async fn load(source: &Source) -> Result<UploadFile> {
    match source {
        Source::Path(p) => UploadFile::from_path_async(p)
            .await
            .map_err(|e| CliError::io(format!("couldn't read {}", p.display()), e)),
        Source::Stdin(name) => {
            let mut data = Vec::new();
            tokio::io::stdin()
                .read_to_end(&mut data)
                .await
                .map_err(|e| CliError::io("couldn't read standard input", e))?;
            if data.is_empty() {
                return Err(CliError::Usage("standard input was empty".into()));
            }
            if data.len() as u64 > MAX_BYTES {
                return Err(CliError::Usage(
                    "the file on standard input is over 20 MB".into(),
                ));
            }
            Ok(UploadFile::new(name.clone(), data))
        }
    }
}

struct Job {
    client: Client,
    idempotency_key: Option<String>,
    wait: Option<WaitOptions>,
}

async fn process(job: &Job, source: &Source, bar: Option<&ProgressBar>) -> FileResult {
    let file = source.name();
    let mut result = FileResult {
        file: file.clone(),
        document: None,
        extraction: None,
        duplicate: false,
        needs_check: Vec::new(),
        error: None,
    };
    let step = |msg: &str| {
        if let Some(bar) = bar {
            bar.set_message(format!("{file}  {}", styled(DIM, msg)));
        }
    };

    let outcome: Result<()> = async {
        step("uploading");
        let upload = load(source).await?;
        let mut options = ExtractOptions::new();
        options.idempotency_key = job.idempotency_key.clone();
        let extracted = job.client.extract(upload, options).await?;
        result.duplicate = extracted.duplicate;
        let id = extracted.document.id.clone();
        result.document = Some(extracted.document);
        result.extraction = extracted.extraction;

        let Some(wait) = job.wait else { return Ok(()) };
        let pending = result
            .extraction
            .as_ref()
            .is_none_or(|e| e.status == ExtractionStatus::Pending);
        if pending {
            step("reading");
            result.extraction = Some(job.client.wait_for_extraction(&id, wait).await?);
        }
        // What, if anything, was set aside for a person to check.
        let detail = job.client.get_document(&id).await?;
        result.document = Some(detail.document);
        result.needs_check = open_checks(&detail.review_queue);
        Ok(())
    }
    .await;
    if let Err(e) = outcome {
        result.error = Some(e.headline());
    }
    if let Some(bar) = bar {
        bar.finish_with_message(summary_line(&result));
    }
    result
}

fn open_checks(items: &[ReviewQueueItem]) -> Vec<String> {
    items
        .iter()
        .filter(|i| i.status == ReviewStatus::Pending)
        .map(|i| reason_label(&i.reason))
        .collect()
}

fn summary_line(r: &FileResult) -> String {
    if let Some(error) = &r.error {
        return format!("{} {}  {}", styled(BAD, "✗"), r.file, styled(BAD, error));
    }
    let id = r
        .document
        .as_ref()
        .map(|d| short_id(&d.id))
        .unwrap_or_default();
    let (vendor, total) = match &r.extraction {
        Some(e) if e.status == ExtractionStatus::Done => (
            e.vendor_name
                .clone()
                .unwrap_or_else(|| "supplier not found".into()),
            money(e.total_cents, e.currency.as_deref()),
        ),
        Some(e) if e.status == ExtractionStatus::Failed => {
            return format!(
                "{} {}  {}",
                styled(BAD, "✗"),
                r.file,
                styled(
                    BAD,
                    e.error_message
                        .clone()
                        .unwrap_or_else(|| "Invoist couldn't read it".into())
                )
            );
        }
        _ => {
            return format!(
                "{} {}  {}",
                styled(DIM, "·"),
                r.file,
                styled(DIM, format!("uploaded as {id}, still reading"))
            );
        }
    };
    let mut line = format!(
        "{} {}  {vendor} · {total}  {}",
        styled(GOOD, "✓"),
        r.file,
        styled(DIM, &id)
    );
    if r.duplicate {
        line.push_str(&format!("  {}", styled(DIM, "(already uploaded)")));
    }
    if !r.needs_check.is_empty() {
        line.push_str(&format!(
            "  {}",
            styled(WARN, format!("check: {}", r.needs_check.join(", ")))
        ));
    }
    line
}

pub async fn run(
    profile: Option<&str>,
    base_url: Option<&str>,
    args: ExtractArgs,
    out: Out,
) -> Result<()> {
    let sources = collect(&args)?;
    if args.idempotency_key.is_some() && sources.len() != 1 {
        return Err(CliError::Usage(
            "--idempotency-key works with one file at a time".into(),
        ));
    }
    let mut session = Session::open(profile, base_url)?;
    let job = Arc::new(Job {
        client: session.client().await?,
        idempotency_key: args.idempotency_key.clone(),
        wait: (!args.no_wait)
            .then(|| WaitOptions::new().timeout(Duration::from_secs(args.timeout))),
    });

    let show_progress = !out.quiet && !out.is_machine() && output::interactive();
    let bars = show_progress.then(MultiProgress::new);
    let style = ProgressStyle::with_template("{spinner:.blue} {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_spinner())
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✓"]);

    let gate = Arc::new(Semaphore::new(args.concurrency as usize));
    let mut tasks = tokio::task::JoinSet::new();
    let sources = Arc::new(sources);
    for index in 0..sources.len() {
        let bar = bars.as_ref().map(|m| {
            let bar = m.add(ProgressBar::new_spinner());
            bar.set_style(style.clone());
            bar.enable_steady_tick(Duration::from_millis(90));
            bar.set_message(format!(
                "{}  {}",
                sources[index].name(),
                styled(DIM, "waiting")
            ));
            bar
        });
        let (job, gate, sources) = (job.clone(), gate.clone(), sources.clone());
        tasks.spawn(async move {
            let _permit = gate.acquire_owned().await;
            (index, process(&job, &sources[index], bar.as_ref()).await)
        });
    }

    let mut results: Vec<Option<FileResult>> = (0..sources.len()).map(|_| None).collect();
    loop {
        tokio::select! {
            next = tasks.join_next() => match next {
                Some(Ok((index, r))) => results[index] = Some(r),
                Some(Err(e)) => return Err(CliError::Failed(format!("an upload stopped unexpectedly: {e}"))),
                None => break,
            },
            _ = tokio::signal::ctrl_c() => {
                tasks.abort_all();
                return Err(CliError::Interrupted);
            }
        }
    }
    let results: Vec<FileResult> = results.into_iter().flatten().collect();

    if out.is_machine() {
        out.list(
            &results,
            &[
                "file",
                "document_id",
                "supplier",
                "invoice_number",
                "total",
                "currency",
                "due_date",
                "duplicate",
                "needs_check",
                "error",
            ],
            |r| {
                let e = r.extraction.as_ref();
                vec![
                    r.file.clone(),
                    r.document
                        .as_ref()
                        .map(|d| d.id.clone())
                        .unwrap_or_default(),
                    e.and_then(|e| e.vendor_name.clone()).unwrap_or_default(),
                    e.and_then(|e| e.invoice_number.clone()).unwrap_or_default(),
                    e.and_then(|e| e.total_cents)
                        .map(output::decimal)
                        .unwrap_or_default(),
                    e.and_then(|e| e.currency.clone()).unwrap_or_default(),
                    e.and_then(|e| e.due_date.clone()).unwrap_or_default(),
                    r.duplicate.to_string(),
                    r.needs_check.join("; "),
                    r.error.clone().unwrap_or_default(),
                ]
            },
            "",
        )?;
    } else if !show_progress {
        // Without live progress, one line per file once it's done.
        for r in &results {
            let _ = std::io::Write::write_all(
                &mut anstream::stdout(),
                format!("{}\n", summary_line(r)).as_bytes(),
            );
        }
    }

    let failed = results.iter().filter(|r| r.error.is_some()).count();
    let flagged = results.iter().filter(|r| !r.needs_check.is_empty()).count();
    if !out.is_machine() && results.len() > 1 {
        out.note(styled(
            DIM,
            format!(
                "{} read, {flagged} to check, {failed} failed",
                results.len() - failed
            ),
        ));
    }
    if failed > 0 {
        return Err(CliError::Failed(format!(
            "{failed} of {} {} couldn't be read",
            results.len(),
            if results.len() == 1 { "file" } else { "files" }
        )));
    }
    if args.fail_on_review && flagged > 0 {
        return Err(CliError::NeedsReview(format!(
            "{flagged} {} set aside for a person to check: see `invoist review list`",
            if flagged == 1 {
                "invoice was"
            } else {
                "invoices were"
            }
        )));
    }
    Ok(())
}
