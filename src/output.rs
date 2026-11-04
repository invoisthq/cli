//! How results are shown: readable tables and cards on a terminal, and
//! stable JSON, NDJSON or CSV for scripts. Data goes to stdout; messages go
//! to stderr, so piping a command never mixes the two.
//!
//! `--json` prints the SDK's own models unchanged, so a script's view of a
//! document never changes when a table's layout does.

use std::io::{IsTerminal, Write};

use anstyle::{AnsiColor, Style};
use comfy_table::{Cell, ContentArrangement, Table, presets};
use invoist::models::{DocumentStatus, ReviewReason};
use serde::Serialize;

use crate::error::{CliError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    /// A readable table (the default on a terminal).
    Table,
    /// One JSON document.
    Json,
    /// One JSON object per line, for long lists.
    Ndjson,
    /// Comma-separated values, with a header row.
    Csv,
}

/// The output settings every command shares.
#[derive(Debug, Clone, Copy)]
pub struct Out {
    pub format: Format,
    pub quiet: bool,
}

pub const BOLD: Style = Style::new().bold();
pub const DIM: Style = Style::new().dimmed();
pub const ACCENT: Style = AnsiColor::Blue.on_default().bold();
pub const GOOD: Style = AnsiColor::Green.on_default();
pub const WARN: Style = AnsiColor::Yellow.on_default();
pub const BAD: Style = AnsiColor::Red.on_default();

/// Whether colour should be used: never with `--no-color`, `NO_COLOR` or
/// without a terminal; always with `CLICOLOR_FORCE`.
pub fn init_color(no_color: bool) {
    let choice = if no_color {
        anstream::ColorChoice::Never
    } else {
        anstream::ColorChoice::Auto
    };
    choice.write_global();
}

/// Whether the CLI may ask a question or draw progress: a terminal on both
/// ends, and not CI.
pub fn interactive() -> bool {
    std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
        && std::env::var_os("CI").is_none()
}

impl Out {
    /// A line for a person, on stderr. Silent with `--quiet`, and when the
    /// output is meant for a program.
    pub fn note(&self, line: impl std::fmt::Display) {
        if !self.quiet {
            let _ = writeln!(anstream::stderr(), "{line}");
        }
    }

    pub fn is_machine(&self) -> bool {
        !matches!(self.format, Format::Table)
    }

    /// Prints one value: as JSON for a program, or with `human` for a person.
    pub fn one<T: Serialize>(&self, value: &T, human: impl FnOnce() -> String) -> Result<()> {
        match self.format {
            Format::Json | Format::Ndjson | Format::Csv => {
                print_json(value, self.format == Format::Json)
            }
            Format::Table => {
                let _ = write!(anstream::stdout(), "{}", human());
                Ok(())
            }
        }
    }

    /// Prints a list in the chosen format. `columns` names the table and CSV
    /// columns; `row` gives one row's cells in the same order.
    pub fn list<T: Serialize>(
        &self,
        rows: &[T],
        columns: &[&str],
        row: impl Fn(&T) -> Vec<String>,
        empty: &str,
    ) -> Result<()> {
        match self.format {
            Format::Json => print_json(&rows, true),
            Format::Ndjson => {
                let mut out = std::io::stdout().lock();
                for r in rows {
                    let line =
                        serde_json::to_string(r).map_err(|e| CliError::Failed(e.to_string()))?;
                    writeln!(out, "{line}")
                        .map_err(|e| CliError::io("couldn't write output", e))?;
                }
                Ok(())
            }
            Format::Csv => {
                let mut out = std::io::stdout().lock();
                writeln!(
                    out,
                    "{}",
                    columns
                        .iter()
                        .map(|c| csv_field(c))
                        .collect::<Vec<_>>()
                        .join(",")
                )
                .map_err(|e| CliError::io("couldn't write output", e))?;
                for r in rows {
                    let cells = row(r);
                    writeln!(
                        out,
                        "{}",
                        cells
                            .iter()
                            .map(|c| csv_field(&strip_styles(c)))
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                    .map_err(|e| CliError::io("couldn't write output", e))?;
                }
                Ok(())
            }
            Format::Table => {
                if rows.is_empty() {
                    self.note(format!("{DIM}{empty}{DIM:#}"));
                    return Ok(());
                }
                let mut table = Table::new();
                table
                    .load_style(presets::NOTHING)
                    .set_content_arrangement(ContentArrangement::Dynamic)
                    .set_header(columns.iter().map(|c| {
                        Cell::new(c.to_uppercase()).add_attribute(comfy_table::Attribute::Dim)
                    }));
                let cells: Vec<Vec<String>> = rows.iter().map(&row).collect();
                let numeric: Vec<bool> = (0..columns.len())
                    .map(|i| {
                        !columns[i].eq_ignore_ascii_case("id")
                            && cells
                                .iter()
                                .any(|r| r.get(i).is_some_and(|c| !strip_styles(c).is_empty()))
                            && cells
                                .iter()
                                .all(|r| r.get(i).is_none_or(|c| is_amount(&strip_styles(c))))
                    })
                    .collect();
                for r in cells {
                    table.add_row(r);
                }
                // Numbers and amounts line up on the right.
                for (column, numeric) in table.column_iter_mut().zip(&numeric) {
                    if *numeric {
                        column.set_cell_alignment(comfy_table::CellAlignment::Right);
                    }
                }
                // Flush left with two spaces between columns, like other
                // command-line tools, and no spaces at the ends of lines.
                for column in table.column_iter_mut() {
                    column.set_padding((0, 2));
                }
                let mut out = anstream::stdout();
                for line in table.lines() {
                    writeln!(out, "{}", line.trim_end())
                        .map_err(|e| CliError::io("couldn't write output", e))?;
                }
                Ok(())
            }
        }
    }
}

pub fn print_json<T: Serialize + ?Sized>(value: &T, pretty: bool) -> Result<()> {
    let text = if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    }
    .map_err(|e| CliError::Failed(e.to_string()))?;
    writeln!(std::io::stdout(), "{text}").map_err(|e| CliError::io("couldn't write output", e))
}

