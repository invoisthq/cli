//! `invoist bills …` and `invoist held-emails`: a household's side.

use std::str::FromStr;

use invoist::models::{Bill, BillDetail, BillsStats, BillsSummary, ReviewStatus};
use invoist::{BillsStatsParams, BillsSummaryParams, Client, ListBillsParams, Month};

use crate::cli::{BillsArgs, BillsCmd, Paging};
use crate::commands::{confirm, documents};
use crate::error::{CliError, Result};
use crate::ids;
use crate::output::{BOLD, DIM, GOOD, Out, WARN, bars, day, money, reason_label, short_id, styled};
use crate::session::Session;

const PAGE: u32 = 200;

fn month(value: Option<String>) -> Result<Option<Month>> {
    value
        .map(|m| {
            Month::from_str(&m).map_err(|_| {
                CliError::Usage(format!("`{m}` isn't a month: use YYYY-MM, such as 2026-11"))
            })
        })
        .transpose()
}

pub async fn run(
    profile: Option<&str>,
    base_url: Option<&str>,
    args: BillsArgs,
    out: Out,
) -> Result<()> {
    let mut session = Session::open(profile, base_url)?;
    let client = session.client().await?;
    let Some(cmd) = args.command else {
        let summary = client
            .bills()
            .summary(BillsSummaryParams::new().days(args.days))
            .await?;
        let unpaid = client
            .bills()
            .list(ListBillsParams::new().paid(false).limit(PAGE))
            .await?;
        return out.one(&summary, || summary_text(&summary, &unpaid));
    };
    match cmd {
        BillsCmd::List {
            month: m,
            unpaid,
            paid,
            sender,
            search,
            paging,
        } => {
            let mut params = ListBillsParams::new();
            params.month = month(m)?;
            params.paid = if unpaid {
                Some(false)
            } else if paid {
                Some(true)
            } else {
                None
            };
            params.sender = sender;
            params.q = search;
            let rows = fetch(&client, params, &paging).await?;
            out.list(
                &rows,
                &["id", "from", "amount", "due", "paid"],
                row,
                "No bills match.",
            )
        }
        BillsCmd::Get { id } => {
            let id = ids::bill(&client, &id).await?;
            let detail = client.bills().get(&id).await?;
            out.one(&detail, || card(&detail))
        }
        BillsCmd::Download { id, out: path } => {
            let id = ids::bill(&client, &id).await?;
            // The original file name is needed only when no --out is given.
            let name = match path {
                Some(_) => String::new(),
                None => client.bills().get(&id).await?.document.original_filename,
            };
            let file = client.bills().get_file(&id).await?;
            documents::save(&file.bytes, path, &name, out)
        }
        BillsCmd::Paid { id, yes } => mark(&client, &id, true, yes, out).await,
        BillsCmd::Unpaid { id, yes } => mark(&client, &id, false, yes, out).await,
        BillsCmd::Stats { month: m, sender } => {
            let mut params = BillsStatsParams::new();
            params.month = month(m)?;
            params.sender = sender;
            let stats = client.bills().stats(params).await?;
            out.one(&stats, || stats_text(&stats))
        }
    }
}

async fn fetch(client: &Client, params: ListBillsParams, paging: &Paging) -> Result<Vec<Bill>> {
    if !paging.all {
        return Ok(client.bills().list(params.limit(paging.limit)).await?);
    }
    let mut all = Vec::new();
    let mut offset = 0;
    loop {
        let page = client
            .bills()
            .list(params.clone().limit(PAGE).offset(offset))
            .await?;
        let done = (page.len() as u32) < PAGE;
        all.extend(page);
        if done {
            return Ok(all);
        }
        offset += PAGE;
    }
}

fn row(b: &Bill) -> Vec<String> {
    vec![
        styled(DIM, short_id(&b.id)),
        b.vendor_name
            .clone()
            .unwrap_or_else(|| b.original_filename.clone()),
        money(b.total_cents, b.currency.as_deref()),
        b.due_date.clone().unwrap_or_default(),
        match &b.bill.paid_at {
            Some(at) => styled(GOOD, format!("paid {}", day(at))),
            None => styled(WARN, "to pay"),
        },
    ]
}

