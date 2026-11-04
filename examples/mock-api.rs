//! A stand-in for the Invoist API with made-up data, for trying the CLI and
//! the full-screen app without an account, and for recording the demo.
//!
//!   cargo run --example mock-api              # a business, on port 4010
//!   cargo run --example mock-api -- --household
//!
//! Then, in another terminal:
//!
//!   export INVOIST_BASE_URL=http://127.0.0.1:4010
//!   export INVOIST_API_KEY=inv_sk_test_mock
//!   invoist
//!
//! It answers every request the CLI makes with the fixtures in
//! `tests/fixtures`. Every invoice opens the same detail, and every bill too.

use std::net::TcpListener;

use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ID: &str = "[0-9a-f-]{36}";

fn json(text: &str) -> ResponseTemplate {
    let value: serde_json::Value = serde_json::from_str(text).expect("fixture is JSON");
    ResponseTemplate::new(200).set_body_json(value)
}

/// A one-page PDF with nothing on it, for downloads.
const BLANK_PDF: &[u8] = b"%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 595 842]>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n";

#[tokio::main]
async fn main() {
    let household = std::env::args().any(|a| a == "--household");
    let port = std::env::args()
        .skip_while(|a| a != "--port")
        .nth(1)
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(4010);
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap_or_else(|e| {
        eprintln!("couldn't listen on port {port}: {e}. Choose another with --port.");
        std::process::exit(1);
    });
    let server = MockServer::builder().listener(listener).start().await;

    let pdf = ResponseTemplate::new(200)
        .insert_header("Content-Type", "application/pdf")
        .set_body_bytes(BLANK_PDF);
    let wrong_kind = |code: &str, message: &str| {
        ResponseTemplate::new(403)
            .set_body_json(serde_json::json!({"error": {"code": code, "message": message}}))
    };

    let business = vec![
        Mock::given(method("GET"))
            .and(path("/v1/usage"))
            .respond_with(json(include_str!("../tests/fixtures/usage.json"))),
        Mock::given(method("GET"))
            .and(path("/v1/documents"))
            .respond_with(json(include_str!("../tests/fixtures/documents.json"))),
        Mock::given(method("GET"))
            .and(path_regex(format!("^/v1/documents/{ID}$")))
            .respond_with(json(include_str!("../tests/fixtures/document.json"))),
        Mock::given(method("GET"))
            .and(path_regex(format!("^/v1/documents/{ID}/file$")))
            .respond_with(pdf.clone()),
        Mock::given(method("POST"))
            .and(path("/v1/extract"))
            .respond_with(json(include_str!("../tests/fixtures/extract.json")).set_delay(std::time::Duration::from_millis(600))),
        Mock::given(method("GET"))
            .and(path("/v1/review-queue"))
            .respond_with(json(include_str!("../tests/fixtures/review.json"))),
        Mock::given(method("POST"))
            .and(path_regex(format!("^/v1/review-queue/{ID}/resolve$")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "review_queue_item": serde_json::from_str::<serde_json::Value>(include_str!("../tests/fixtures/document.json"))
                    .expect("fixture is JSON")["review_queue"][0]
                    .clone()
            }))),
        Mock::given(method("GET"))
            .and(path("/v1/categories"))
            .respond_with(json(include_str!("../tests/fixtures/categories.json"))),
        Mock::given(method("GET"))
            .and(path("/v1/stats/overview"))
            .respond_with(json(include_str!("../tests/fixtures/stats.json"))),
        Mock::given(method("GET"))
            .and(path("/v1/webhook-endpoints"))
            .respond_with(json(include_str!("../tests/fixtures/webhooks.json"))),
    ];
    let home = vec![
        Mock::given(method("GET"))
            .and(path("/v1/bills/summary"))
            .respond_with(json(include_str!("../tests/fixtures/bills_summary.json"))),
        Mock::given(method("GET"))
            .and(path("/v1/bills/stats"))
            .respond_with(json(include_str!("../tests/fixtures/bills_stats.json"))),
        Mock::given(method("GET"))
            .and(path("/v1/bills"))
            .respond_with(json(include_str!("../tests/fixtures/bills.json"))),
        Mock::given(method("GET"))
            .and(path_regex(format!("^/v1/bills/{ID}$")))
            .respond_with(json(include_str!("../tests/fixtures/bill.json"))),
        Mock::given(method("GET"))
            .and(path_regex(format!("^/v1/bills/{ID}/file$")))
            .respond_with(pdf),
        Mock::given(method("POST"))
            .and(path_regex(format!("^/v1/bills/{ID}/paid$")))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"bill": {"paid_at": "2026-11-03T10:00:00Z", "paid_by": "you"}}),
            )),
        Mock::given(method("DELETE"))
            .and(path_regex(format!("^/v1/bills/{ID}/paid$")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"bill": {}})),
            ),
        Mock::given(method("GET"))
            .and(path("/v1/held-emails"))
            .respond_with(json(include_str!("../tests/fixtures/held_emails.json"))),
    ];
    // Like the real API, each kind of account is refused on the other's
    // routes. That is also how the CLI tells the two apart.
    let (mocks, refused) = if household {
        (
            home,
            Mock::given(path_regex(
                "^/v1/(usage|documents|extract|review-queue|categories|stats|webhook-endpoints)",
            ))
            .respond_with(wrong_kind(
                "business_feature",
                "this is part of Invoist for businesses",
            )),
        )
    } else {
        (
            business,
            Mock::given(path_regex("^/v1/(bills|held-emails)")).respond_with(wrong_kind(
                "household_feature",
                "this is part of Invoist Personal, for a household's bills",
            )),
        )
    };
    refused.mount(&server).await;
    for mock in mocks {
        mock.mount(&server).await;
    }

    let kind = if household {
        "a household"
    } else {
        "a business"
    };
    println!(
        "A made-up Invoist API for {kind} is running at {}",
        server.uri()
    );
    println!();
    println!("  export INVOIST_BASE_URL={}", server.uri());
    println!("  export INVOIST_API_KEY=inv_sk_test_mock");
    println!("  invoist");
    println!();
    println!("Press Ctrl-C to stop it.");
    let _ = tokio::signal::ctrl_c().await;
}
