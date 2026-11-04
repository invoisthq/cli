//! `invoist documents …` and `invoist export`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use invoist::models::{DocumentDetail, DocumentListItem, ExtractionStatus, ReviewStatus};
use invoist::{Client, ListDocumentsParams};
use serde::Serialize;
use tokio::sync::Semaphore;

use crate::cli::{DocumentsCmd, ExportArgs, Paging};
use crate::error::{CliError, Result};
use crate::ids;
use crate::output::{
    self, BAD, BOLD, DIM, Format, GOOD, Out, WARN, day, money, reason_label, short_id,
    status_label, styled,
};
use crate::session::Session;

const PAGE: u32 = 200;

pub async fn run(
    profile: Option<&str>,
    base_url: Option<&str>,
    cmd: DocumentsCmd,
    out: Out,
) -> Result<()> {
    let mut session = Session::open(profile, base_url)?;
    let client = session.client().await?;
    match cmd {
        DocumentsCmd::List {
            status,
            flagged,
            search,
            category,
            paging,
        } => {
            let mut params = ListDocumentsParams::new().flagged(flagged);
            params.status = status.map(|s| s.to_sdk());
            params.q = search;
            params.category = category;
            let rows = fetch(&client, params, &paging).await?;
            out.list(
                &rows,
                &["id", "supplier", "invoice", "total", "status", "added"],
                row,
                "No invoices match.",
            )
        }
        DocumentsCmd::Get { id } => {
            let id = ids::document(&client, &id).await?;
            let detail = client.get_document(&id).await?;
            out.one(&detail, || card(&detail))
        }
        DocumentsCmd::Download { id, out: path } => {
            let id = ids::document(&client, &id).await?;
            // The original file name is needed only when no --out is given.
            let name = match path {
                Some(_) => String::new(),
                None => client.get_document(&id).await?.document.original_filename,
            };
            let file = client.get_document_file(&id).await?;
            save(&file.bytes, path, &name, out)
        }
    }
}

/// One page, or every page with `--all`.
async fn fetch(
    client: &Client,
    params: ListDocumentsParams,
    paging: &Paging,
) -> Result<Vec<DocumentListItem>> {
    if !paging.all {
        return Ok(client.list_documents(params.limit(paging.limit)).await?);
    }
    let mut all = Vec::new();
    let mut offset = 0;
    loop {
        let page = client
            .list_documents(params.clone().limit(PAGE).offset(offset))
            .await?;
        let done = (page.len() as u32) < PAGE;
        all.extend(page);
        if done {
            return Ok(all);
        }
        offset += PAGE;
    }
}

fn row(d: &DocumentListItem) -> Vec<String> {
    let status = if d.flagged {
        styled(WARN, "to check")
    } else {
        let label = status_label(&d.status);
        match label {
            "read" => styled(GOOD, label),
            "couldn't read" => styled(BAD, label),
            _ => styled(DIM, label),
        }
    };
    vec![
        styled(DIM, short_id(&d.id)),
        d.vendor_name
            .clone()
            .unwrap_or_else(|| d.original_filename.clone()),
        d.invoice_number.clone().unwrap_or_default(),
        money(d.total_cents, d.currency.as_deref()),
        status,
        day(&d.created_at),
    ]
}