fn summary_text(summary: &BillsSummary, unpaid: &[Bill]) -> String {
    let mut s = String::new();
    if summary.unpaid == 0 {
        return format!("{} Nothing left to pay.\n", styled(GOOD, "✓"));
    }
    let due: Vec<String> = summary
        .due_soon
        .iter()
        .map(|d| money(Some(d.total_cents), Some(&d.currency)))
        .collect();
    s.push_str(&format!(
        "{} unpaid {}",
        styled(BOLD, summary.unpaid),
        if summary.unpaid == 1 { "bill" } else { "bills" }
    ));
    if summary.overdue > 0 {
        s.push_str(&format!(
            ", {}",
            styled(WARN, format!("{} overdue", summary.overdue))
        ));
    }
    s.push('\n');
    if !due.is_empty() {
        s.push_str(&format!(
            "{} due in the next {} days\n",
            styled(BOLD, due.join(" + ")),
            summary.days
        ));
    }
    let mut upcoming: Vec<&Bill> = unpaid.iter().collect();
    upcoming.sort_by(|a, b| {
        a.due_date
            .as_deref()
            .unwrap_or("9999")
            .cmp(b.due_date.as_deref().unwrap_or("9999"))
    });
    if !upcoming.is_empty() {
        s.push('\n');
        for b in upcoming.iter().take(10) {
            let late = b
                .due_date
                .as_deref()
                .is_some_and(|d| d < summary.today.as_str());
            let due = b.due_date.clone().unwrap_or_else(|| "no due date".into());
            s.push_str(&format!(
                "  {}  {:<28} {:>12}  {}\n",
                styled(DIM, short_id(&b.id)),
                b.vendor_name
                    .clone()
                    .unwrap_or_else(|| b.original_filename.clone())
                    .chars()
                    .take(28)
                    .collect::<String>(),
                money(b.total_cents, b.currency.as_deref()),
                if late {
                    styled(WARN, format!("{due}  overdue"))
                } else {
                    due
                }
            ));
        }
        if upcoming.len() > 10 {
            s.push_str(&styled(
                DIM,
                format!(
                    "  and {} more: `invoist bills list --unpaid`\n",
                    upcoming.len() - 10
                ),
            ));
        }
    }
    s
}

fn card(detail: &BillDetail) -> String {
    let doc = &detail.document;
    let e = detail.extraction.as_ref();
    let mut s = String::new();
    let vendor = e
        .and_then(|e| e.vendor_name.clone())
        .unwrap_or_else(|| doc.original_filename.clone());
    let paid = match &detail.bill.paid_at {
        Some(at) => styled(GOOD, format!("paid {}", day(at))),
        None => styled(WARN, "to pay"),
    };
    s.push_str(&format!("{}  {paid}\n", styled(BOLD, &vendor)));
    if let Some(e) = e {
        let cur = e.currency.as_deref();
        s.push_str(&format!(
            "\n  {}  {}\n",
            styled(DIM, "Amount   "),
            styled(BOLD, money(e.total_cents, cur))
        ));
        if let Some(d) = &e.due_date {
            s.push_str(&format!("  {}  {d}\n", styled(DIM, "Due      ")));
        }
        if let Some(r) = &e.reference_number {
            s.push_str(&format!("  {}  {r}\n", styled(DIM, "Reference")));
        }
        if let Some(n) = &e.invoice_number {
            s.push_str(&format!("  {}  {n}\n", styled(DIM, "Bill no. ")));
        }
    }
    // Where to pay: the chain entry for this document carries the account.
    if let Some(member) = detail
        .chain
        .as_ref()
        .and_then(|c| c.members.iter().find(|m| m.document_id == doc.id))
    {
        for (label, value) in [
            ("IBAN     ", &member.vendor_iban),
            ("BIC      ", &member.vendor_bic),
            ("Account  ", &member.payment_account),
        ] {
            if let Some(v) = value {
                s.push_str(&format!("  {}  {v}\n", styled(DIM, label)));
            }
        }
    }
    let open: Vec<_> = detail
        .review_queue
        .iter()
        .filter(|i| i.status == ReviewStatus::Pending)
        .collect();
    if !open.is_empty() {
        s.push_str(&format!("\n{}\n", styled(WARN, "CHECK BEFORE PAYING")));
        for item in open {
            s.push_str(&format!(
                "  {} {}\n",
                styled(WARN, "!"),
                reason_label(&item.reason)
            ));
        }
    }
    s.push_str(&format!(
        "\n{}\n",
        styled(DIM, format!("{}  ·  {}", doc.id, doc.original_filename))
    ));
    s
}

