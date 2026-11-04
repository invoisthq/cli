//! `invoist webhooks …`.

use std::io::Read;
use std::path::Path;

use invoist::CreateWebhookEndpoint;

use crate::cli::WebhooksCmd;
use crate::commands::confirm;
use crate::error::{CliError, Result};
use crate::ids;
use crate::output::{BAD, BOLD, DIM, GOOD, Out, WARN, day, short_id, styled};
use crate::session::Session;

pub async fn run(
    profile: Option<&str>,
    base_url: Option<&str>,
    cmd: WebhooksCmd,
    out: Out,
) -> Result<()> {
    // Verifying a signature is local: no sign-in, no network.
    if let WebhooksCmd::Verify {
        body,
        signature,
        secret,
    } = cmd
    {
        return verify(&body, &signature, &secret, out);
    }
    let mut session = Session::open(profile, base_url)?;
    let client = session.client().await?;
    match cmd {
        WebhooksCmd::List => {
            let list = client.list_webhook_endpoints().await?;
            if out.is_machine() {
                return out.one(&list, String::new);
            }
            out.list(
                &list.endpoints,
                &["id", "url", "events", "state", "since"],
                |e| {
                    let state = if !e.enabled {
                        styled(BAD, "paused")
                    } else if e.consecutive_failures > 0 {
                        styled(
                            WARN,
                            format!("{} failures in a row", e.consecutive_failures),
                        )
                    } else {
                        styled(GOOD, "working")
                    };
                    let events = if e.events.iter().any(|v| v == "*") || e.events.is_empty() {
                        "all".to_string()
                    } else {
                        e.events.join(", ")
                    };
                    vec![
                        styled(DIM, short_id(&e.id)),
                        e.url.clone(),
                        events,
                        state,
                        day(&e.created_at),
                    ]
                },
                "No webhook endpoints yet. Add one with `invoist webhooks create <url>`.",
            )?;
            out.note(styled(
                DIM,
                format!("Events you can send: {}", list.known_events.join(", ")),
            ));
            Ok(())
        }
        WebhooksCmd::Create {
            url,
            events,
            description,
        } => {
            let mut endpoint = CreateWebhookEndpoint::new(url).events(events);
            endpoint.description = description;
            let created = client.create_webhook_endpoint(endpoint).await?;
            out.one(&created, || {
                format!(
                    "{} Invoist now sends events to {}\n\n  {}  {}\n\n{}\n",
                    styled(GOOD, "✓"),
                    created.url,
                    styled(DIM, "Signing secret"),
                    styled(BOLD, &created.secret),
                    styled(WARN, "This is the only time the secret is shown. Store it where your server can read it.")
                )
            })
        }
        WebhooksCmd::Delete { id, yes } => {
            let id = ids::webhook(&client, &id).await?;
            if !confirm(
                &format!("Stop sending events to endpoint {}?", short_id(&id)),
                yes,
            )? {
                out.note("Nothing changed.");
                return Ok(());
            }
            client.delete_webhook_endpoint(&id).await?;
            out.note(format!(
                "{} Deleted endpoint {}",
                styled(GOOD, "✓"),
                short_id(&id)
            ));
            Ok(())
        }
        WebhooksCmd::Verify { .. } => unreachable!("handled above"),
    }
}

fn verify(body: &Path, signature: &str, secret: &str, out: Out) -> Result<()> {
    let payload = if body == Path::new("-") {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| CliError::io("couldn't read standard input", e))?;
        buf
    } else {
        std::fs::read(body)
            .map_err(|e| CliError::io(format!("couldn't read {}", body.display()), e))?
    };
    // The SDK's own check: the same rules every Invoist library applies.
    let event = invoist::webhooks::verify(&payload, signature, secret)?;
    out.one(&event, || {
        format!(
            "{} The signature is valid.\n  {}  {}\n  {}  {}\n  {}  {}\n",
            styled(GOOD, "✓"),
            styled(DIM, "event"),
            event.event_type,
            styled(DIM, "id   "),
            event.id,
            styled(DIM, "at   "),
            event.created_at
        )
    })
}
