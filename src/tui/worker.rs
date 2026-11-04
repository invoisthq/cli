//! Runs the app's requests through the SDK, in the background, and sends
//! the results back. Each request is its own task, so a slow upload never
//! holds up a list. The session is shared, so a browser sign-in refreshes
//! once when its hour is up and every later request uses the new token.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use invoist::models::{ExtractionStatus, ReviewStatus};
use invoist::{
    BillsStatsParams, BillsSummaryParams, Client, ExtractOptions, ListBillsParams,
    ListDocumentsParams, ListReviewQueueParams, UploadFile, WaitOptions,
};
use tokio::sync::{Mutex, mpsc};

use crate::error::CliError;
use crate::output::{money, reason_label};
use crate::session::Session;
use crate::tui::app::{BillsData, Msg, OverviewData, Request, ToPayData, UploadStatus};

const PAGE: u32 = 200;

#[derive(Clone)]
pub struct Worker {
    session: Arc<Mutex<Session>>,
    tx: mpsc::UnboundedSender<Msg>,
}

fn text(e: impl Into<CliError>) -> String {
    e.into().headline()
}

impl Worker {
    pub fn new(session: Session, tx: mpsc::UnboundedSender<Msg>) -> Worker {
        Worker {
            session: Arc::new(Mutex::new(session)),
            tx,
        }
    }

    async fn client(&self) -> Result<Client, String> {
        self.session
            .lock()
            .await
            .client()
            .await
            .map_err(|e| e.headline())
    }

    pub fn send(&self, request: Request) {
        let worker = self.clone();
        tokio::spawn(async move {
            let msg = worker.run(request).await;
            if let Some(msg) = msg {
                let _ = worker.tx.send(msg);
            }
        });
    }

    async fn run(&self, request: Request) -> Option<Msg> {
        let client = match self.client().await {
            Ok(c) => c,
            Err(e) => return Some(failure(request, e)),
        };
        Some(match request {
            Request::Overview => Msg::Overview(
                async {
                    let (stats, usage, latest) = tokio::try_join!(
                        client.get_stats(),
                        client.get_usage(),
                        client.list_documents(ListDocumentsParams::new().limit(8))
                    )
                    .map_err(text)?;
                    Ok(OverviewData {
                        stats,
                        usage,
                        latest,
                    })
                }
                .await,
            ),
            Request::Documents { search, flagged } => {
                let mut params = ListDocumentsParams::new().limit(PAGE).flagged(flagged);
                if !search.is_empty() {
                    params.q = Some(search);
                }
                Msg::Documents(client.list_documents(params).await.map_err(text))
            }
            Request::Document(id) => {
                let r = client.get_document(&id).await.map(Box::new).map_err(text);
                Msg::Document(id, r)
            }
            Request::Review => Msg::Review(
                client
                    .list_review_queue(ListReviewQueueParams::new().status(ReviewStatus::Pending))
                    .await
                    .map_err(text),
            ),
            Request::Resolve { id, dismiss } => {
                let resolution = if dismiss {
                    invoist::ReviewResolution::Dismissed
                } else {
                    invoist::ReviewResolution::Resolved
                };
                let r = client
                    .resolve_review_item(&id, resolution)
                    .await
                    .map(|_| ())
                    .map_err(text);
                Msg::Resolved(id, r)
            }
            Request::Webhooks => Msg::Webhooks(client.list_webhook_endpoints().await.map_err(text)),
            Request::Upload { index, path } => {
                self.upload(&client, index, path).await;
                return None;
            }
            Request::ToPay => Msg::ToPay(
                async {
                    let bills = client.bills();
                    let (summary, unpaid) = tokio::try_join!(
                        bills.summary(BillsSummaryParams::new()),
                        bills.list(ListBillsParams::new().paid(false).limit(PAGE))
                    )
                    .map_err(text)?;
                    Ok(ToPayData { summary, unpaid })
                }
                .await,
            ),
            Request::Bills(month) => Msg::Bills(
                month,
                async {
                    let api = client.bills();
                    let (bills, stats) = tokio::try_join!(
                        api.list(ListBillsParams::new().month(month).limit(PAGE)),
                        api.stats(BillsStatsParams::new().month(month))
                    )
                    .map_err(text)?;
                    Ok(BillsData { bills, stats })
                }
                .await,
            ),
            Request::Bill(id) => {
                let r = client.bills().get(&id).await.map(Box::new).map_err(text);
                Msg::Bill(id, r)
            }
            Request::Held => Msg::Held(client.held_emails().list().await.map_err(text)),
            Request::MarkPaid { id, paid } => {
                let r = if paid {
                    client.bills().mark_paid(&id).await
                } else {
                    client.bills().mark_unpaid(&id).await
                };
                Msg::Marked(id, paid, r.map(|_| ()).map_err(text))
            }
            Request::OpenFile { id, filename, bill } => {
                let file = if bill {
                    client.bills().get_file(&id).await
                } else {
                    client.get_document_file(&id).await
                };
                Msg::FileOpened(match file {
                    Ok(f) => open_downloaded(&id, &filename, &f.bytes),
                    Err(e) => Err(text(e)),
                })
            }
        })
    }

