//! `invoist review …`: invoices set aside for a person to check.

use invoist::models::{ReviewQueueListItem, ReviewStatus};
use invoist::{ListReviewQueueParams, ReviewResolution};

use crate::cli::{ReviewCmd, ReviewFilter};
use crate::error::Result;
use crate::ids;
use crate::output::{DIM, GOOD, Out, WARN, day, money, reason_label, short_id, styled};
use crate::session::Session;

pub async fn run(
    profile: Option<&str>,
    base_url: Option<&str>,
    cmd: ReviewCmd,
    out: Out,
) -> Result<()> {
    let mut session = Session::open(profile, base_url)?;
    let client = session.client().await?;
    match cmd {
        ReviewCmd::List { status } => {
            let mut params = ListReviewQueueParams::new();
            params.status = match status {
                ReviewFilter::Pending => Some(ReviewStatus::Pending),
                ReviewFilter::Resolved => Some(ReviewStatus::Resolved),
                ReviewFilter::Dismissed => Some(ReviewStatus::Dismissed),
                ReviewFilter::All => None,
            };
            let items = client.list_review_queue(params).await?;
            out.list(
                &items,
                &["id", "why", "supplier", "invoice", "total", "since"],
                row,
                "Nothing is waiting to be checked.",
            )
        }
        ReviewCmd::Resolve { id, dismiss } => {
            let id = ids::review_item(&client, &id).await?;
            let resolution = if dismiss {
                ReviewResolution::Dismissed
            } else {
                ReviewResolution::Resolved
            };
            let item = client.resolve_review_item(&id, resolution).await?;
            out.one(&item, || {
                format!(
                    "{} {} ({})\n",
                    styled(GOOD, "✓"),
                    if dismiss {
                        "Dismissed: not a real problem"
                    } else {
                        "Marked as checked"
                    },
                    reason_label(&item.reason)
                )
            })
        }
    }
}

fn row(i: &ReviewQueueListItem) -> Vec<String> {
    vec![
        styled(DIM, short_id(&i.id)),
        styled(WARN, reason_label(&i.reason)),
        i.vendor_name
            .clone()
            .unwrap_or_else(|| i.original_filename.clone()),
        i.invoice_number.clone().unwrap_or_default(),
        money(i.total_cents, i.currency.as_deref()),
        day(&i.created_at),
    ]
}