async fn mark(client: &Client, id: &str, paid: bool, yes: bool, out: Out) -> Result<()> {
    let id = ids::bill(client, id).await?;
    let detail = client.bills().get(&id).await?;
    let vendor = detail
        .extraction
        .as_ref()
        .and_then(|e| e.vendor_name.clone())
        .unwrap_or_else(|| detail.document.original_filename.clone());
    let amount = detail
        .extraction
        .as_ref()
        .map(|e| money(e.total_cents, e.currency.as_deref()))
        .unwrap_or_default();
    let question = if paid {
        format!("Mark the {vendor} bill ({amount}) as paid?")
    } else {
        format!("Mark the {vendor} bill ({amount}) as not paid?")
    };
    if !confirm(&question, yes)? {
        out.note("Nothing changed.");
        return Ok(());
    }
    let record = if paid {
        client.bills().mark_paid(&id).await?
    } else {
        client.bills().mark_unpaid(&id).await?
    };
    out.one(&record, || {
        format!(
            "{} {vendor} bill {}\n",
            styled(GOOD, "✓"),
            if paid {
                "marked as paid"
            } else {
                "marked as not paid"
            }
        )
    })
}

fn stats_text(stats: &BillsStats) -> String {
    let mut s = format!("{}\n", styled(BOLD, format!("Bills for {}", stats.month)));
    for t in &stats.totals {
        s.push_str(&format!(
            "  {} in {} {}, {} paid ({} of {})\n",
            money(Some(t.total_cents), Some(&t.currency)),
            t.count,
            if t.count == 1 { "bill" } else { "bills" },
            money(Some(t.paid_cents), Some(&t.currency)),
            t.paid_count,
            t.count
        ));
    }
    if !stats.trend.is_empty() {
        s.push_str(&format!("\n{}\n", styled(DIM, "BY MONTH")));
        let rows: Vec<(String, String, i64)> = stats
            .trend
            .iter()
            .map(|t| {
                (
                    t.month.clone(),
                    money(Some(t.total_cents), Some(&t.currency)),
                    t.total_cents,
                )
            })
            .collect();
        s.push_str(&bars(&rows, 30));
    }
    if !stats.top_senders.is_empty() {
        s.push_str(&format!("\n{}\n", styled(DIM, "BIGGEST SENDERS")));
        let rows: Vec<(String, String, i64)> = stats
            .top_senders
            .iter()
            .map(|t| {
                (
                    t.name.clone(),
                    money(t.total_cents, t.currency.as_deref()),
                    t.total_cents.unwrap_or(0),
                )
            })
            .collect();
        s.push_str(&bars(&rows, 20));
    }
    s
}

pub async fn held_emails(profile: Option<&str>, base_url: Option<&str>, out: Out) -> Result<()> {
    let mut session = Session::open(profile, base_url)?;
    let list = session.client().await?.held_emails().list().await?;
    out.list(
        &list,
        &["id", "from", "subject", "files", "why", "kept until"],
        |m| {
            vec![
                styled(DIM, short_id(&m.id)),
                m.from_email.clone(),
                m.subject.chars().take(40).collect(),
                m.files.len().to_string(),
                m.hold_reason
                    .as_ref()
                    .map(|r| match r.as_str() {
                        "new_sender" => "new sender".to_string(),
                        "unverified_sender" => "sender not verified".into(),
                        "no_plan" => "no active plan".into(),
                        "quota_used" => "plan's invoices used up".into(),
                        other => other.replace('_', " "),
                    })
                    .unwrap_or_default(),
                m.held_until.as_deref().map(day).unwrap_or_default(),
            ]
        },
        "No emails are waiting for approval.",
    )?;
    if !list.is_empty() {
        out.note(styled(DIM, "Approve or block senders in the Invoist app."));
    }
    Ok(())
}