    async fn upload(&self, client: &Client, index: usize, path: PathBuf) {
        let step = |s: UploadStatus| {
            let _ = self.tx.send(Msg::UploadStep(index, s));
        };
        step(UploadStatus::Uploading);
        let result: Result<UploadStatus, String> = async {
            let file = UploadFile::from_path_async(&path)
                .await
                .map_err(|e| format!("couldn't read the file: {e}"))?;
            let res = client
                .extract(file, ExtractOptions::new())
                .await
                .map_err(text)?;
            let id = res.document.id.clone();
            let mut extraction = res.extraction;
            if extraction
                .as_ref()
                .is_none_or(|e| e.status == ExtractionStatus::Pending)
            {
                step(UploadStatus::Reading);
                extraction = Some(
                    client
                        .wait_for_extraction(
                            &id,
                            WaitOptions::new().timeout(Duration::from_secs(180)),
                        )
                        .await
                        .map_err(text)?,
                );
            }
            let detail = client.get_document(&id).await.map_err(text)?;
            let e = extraction.unwrap_or_else(|| unreachable!("set above"));
            if e.status == ExtractionStatus::Failed {
                return Ok(UploadStatus::Failed(
                    e.error_message
                        .unwrap_or_else(|| "Invoist couldn't read it".into()),
                ));
            }
            Ok(UploadStatus::Done {
                vendor: e.vendor_name.unwrap_or_else(|| "supplier not found".into()),
                total: money(e.total_cents, e.currency.as_deref()),
                duplicate: res.duplicate,
                checks: detail
                    .review_queue
                    .iter()
                    .filter(|i| i.status == ReviewStatus::Pending)
                    .map(|i| reason_label(&i.reason))
                    .collect(),
            })
        }
        .await;
        step(result.unwrap_or_else(UploadStatus::Failed));
    }
}

/// When the session itself can't produce a client, every request fails the
/// same way, attached to the screen that asked.
fn failure(request: Request, e: String) -> Msg {
    match request {
        Request::Overview => Msg::Overview(Err(e)),
        Request::Documents { .. } => Msg::Documents(Err(e)),
        Request::Document(id) => Msg::Document(id, Err(e)),
        Request::Review => Msg::Review(Err(e)),
        Request::Resolve { id, .. } => Msg::Resolved(id, Err(e)),
        Request::Webhooks => Msg::Webhooks(Err(e)),
        Request::Upload { index, .. } => Msg::UploadStep(index, UploadStatus::Failed(e)),
        Request::ToPay => Msg::ToPay(Err(e)),
        Request::Bills(m) => Msg::Bills(m, Err(e)),
        Request::Bill(id) => Msg::Bill(id, Err(e)),
        Request::Held => Msg::Held(Err(e)),
        Request::MarkPaid { id, paid } => Msg::Marked(id, paid, Err(e)),
        Request::OpenFile { .. } => Msg::FileOpened(Err(e)),
    }
}

/// Saves a downloaded file in a private temporary folder and opens it with
/// the system's viewer.
fn open_downloaded(id: &str, filename: &str, bytes: &[u8]) -> Result<String, String> {
    let dir = std::env::temp_dir().join("invoist-cli");
    std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't save the file: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let name = std::path::Path::new(filename)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "invoice".into());
    let path = dir.join(format!("{}-{name}", crate::output::short_id(id)));
    std::fs::write(&path, bytes).map_err(|e| format!("couldn't save the file: {e}"))?;
    if crate::oauth::open_browser(&path.to_string_lossy()) {
        Ok(name)
    } else {
        Err(format!(
            "Saved to {}, but couldn't open a viewer.",
            path.display()
        ))
    }
}
