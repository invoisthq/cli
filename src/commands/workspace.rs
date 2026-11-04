//! `invoist categories`, `usage` and `stats`.

use crate::error::Result;
use crate::output::{BOLD, DIM, Out, bars, capitalise, count, day, money, short_id, styled};
use crate::session::Session;

pub async fn categories(profile: Option<&str>, base_url: Option<&str>, out: Out) -> Result<()> {
    let mut session = Session::open(profile, base_url)?;
    let list = session.client().await?.list_categories().await?;
    out.list(
        &list,
        &["id", "name", "default"],
        |c| {
            vec![
                styled(DIM, short_id(&c.id)),
                c.name.clone(),
                if c.is_default {
                    "yes".into()
                } else {
                    String::new()
                },
            ]
        },
        "No categories yet.",
    )
}

pub async fn usage(profile: Option<&str>, base_url: Option<&str>, out: Out) -> Result<()> {
    let mut session = Session::open(profile, base_url)?;
    let usage = session.client().await?.get_usage().await?;
    out.one(&usage, || {
        let mut s = format!(
            "{}\n",
            styled(DIM, format!("Since {}", day(&usage.period_start)))
        );
        s.push_str(&format!(
            "  {}  {}\n",
            styled(DIM, "Invoices read"),
            styled(BOLD, count(usage.total_documents))
        ));
        s.push_str(&format!(
            "  {}  {}\n",
            styled(DIM, "API calls    "),
            count(usage.total_calls)
        ));
        s.push_str(&format!(
            "  {}  {}\n",
            styled(DIM, "Cost         "),
            money(Some(usage.total_cost_cents), Some("USD"))
        ));
        let days: Vec<_> = usage
            .by_day
            .iter()
            .filter(|d| d.documents.unwrap_or(0) > 0)
            .collect();
        if !days.is_empty() {
            s.push_str(&format!("\n{}\n", styled(DIM, "BY DAY")));
            let rows: Vec<(String, String, i64)> = days
                .iter()
                .map(|d| {
                    let n = d.documents.unwrap_or(0);
                    (d.day.as_deref().map(day).unwrap_or_default(), count(n), n)
                })
                .collect();
            s.push_str(&bars(&rows, 30));
        }
        s
    })
}

pub async fn stats(profile: Option<&str>, base_url: Option<&str>, out: Out) -> Result<()> {
    let mut session = Session::open(profile, base_url)?;
    let stats = session.client().await?.get_stats().await?;
    out.one(&stats, || {
        let mut s = String::new();
        let included = stats
            .documents_included
            .map(|n| format!(" of {n} included"))
            .unwrap_or_default();
        s.push_str(&format!(
            "  {}  {}{}\n",
            styled(DIM, "This month     "),
            styled(BOLD, count(stats.documents_this_month)),
            styled(DIM, included)
        ));
        s.push_str(&format!(
            "  {}  {}\n",
            styled(DIM, "All time       "),
            count(stats.documents_total)
        ));
        s.push_str(&format!(
            "  {}  {}\n",
            styled(DIM, "To check       "),
            count(stats.pending_review_count)
        ));
        s.push_str(&format!(
            "  {}  {}\n",
            styled(DIM, "Plan           "),
            capitalise(stats.plan.as_str())
        ));
        s
    })
}