/// Everything Invoist read from one invoice, laid out for a person.
pub fn card(detail: &DocumentDetail) -> String {
    let mut s = String::new();
    let doc = &detail.document;
    let Some(e) = &detail.extraction else {
        s.push_str(&format!(
            "{}\n{}\n",
            styled(BOLD, &doc.original_filename),
            styled(DIM, "Not read yet.")
        ));
        return s;
    };
    let vendor = e
        .vendor_name
        .clone()
        .unwrap_or_else(|| doc.original_filename.clone());
    s.push_str(&format!(
        "{}  {}\n",
        styled(BOLD, &vendor),
        styled(DIM, status_label(&doc.status))
    ));
    if e.status == ExtractionStatus::Failed {
        s.push_str(&format!(
            "{}\n",
            styled(
                BAD,
                e.error_message
                    .clone()
                    .unwrap_or_else(|| "Invoist couldn't read this file.".into())
            )
        ));
    }

    let mut facts = Vec::new();
    if let Some(n) = &e.invoice_number {
        facts.push(format!("Invoice {n}"));
    }
    if let Some(d) = &e.invoice_date {
        facts.push(format!("issued {d}"));
    }
    if let Some(d) = &e.due_date {
        facts.push(format!("due {d}"));
    }
    if !facts.is_empty() {
        s.push_str(&format!("{}\n", facts.join(" · ")));
    }
    let cur = e.currency.as_deref();
    s.push_str(&format!(
        "\n  {}  {}\n",
        styled(DIM, "Total    "),
        styled(BOLD, money(e.total_cents, cur))
    ));
    if e.subtotal_cents.is_some() {
        s.push_str(&format!(
            "  {}  {}\n",
            styled(DIM, "Subtotal "),
            money(e.subtotal_cents, cur)
        ));
    }
    if e.tax_cents.is_some() {
        s.push_str(&format!(
            "  {}  {}\n",
            styled(DIM, "Tax      "),
            money(e.tax_cents, cur)
        ));
    }
    for (label, value) in [
        ("Reference", &e.reference_number),
        ("PO number", &e.po_number),
        ("Customer", &e.customer_name),
    ] {
        if let Some(v) = value {
            s.push_str(&format!("  {}  {v}\n", styled(DIM, format!("{label:<9}"))));
        }
    }

    if let Some(lines) = e.line_items.as_ref().filter(|l| !l.is_empty()) {
        s.push_str(&format!("\n{}\n", styled(DIM, "LINES")));
        let width = lines
            .iter()
            .map(|l| l.description.chars().count())
            .max()
            .unwrap_or(0)
            .min(48);
        // "40 × $21.50" beside the line's total, when the invoice gives both.
        let each: Vec<String> = lines
            .iter()
            .map(|l| match l.unit_price_cents {
                Some(price) if l.quantity != 1.0 && l.quantity != 0.0 => {
                    format!("{} × {}", trim_float(l.quantity), money(Some(price), cur))
                }
                _ if l.quantity != 1.0 && l.quantity != 0.0 => {
                    format!("{} ×", trim_float(l.quantity))
                }
                _ => String::new(),
            })
            .collect();
        let each_width = each.iter().map(|e| e.chars().count()).max().unwrap_or(0);
        let totals: Vec<String> = lines
            .iter()
            .map(|l| money(l.line_total_cents, cur))
            .collect();
        let total_width = totals.iter().map(|t| t.chars().count()).max().unwrap_or(0);
        for ((l, each), total) in lines.iter().zip(&each).zip(&totals) {
            let description: String = l.description.chars().take(48).collect();
            s.push_str(&format!(
                "  {description:<width$}  {}  {total:>total_width$}\n",
                styled(DIM, format!("{each:>each_width$}")),
            ));
        }
    }

    let open: Vec<_> = detail
        .review_queue
        .iter()
        .filter(|i| i.status == ReviewStatus::Pending)
        .collect();
    if !open.is_empty() {
        s.push_str(&format!(
            "\n{}\n",
            styled(WARN, "SET ASIDE FOR A PERSON TO CHECK")
        ));
        for item in open {
            s.push_str(&format!(
                "  {} {}  {}\n",
                styled(WARN, "!"),
                reason_label(&item.reason),
                styled(DIM, short_id(&item.id))
            ));
        }
    }
    if let Some(flags) = e.fraud_indicators.as_ref().filter(|f| !f.is_empty()) {
        s.push_str(&format!("\n{}\n", styled(DIM, "WARNING SIGNS")));
        for f in flags {
            s.push_str(&format!("  {} {}\n", styled(WARN, "•"), f.description));
        }
    }
    s.push_str(&format!(
        "\n{}\n",
        styled(
            DIM,
            format!(
                "{}  ·  {}  ·  added {}",
                doc.id,
                doc.original_filename,
                day(&doc.created_at)
            )
        )
    ));
    s
}

fn trim_float(n: f64) -> String {
    let s = format!("{n:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Saves a downloaded file: to `path`, to standard output with `-`, or
/// under its original name in the current folder. Never overwrites a file
/// that's already there.
pub fn save(bytes: &[u8], path: Option<PathBuf>, original: &str, out: Out) -> Result<()> {
    if path.as_deref() == Some(Path::new("-")) {
        use std::io::Write;
        return std::io::stdout()
            .write_all(bytes)
            .map_err(|e| CliError::io("couldn't write to standard output", e));
    }
    let target = path.unwrap_or_else(|| {
        // Only the file name, never a path the API sent.
        PathBuf::from(
            Path::new(original)
                .file_name()
                .map(|n| n.to_owned())
                .unwrap_or_else(|| "invoice".into()),
        )
    });
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(&target).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            CliError::Usage(format!(
                "{} already exists; choose another name with --out",
                target.display()
            ))
        } else {
            CliError::io(format!("couldn't save {}", target.display()), e)
        }
    })?;
    std::io::Write::write_all(&mut file, bytes)
        .map_err(|e| CliError::io(format!("couldn't save {}", target.display()), e))?;
    out.note(format!(
        "{} Saved {} ({} KB)",
        styled(GOOD, "✓"),
        target.display(),
        bytes.len().div_ceil(1024)
    ));
    Ok(())
}