/// A word with its first letter in capitals: `starter` to `Starter`.
pub fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// A small bar chart, one row per `(label, value, size)`: the label, the
/// value lined up on the right, then a bar as long as `size` is against the
/// largest. A row with a size above zero always gets at least one block.
pub fn bars(rows: &[(String, String, i64)], width: usize) -> String {
    let max = rows.iter().map(|r| r.2).max().unwrap_or(0).max(1);
    let label_width = rows.iter().map(|r| r.0.chars().count()).max().unwrap_or(0);
    let value_width = rows.iter().map(|r| r.1.chars().count()).max().unwrap_or(0);
    let mut s = String::new();
    for (label, value, size) in rows {
        let blocks = ((*size).max(0) as f64 / max as f64 * width as f64).round() as usize;
        let blocks = blocks.max(usize::from(*size > 0));
        s.push_str(&format!(
            "  {label:<label_width$}  {value:>value_width$}  {}\n",
            "▇".repeat(blocks)
        ));
    }
    s
}

/// Whether a cell is a number or an amount (`1,204`, `$84.20`, `-3`,
/// `1,204.20 SEK`), or empty, so its column can line up on the right.
fn is_amount(cell: &str) -> bool {
    let cell = cell.trim();
    if cell.is_empty() || cell == "—" {
        return true;
    }
    let cell = cell.strip_prefix('-').unwrap_or(cell);
    let cell = cell.trim_start_matches(['$', '€', '£']);
    let number = match cell.rsplit_once(' ') {
        Some((n, code)) if code.len() == 3 && code.chars().all(|c| c.is_ascii_uppercase()) => n,
        _ => cell,
    };
    number.starts_with(|c: char| c.is_ascii_digit())
        && number
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
}

