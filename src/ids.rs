//! Ids can be typed in full, or shortened: the 8 characters tables show (the
//! end of the id), or the start of it. A full id is used as it is; a short
//! one is matched against the newest items and must match exactly one.

use invoist::{Client, ListBillsParams, ListDocumentsParams, ListReviewQueueParams};

use crate::error::{CliError, Result};

/// How far back a short id is looked for.
const SEARCH_DEPTH: u32 = 1000;
const PAGE: u32 = 200;

/// A UUID is 36 characters; anything that long is taken as a full id.
fn is_full(id: &str) -> bool {
    id.len() >= 36
}

fn check_prefix(id: &str) -> Result<String> {
    let id = id.trim().to_lowercase();
    if id.len() < 4 {
        return Err(CliError::Usage(
            "give at least 4 characters of the id".into(),
        ));
    }
    if !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(CliError::Usage(format!(
            "`{id}` isn't an id: ids are letters a–f, digits and dashes"
        )));
    }
    Ok(id)
}

/// Whether a shortened id is the start or the end of a full one.
fn is_short_for(full: &str, short: &str) -> bool {
    full.starts_with(short) || full.ends_with(short)
}

/// Picks the one match, or explains why there isn't one.
fn one(prefix: &str, what: &str, matches: Vec<String>) -> Result<String> {
    match matches.len() {
        1 => Ok(matches.into_iter().next().unwrap_or_default()),
        0 => Err(CliError::NotFound(format!(
            "no {what} has an id that starts or ends with `{prefix}`"
        ))),
        n => Err(CliError::Usage(format!(
            "{n} {what}s have ids that start or end with `{prefix}`: type more of the id"
        ))),
    }
}

pub async fn document(client: &Client, id: &str) -> Result<String> {
    if is_full(id) {
        return Ok(id.to_string());
    }
    let prefix = check_prefix(id)?;
    let mut matches = Vec::new();
    let mut offset = 0;
    while offset < SEARCH_DEPTH {
        let page = client
            .list_documents(ListDocumentsParams::new().limit(PAGE).offset(offset))
            .await?;
        matches.extend(
            page.iter()
                .filter(|d| is_short_for(&d.id, &prefix))
                .map(|d| d.id.clone()),
        );
        if (page.len() as u32) < PAGE {
            break;
        }
        offset += PAGE;
    }
    one(&prefix, "invoice", matches)
}

pub async fn bill(client: &Client, id: &str) -> Result<String> {
    if is_full(id) {
        return Ok(id.to_string());
    }
    let prefix = check_prefix(id)?;
    let mut matches = Vec::new();
    let mut offset = 0;
    while offset < SEARCH_DEPTH {
        let page = client
            .bills()
            .list(ListBillsParams::new().limit(PAGE).offset(offset))
            .await?;
        matches.extend(
            page.iter()
                .filter(|b| is_short_for(&b.id, &prefix))
                .map(|b| b.id.clone()),
        );
        if (page.len() as u32) < PAGE {
            break;
        }
        offset += PAGE;
    }
    one(&prefix, "bill", matches)
}

pub async fn review_item(client: &Client, id: &str) -> Result<String> {
    if is_full(id) {
        return Ok(id.to_string());
    }
    let prefix = check_prefix(id)?;
    let items = client
        .list_review_queue(ListReviewQueueParams::new())
        .await?;
    one(
        &prefix,
        "review item",
        items
            .into_iter()
            .filter(|i| is_short_for(&i.id, &prefix))
            .map(|i| i.id)
            .collect(),
    )
}

pub async fn webhook(client: &Client, id: &str) -> Result<String> {
    if is_full(id) {
        return Ok(id.to_string());
    }
    let prefix = check_prefix(id)?;
    let list = client.list_webhook_endpoints().await?;
    one(
        &prefix,
        "webhook endpoint",
        list.endpoints
            .into_iter()
            .filter(|e| is_short_for(&e.id, &prefix))
            .map(|e| e.id)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_must_be_long_enough_and_look_like_ids() {
        assert!(check_prefix("01a").is_err());
        assert!(check_prefix("zzzz").is_err());
        assert_eq!(check_prefix(" 01A19812 ").unwrap(), "01a19812");
    }

    #[test]
    fn a_short_id_is_the_start_or_the_end() {
        let id = "01a19812-0000-7000-8000-00000a1b2c3d";
        assert!(is_short_for(id, "0a1b2c3d"));
        assert!(is_short_for(id, "01a19812"));
        assert!(!is_short_for(id, "7000"));
    }

    #[test]
    fn exactly_one_match_is_required() {
        assert_eq!(one("01a1", "bill", vec!["01a1x".into()]).unwrap(), "01a1x");
        assert!(matches!(
            one("01a1", "bill", vec![]),
            Err(CliError::NotFound(_))
        ));
        assert!(matches!(
            one("01a1", "bill", vec!["a".into(), "b".into()]),
            Err(CliError::Usage(_))
        ));
    }
}