fn valid_day(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && value
            .chars()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        && (1..=12).contains(&value[5..7].parse::<u8>().unwrap_or(0))
        && (1..=31).contains(&value[8..10].parse::<u8>().unwrap_or(0))
}

/// One row of an export: the list's fields, plus the details when asked.
#[derive(Debug, Serialize)]
struct ExportRow {
    #[serde(flatten)]
    item: DocumentListItem,
    #[serde(skip_serializing_if = "Option::is_none")]
    invoice_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subtotal_cents: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tax_cents: Option<i64>,
}

pub async fn export(
    profile: Option<&str>,
    base_url: Option<&str>,
    args: ExportArgs,
    out: Out,
) -> Result<()> {
    for value in [&args.since, &args.until].into_iter().flatten() {
        if !valid_day(value) {
            return Err(CliError::Usage(format!(
                "`{value}` isn't a date: use YYYY-MM-DD"
            )));
        }
    }
    // An export is for a file or a spreadsheet, so a table isn't offered.
    let out = Out {
        format: if out.format == Format::Table {
            Format::Csv
        } else {
            out.format
        },
        ..out
    };
    let mut session = Session::open(profile, base_url)?;
    let client = session.client().await?;
    let mut params = ListDocumentsParams::new();
    params.status = args.status.map(|s| s.to_sdk());
    let items: Vec<DocumentListItem> = fetch(
        &client,
        params,
        &Paging {
            limit: PAGE,
            all: true,
        },
    )
    .await?
    .into_iter()
    .filter(|d| {
        let added = day(&d.created_at);
        args.since
            .as_ref()
            .is_none_or(|s| added.as_str() >= s.as_str())
            && args
                .until
                .as_ref()
                .is_none_or(|u| added.as_str() <= u.as_str())
    })
    .collect();

    let mut rows: Vec<ExportRow> = items
        .into_iter()
        .map(|item| ExportRow {
            item,
            invoice_date: None,
            due_date: None,
            subtotal_cents: None,
            tax_cents: None,
        })
        .collect();
    if args.details && !rows.is_empty() {
        out.note(styled(
            DIM,
            format!("Fetching the details of {} invoices…", rows.len()),
        ));
        let gate = Arc::new(Semaphore::new(6));
        let mut tasks = tokio::task::JoinSet::new();
        for (index, row) in rows.iter().enumerate() {
            let (client, gate, id) = (client.clone(), gate.clone(), row.item.id.clone());
            tasks.spawn(async move {
                let _permit = gate.acquire_owned().await;
                (index, client.get_document(&id).await)
            });
        }
        while let Some(done) = tasks.join_next().await {
            let (index, detail) =
                done.map_err(|e| CliError::Failed(format!("a request stopped unexpectedly: {e}")))?;
            if let Some(e) = detail?.extraction {
                rows[index].invoice_date = e.invoice_date;
                rows[index].due_date = e.due_date;
                rows[index].subtotal_cents = e.subtotal_cents;
                rows[index].tax_cents = e.tax_cents;
            }
        }
    }

    let mut columns = vec![
        "id",
        "file",
        "supplier",
        "invoice_number",
        "reference",
        "total",
        "currency",
        "category",
        "status",
        "to_check",
        "added",
    ];
    if args.details {
        columns.extend(["invoice_date", "due_date", "subtotal", "tax"]);
    }
    out.list(
        &rows,
        &columns,
        |r| {
            let d = &r.item;
            let mut cells = vec![
                d.id.clone(),
                d.original_filename.clone(),
                d.vendor_name.clone().unwrap_or_default(),
                d.invoice_number.clone().unwrap_or_default(),
                d.reference_number.clone().unwrap_or_default(),
                d.total_cents.map(output::decimal).unwrap_or_default(),
                d.currency.clone().unwrap_or_default(),
                d.category_name.clone().unwrap_or_default(),
                d.status.as_str().to_string(),
                d.flagged.to_string(),
                day(&d.created_at),
            ];
            if args.details {
                cells.extend([
                    r.invoice_date.clone().unwrap_or_default(),
                    r.due_date.clone().unwrap_or_default(),
                    r.subtotal_cents.map(output::decimal).unwrap_or_default(),
                    r.tax_cents.map(output::decimal).unwrap_or_default(),
                ]);
            }
            cells
        },
        "",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_must_be_real_days() {
        assert!(valid_day("2026-11-03"));
        assert!(!valid_day("2026-13-01"));
        assert!(!valid_day("03/11/2026"));
        assert!(!valid_day("2026-1-3"));
    }

    #[test]
    fn quantities_drop_trailing_zeros() {
        assert_eq!(trim_float(40.0), "40");
        assert_eq!(trim_float(2.5), "2.5");
    }
}