/// RFC 4180: quote a field when it holds a comma, a quote or a line break.
pub fn csv_field(value: &str) -> String {
    // A leading = + - @ makes a spreadsheet run the cell as a formula; a
    // quote in front keeps an invoice's text from doing that.
    let guarded = if value.starts_with(['=', '+', '-', '@']) && value.parse::<f64>().is_err() {
        format!("'{value}")
    } else {
        value.to_string()
    };
    if guarded.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

/// Removes ANSI styling, for CSV cells built with colour.
fn strip_styles(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The last 8 characters of an id, accepted anywhere an id is. Invoist ids
/// are UUIDv7: they start with the time they were made, so invoices added
/// together share their first characters, while the end is random.
pub fn short_id(id: &str) -> String {
    let n = id.chars().count();
    id.chars().skip(n.saturating_sub(8)).collect()
}

/// Money from minor units, with the currency: `$1,204.20`, `€89.20`,
/// `1,204.20 SEK`. Integer arithmetic only, so no cent is ever lost.
pub fn money(cents: Option<i64>, currency: Option<&str>) -> String {
    let Some(cents) = cents else {
        return "—".into();
    };
    let code = currency.unwrap_or("").to_uppercase();
    let negative = cents < 0;
    let abs = cents.unsigned_abs();
    let digits = group_thousands(abs / 100);
    let amount = format!("{digits}.{:02}", abs % 100);
    let sign = if negative { "-" } else { "" };
    match code.as_str() {
        "USD" | "CAD" | "AUD" | "NZD" => format!("{sign}${amount}"),
        "EUR" => format!("{sign}€{amount}"),
        "GBP" => format!("{sign}£{amount}"),
        "" => format!("{sign}{amount}"),
        other => format!("{sign}{amount} {other}"),
    }
}

/// Minor units as a plain decimal for CSV and spreadsheets: `-0.50`, `1204.20`.
pub fn decimal(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.unsigned_abs();
    format!("{sign}{}.{:02}", abs / 100, abs % 100)
}

/// `1312` → `1,312`.
pub fn count(n: i64) -> String {
    let sign = if n < 0 { "-" } else { "" };
    format!("{sign}{}", group_thousands(n.unsigned_abs()))
}

fn group_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `2026-11-03T09:12:44Z` → `2026-11-03`; dates pass through.
pub fn day(timestamp: &str) -> String {
    timestamp.chars().take(10).collect()
}

/// What a document's status means, in plain words.
pub fn status_label(status: &DocumentStatus) -> &'static str {
    match status {
        DocumentStatus::Pending => "waiting",
        DocumentStatus::Processing => "reading",
        DocumentStatus::Done => "read",
        DocumentStatus::Failed => "couldn't read",
        _ => "unknown",
    }
}

/// Why an invoice was set aside for a person, in plain words.
pub fn reason_label(reason: &ReviewReason) -> String {
    match reason {
        ReviewReason::LowConfidence => "not sure about a detail".into(),
        ReviewReason::TotalMismatch => "totals don't add up".into(),
        ReviewReason::PoorQuality => "hard to read".into(),
        ReviewReason::Duplicate => "looks like a duplicate".into(),
        ReviewReason::UnusualVendor => "new or unusual supplier".into(),
        ReviewReason::BankDetailsChanged => "bank details changed".into(),
        ReviewReason::AmountAnomaly => "amount far from usual".into(),
        ReviewReason::AmountOverLimit => "amount over your limit".into(),
        ReviewReason::MissingPoNumber => "no purchase order number".into(),
        other => other.as_str().replace('_', " "),
    }
}

pub fn styled(style: Style, text: impl std::fmt::Display) -> String {
    format!("{style}{text}{style:#}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_keeps_every_cent_and_groups_thousands() {
        assert_eq!(money(Some(120420), Some("usd")), "$1,204.20");
        assert_eq!(money(Some(8920), Some("EUR")), "€89.20");
        assert_eq!(money(Some(-500), Some("GBP")), "-£5.00");
        assert_eq!(money(Some(123456789), Some("SEK")), "1,234,567.89 SEK");
        assert_eq!(money(None, Some("USD")), "—");
        assert_eq!(money(Some(5), None), "0.05");
    }

    #[test]
    fn decimals_keep_the_sign_of_small_amounts() {
        assert_eq!(decimal(-50), "-0.50");
        assert_eq!(decimal(120420), "1204.20");
        assert_eq!(decimal(0), "0.00");
    }

    #[test]
    fn csv_quotes_and_defuses_formulas() {
        assert_eq!(csv_field("Northwind, Ltd"), "\"Northwind, Ltd\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("=HYPERLINK(1)"), "'=HYPERLINK(1)");
        assert_eq!(csv_field("-12.50"), "-12.50");
    }

    #[test]
    fn bars_line_up_their_values_and_scale_to_the_largest() {
        let rows = vec![
            ("2026-09".to_string(), "€412.00".to_string(), 41200),
            ("2026-10".to_string(), "€88.00".to_string(), 8800),
            ("2026-11".to_string(), "€0.00".to_string(), 0),
        ];
        assert_eq!(
            bars(&rows, 10),
            "  2026-09  €412.00  ▇▇▇▇▇▇▇▇▇▇\n  2026-10   €88.00  ▇▇\n  2026-11    €0.00  \n"
        );
    }

    #[test]
    fn amounts_are_told_apart_from_other_cells() {
        for yes in ["1,204", "$84.20", "€170.00", "-3", "1,204.20 SEK", "", "—"] {
            assert!(is_amount(yes), "{yes}");
        }
        for no in ["2026-10-30", "to pay", "5d1fd6cc", "USD", "1 box"] {
            assert!(!is_amount(no), "{no}");
        }
    }

    #[test]
    fn short_ids_and_days() {
        assert_eq!(short_id("01a19812-0000-7000-8000-00000a1b2c3d"), "0a1b2c3d");
        assert_eq!(short_id("abc"), "abc");
        assert_eq!(day("2026-11-03T09:12:44Z"), "2026-11-03");
    }

    #[test]
    fn styles_are_stripped_from_csv_cells() {
        assert_eq!(strip_styles(&styled(GOOD, "read")), "read");
    }
}
