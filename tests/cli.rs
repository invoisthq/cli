//! The `invoist` binary end to end, against a mock of the Invoist API.
//!
//! Every response is a made-up fixture in `tests/fixtures`, shaped like the
//! API's own (the app's unit tests check each one deserializes into the
//! SDK's models). No real key, server or data is ever used.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command as StdCommand, Stdio};

use assert_cmd::Command;
use predicates::prelude::*;
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_KEY: &str = "inv_sk_test_0000000000000000example";
const DOC_ID: &str = "01a19812-0000-7000-8000-7bbb86296ebf";
const BILL_ID: &str = "02b10000-0000-7000-8000-c62d2950e7ac";

fn fixture(name: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("fixture exists");
    serde_json::from_str(&text).expect("fixture is JSON")
}

fn ok(name: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(fixture(name))
}

fn api_error(status: u16, code: &str, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .insert_header("X-Request-Id", "req_test_1")
        .set_body_json(serde_json::json!({ "error": { "code": code, "message": message } }))
}

/// The few variables a process needs from the system: `PATH`, and on
/// Windows `SystemRoot`, without which it can't open a network socket.
/// Everything else is cleared, so nothing from the machine running the
/// tests (a real key, a profile) can leak in.
fn system_env() -> Vec<(String, std::ffi::OsString)> {
    ["PATH", "SystemRoot", "SYSTEMROOT", "TEMP", "TMP"]
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|v| (name.to_string(), v)))
        .collect()
}

/// The binary, pointed at the mock, with a throwaway config folder, no
/// keychain, no colour and no browser.
struct Env {
    server: MockServer,
    config: tempfile::TempDir,
}

impl Env {
    async fn new() -> Env {
        Env {
            server: MockServer::start().await,
            config: tempfile::tempdir().expect("temp dir"),
        }
    }

    fn cmd(&self) -> Command {
        let mut cmd = Command::cargo_bin("invoist").expect("binary builds");
        cmd.env_clear()
            .envs(system_env())
            .env("INVOIST_BASE_URL", self.server.uri())
            .env("INVOIST_CONFIG_DIR", self.config.path())
            .env("INVOIST_NO_KEYCHAIN", "1")
            .env("INVOIST_NO_BROWSER", "1")
            .env("NO_COLOR", "1")
            .env("CI", "1");
        cmd
    }

    /// Signed in with a test key, as a script would be.
    fn keyed(&self) -> Command {
        let mut cmd = self.cmd();
        cmd.env("INVOIST_API_KEY", TEST_KEY);
        cmd
    }

    async fn mount(&self, mock: Mock) {
        mock.mount(&self.server).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn not_signed_in_exits_3_and_says_how_to_sign_in() {
    let env = Env::new().await;
    env.cmd()
        .args(["documents", "list"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "You're not signed in. Run `invoist login`",
        ));
}

#[tokio::test(flavor = "multi_thread")]
async fn documents_list_prints_a_table_json_and_csv() {
    let env = Env::new().await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/documents"))
            .and(header(
                "authorization",
                format!("Bearer {TEST_KEY}").as_str(),
            ))
            .respond_with(ok("documents.json")),
    )
    .await;

    let out = env
        .keyed()
        .args(["documents", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let table = String::from_utf8(out).unwrap();
    insta::assert_snapshot!("documents_list", table);
    assert!(table.contains("Northwind Paper Co."));
    assert!(table.contains("$1,204.20"));
    assert!(table.contains("to check"));
    assert!(
        table.contains("86296ebf"),
        "the short id is the end of the id"
    );

    let out = env
        .keyed()
        .args(["documents", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(json[0]["vendor_name"], "Northwind Paper Co.");
    assert_eq!(json[0]["total_cents"], 120420);

    let out = env
        .keyed()
        .args(["documents", "list", "-o", "csv"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let csv = String::from_utf8(out).unwrap();
    assert_eq!(
        csv.lines().next(),
        Some("id,supplier,invoice,total,status,added")
    );
    assert_eq!(csv.lines().count(), 4);

    let out = env
        .keyed()
        .args(["documents", "list", "-o", "ndjson"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(String::from_utf8(out).unwrap().lines().count(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_short_id_finds_the_invoice() {
    let env = Env::new().await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/documents"))
            .respond_with(ok("documents.json")),
    )
    .await;
    env.mount(
        Mock::given(method("GET"))
            .and(path(format!("/v1/documents/{DOC_ID}")))
            .respond_with(ok("document.json")),
    )
    .await;

    let out = env
        .keyed()
        .args(["documents", "get", DOC_ID])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let card = String::from_utf8(out).unwrap();
    insta::assert_snapshot!("document_card", card);

    // The 8 characters a table shows are enough.
    env.keyed()
        .args(["documents", "get", &DOC_ID[28..]])
        .assert()
        .success()
        .stdout(predicate::str::contains("INV-2041"));

    // "01a19812" starts two invoices' ids in the fixture: the CLI asks for more.
    env.keyed()
        .args(["documents", "get", "01a19812"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "2 invoices have ids that start or end with `01a19812`",
        ));
    env.keyed()
        .args(["documents", "get", "0b0b"])
        .assert()
        .code(4)
        .stderr(predicate::str::contains(
            "no invoice has an id that starts or ends with `0b0b`",
        ));
    env.keyed()
        .args(["documents", "get", "zz-not-an-id"])
        .assert()
        .code(2);
}

#[tokio::test(flavor = "multi_thread")]
async fn extract_waits_and_reports_what_needs_checking() {
    let env = Env::new().await;
    env.mount(
        Mock::given(method("POST"))
            .and(path("/v1/extract"))
            .respond_with(ResponseTemplate::new(201).set_body_json(fixture("extract.json"))),
    )
    .await;
    env.mount(
        Mock::given(method("GET"))
            .and(path(format!("/v1/documents/{DOC_ID}")))
            .respond_with(ok("document.json")),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("northwind-2041.pdf");
    std::fs::write(&file, b"%PDF-1.7 made-up test file").unwrap();

    let out = env
        .keyed()
        .args(["extract", "--idempotency-key", "order-1042"])
        .arg(&file)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let line = String::from_utf8(out).unwrap();
    let sent = env.server.received_requests().await.unwrap_or_default();
    let upload = sent
        .iter()
        .find(|r| r.url.path() == "/v1/extract")
        .expect("uploaded");
    assert_eq!(
        upload
            .headers
            .get("idempotency-key")
            .map(|v| v.to_str().unwrap()),
        Some("order-1042")
    );
    assert!(line.contains("Northwind Paper Co. · $1,204.20"), "{line}");
    assert!(line.contains("check: bank details changed"), "{line}");

    env.keyed()
        .args(["extract", "--fail-on-review"])
        .arg(&file)
        .assert()
        .code(5)
        .stderr(predicate::str::contains("1 invoice was set aside"));

    let out = env
        .keyed()
        .args(["extract", "--json"])
        .arg(&file)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(json[0]["extraction"]["status"], "done");
    assert_eq!(json[0]["needs_check"][0], "bank details changed");
}

#[tokio::test(flavor = "multi_thread")]
async fn extract_checks_files_before_uploading_anything() {
    let env = Env::new().await;
    let dir = tempfile::tempdir().unwrap();
    let text = dir.path().join("notes.txt");
    std::fs::write(&text, b"not an invoice").unwrap();
    env.keyed()
        .arg("extract")
        .arg(&text)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("isn't a PDF, PNG or JPEG file"));
    env.keyed()
        .args(["extract", "missing.pdf"])
        .assert()
        .code(4);
    let empty = tempfile::tempdir().unwrap();
    env.keyed()
        .arg("extract")
        .arg(empty.path())
        .assert()
        .code(2);
    // Nothing reached the API.
    assert!(
        env.server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn api_errors_are_explained_with_their_request_id() {
    let env = Env::new().await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/bills/summary"))
            .respond_with(api_error(
                403,
                "household_feature",
                "this is part of Invoist Personal, for a household's bills",
            )),
    )
    .await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/bills"))
            .respond_with(ok("bills.json")),
    )
    .await;
    env.keyed()
        .arg("bills")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("you're signed in to a business"))
        .stderr(predicate::str::contains("request req_test_1"));

    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/usage"))
            .respond_with(api_error(401, "invalid_api_key", "invalid API key")),
    )
    .await;
    let out = env
        .keyed()
        .args(["usage", "--json"])
        .assert()
        .code(3)
        .get_output()
        .stderr
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(json["error"]["code"], "invalid_api_key");
    assert_eq!(json["error"]["request_id"], "req_test_1");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_resolve_finds_the_item_by_prefix() {
    let env = Env::new().await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/review-queue"))
            .respond_with(ok("review.json")),
    )
    .await;
    env.mount(
        Mock::given(method("POST"))
            .and(path("/v1/review-queue/01a19814-0000-7000-8000-9f939e627a20/resolve"))
            .and(body_string_contains("\"dismissed\""))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "review_queue_item": fixture("document.json")["review_queue"][0].clone() }),
            )),
    )
    .await;
    let out = env
        .keyed()
        .args(["review", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("bank details changed")
    );
    env.keyed()
        .args([
            "review",
            "resolve",
            "01a19814-0000-7000-8000-9f939e627a20",
            "--dismiss",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Dismissed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_household_sees_what_is_left_to_pay_and_marks_bills() {
    let env = Env::new().await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/bills/summary"))
            .and(query_param("days", "30"))
            .respond_with(ok("bills_summary.json")),
    )
    .await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/bills"))
            .respond_with(ok("bills.json")),
    )
    .await;
    env.mount(
        Mock::given(method("GET"))
            .and(path(format!("/v1/bills/{BILL_ID}")))
            .respond_with(ok("bill.json")),
    )
    .await;
    env.mount(
        Mock::given(method("POST"))
            .and(path(format!("/v1/bills/{BILL_ID}/paid")))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"bill": {"paid_at": "2026-11-03T10:00:00Z", "paid_by": "user"}}),
            )),
    )
    .await;

    let out = env
        .keyed()
        .arg("bills")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let summary = String::from_utf8(out).unwrap();
    insta::assert_snapshot!("bills_summary", summary);

    let out = env
        .keyed()
        .args(["bills", "get", BILL_ID])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let card = String::from_utf8(out).unwrap();
    assert!(card.contains("IBAN"));
    assert!(card.contains("RF71 1234 5600 0029"));

    // No terminal and no --yes: refuses instead of waiting for an answer.
    env.keyed()
        .args(["bills", "paid", BILL_ID])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Add --yes"));
    env.keyed()
        .args(["bills", "paid", BILL_ID, "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Brightline Energy bill marked as paid",
        ));

    env.keyed()
        .args(["bills", "list", "--month", "2026-13"])
        .assert()
        .code(2);
}

#[tokio::test(flavor = "multi_thread")]
async fn export_filters_by_day_and_writes_csv() {
    let env = Env::new().await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/documents"))
            .respond_with(ok("documents.json")),
    )
    .await;
    let out = env
        .keyed()
        .args(["export", "--since", "2026-10-25", "--until", "2026-11-30"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let csv = String::from_utf8(out).unwrap();
    let mut lines = csv.lines();
    assert_eq!(
        lines.next(),
        Some(
            "id,file,supplier,invoice_number,reference,total,currency,category,status,to_check,added"
        )
    );
    let rows: Vec<&str> = lines.collect();
    assert_eq!(rows.len(), 2, "{csv}");
    assert!(rows[0].contains(",1204.20,USD,Office supplies,done,true,2026-11-02"));
    env.keyed()
        .args(["export", "--since", "1/11/2026"])
        .assert()
        .code(2);
}

#[tokio::test(flavor = "multi_thread")]
async fn webhooks_verify_uses_the_sdks_check_locally() {
    let env = Env::new().await;
    let body = r#"{"id":"evt_1","type":"extraction.completed","created_at":"2026-11-03T10:00:00Z","data":{"document_id":"x"}}"#;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let signature = invoist::webhooks::sign(body, "whsec_made_up_for_tests", now);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("body.json");
    std::fs::write(&file, body).unwrap();

    // No key is needed: verifying is local.
    env.cmd()
        .args(["webhooks", "verify", "--signature", &signature])
        .arg(&file)
        .env("INVOIST_WEBHOOK_SECRET", "whsec_made_up_for_tests")
        .assert()
        .success()
        .stdout(predicate::str::contains("extraction.completed"));
    env.cmd()
        .args(["webhooks", "verify", "--signature", &signature])
        .arg(&file)
        .env("INVOIST_WEBHOOK_SECRET", "a_different_secret")
        .assert()
        .code(1);
}

#[tokio::test(flavor = "multi_thread")]
async fn login_with_a_key_saves_the_profile_and_status_reads_it() {
    let env = Env::new().await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/usage"))
            .respond_with(ok("usage.json")),
    )
    .await;
    env.cmd()
        .args(["login", "--with-key"])
        .write_stdin(format!("{TEST_KEY}\n"))
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Signed in to a business in test mode",
        ));

    let config = std::fs::read_to_string(env.config.path().join("config.toml")).unwrap();
    assert!(config.contains("kind = \"business\""));
    assert!(config.contains("mode = \"test\""));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(env.config.path().join("config.toml"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the file holding the key is private");
    }

    let out = env
        .cmd()
        .args(["status", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(status["kind"], "business");
    assert_eq!(status["mode"], "test");
    assert_eq!(status["works"], true);
    assert_eq!(status["key_hint"], "…mple");
    // The key itself is never printed.
    assert!(!String::from_utf8_lossy(&out).contains(TEST_KEY));

    env.cmd().arg("logout").assert().success();
    env.cmd().args(["documents", "list"]).assert().code(3);
}

#[tokio::test(flavor = "multi_thread")]
async fn login_rejects_something_that_is_not_a_key() {
    let env = Env::new().await;
    env.cmd()
        .args(["login", "--with-key"])
        .write_stdin("hunter2\n")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("doesn't look like an Invoist key"));
}

/// The whole browser sign-in: discovery, registration, the approval page
/// (played by this test), the local callback and the token exchange.
#[tokio::test(flavor = "multi_thread")]
async fn login_in_the_browser_signs_a_household_in() {
    let env = Env::new().await;
    let base = env.server.uri();
    env.mount(
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/oauth/authorize"),
                "token_endpoint": format!("{base}/oauth/token"),
                "registration_endpoint": format!("{base}/oauth/register"),
                "revocation_endpoint": format!("{base}/oauth/revoke"),
                "scopes_supported": ["bills:read", "bills:write", "documents:read"],
            }))),
    )
    .await;
    env.mount(
        Mock::given(method("POST"))
            .and(path("/oauth/register"))
            .and(body_string_contains("Invoist CLI"))
            .and(body_string_contains("http://127.0.0.1/callback"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({"client_id": "inv_mcp_cli_test"})),
            ),
    )
    .await;
    env.mount(
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .and(body_string_contains(
                "\"code\":\"code-from-the-approval-page\"",
            ))
            .and(body_string_contains("code_verifier"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "inv_at_made_up_access",
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": "inv_rt_made_up_refresh",
                "scope": "bills:read bills:write",
            }))),
    )
    .await;

    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("invoist"))
        .arg("login")
        .env_clear()
        .envs(system_env())
        .env("INVOIST_BASE_URL", &base)
        .env("INVOIST_CONFIG_DIR", env.config.path())
        .env("INVOIST_NO_KEYCHAIN", "1")
        .env("INVOIST_NO_BROWSER", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("starts");

    // Read the link the CLI prints, as a person would open it.
    let mut stderr = BufReader::new(child.stderr.take().expect("stderr"));
    let mut link = String::new();
    let mut line = String::new();
    while stderr.read_line(&mut line).expect("reads") > 0 {
        if let Some(at) = line.find("http") {
            link = line[at..].trim().to_string();
            break;
        }
        line.clear();
    }
    let url = invoist::reqwest::Url::parse(&link).expect("a sign-in link");
    assert_eq!(url.path(), "/oauth/authorize");
    let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(q["client_id"], "inv_mcp_cli_test");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["scope"], "bills:read bills:write documents:read");
    let redirect = invoist::reqwest::Url::parse(&q["redirect_uri"]).expect("a redirect");
    assert_eq!(redirect.host_str(), Some("127.0.0.1"));

    // The approval page sends the browser back to the CLI.
    let port = redirect.port().expect("a port");
    // Browsers open a spare connection first and may send nothing on it;
    // the real redirect must not wait behind it.
    let _idle = TcpStream::connect(("127.0.0.1", port)).expect("the CLI listens");
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("the CLI listens");
    let target = format!(
        "/callback?code=code-from-the-approval-page&state={}&iss={}",
        q["state"],
        url_escape(&base)
    );
    // The request arrives in pieces, as it can over any socket.
    stream
        .write_all(format!("GET {target} HTTP/1.1\r\n").as_bytes())
        .unwrap();
    stream.flush().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    stream
        .write_all(b"Host: 127.0.0.1\r\nAccept: text/html\r\n\r\n")
        .unwrap();
    let mut page = String::new();
    stream.read_to_string(&mut page).unwrap();
    assert!(page.contains("You're signed in"), "{page}");

    let mut rest = String::new();
    stderr.read_to_string(&mut rest).unwrap();
    let status = child.wait().expect("finishes");
    assert!(status.success(), "{rest}");
    assert!(rest.contains("Signed in to a household"), "{rest}");

    let config = std::fs::read_to_string(env.config.path().join("config.toml")).unwrap();
    assert!(config.contains("kind = \"household\""));
    assert!(config.contains("signed_in_with = \"browser\""));
    assert!(config.contains("client_id = \"inv_mcp_cli_test\""));
}

/// A sign-in that came back with someone else's state is refused.
#[tokio::test(flavor = "multi_thread")]
async fn login_refuses_a_callback_from_another_sign_in() {
    let env = Env::new().await;
    let base = env.server.uri();
    env.mount(
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/oauth/authorize"),
                "token_endpoint": format!("{base}/oauth/token"),
                "registration_endpoint": format!("{base}/oauth/register"),
                "scopes_supported": ["documents:read"],
            }))),
    )
    .await;
    env.mount(
        Mock::given(method("POST"))
            .and(path("/oauth/register"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({"client_id": "inv_mcp_cli_test"})),
            ),
    )
    .await;
    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("invoist"))
        .arg("login")
        .env_clear()
        .envs(system_env())
        .env("INVOIST_BASE_URL", &base)
        .env("INVOIST_CONFIG_DIR", env.config.path())
        .env("INVOIST_NO_KEYCHAIN", "1")
        .env("INVOIST_NO_BROWSER", "1")
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("starts");
    let mut stderr = BufReader::new(child.stderr.take().expect("stderr"));
    let mut line = String::new();
    let mut link = String::new();
    while stderr.read_line(&mut line).expect("reads") > 0 {
        if let Some(at) = line.find("http") {
            link = line[at..].trim().to_string();
            break;
        }
        line.clear();
    }
    let url = invoist::reqwest::Url::parse(&link).unwrap();
    let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    let port = invoist::reqwest::Url::parse(&q["redirect_uri"])
        .unwrap()
        .port()
        .unwrap();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(stream, "GET /callback?code=x&state=forged HTTP/1.1\r\n\r\n").unwrap();
    let mut rest = String::new();
    stderr.read_to_string(&mut rest).unwrap();
    assert!(!child.wait().unwrap().success());
    assert!(rest.contains("different sign-in"), "{rest}");
    assert!(
        !env.config.path().join("config.toml").exists(),
        "nothing was saved"
    );
}

fn url_escape(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[test]
fn completions_and_man_pages() {
    let mut cmd = Command::cargo_bin("invoist").unwrap();
    cmd.args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("invoist"));
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("invoist")
        .unwrap()
        .args(["man", "--dir"])
        .arg(dir.path())
        .assert()
        .success();
    assert!(dir.path().join("invoist.1").exists());
    assert!(dir.path().join("invoist-extract.1").exists());
}

#[test]
fn without_a_terminal_bare_invoist_prints_help() {
    Command::cargo_bin("invoist")
        .unwrap()
        .env("CI", "1")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Read invoices, work through checks",
        ));
}

#[test]
fn version_names_the_sdk() {
    Command::cargo_bin("invoist")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(
            predicate::str::is_match(r"^invoist \d+\.\d+\.\d+ \(Rust SDK \d+\.\d+\.\d+\)").unwrap(),
        );
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_tables() {
    let env = Env::new().await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/categories"))
            .respond_with(ok("categories.json")),
    )
    .await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/usage"))
            .respond_with(ok("usage.json")),
    )
    .await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/stats/overview"))
            .respond_with(ok("stats.json")),
    )
    .await;
    for (command, name) in [
        ("categories", "categories"),
        ("usage", "usage"),
        ("stats", "stats"),
    ] {
        let out = env
            .keyed()
            .arg(command)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        insta::assert_snapshot!(name, String::from_utf8(out).unwrap());
    }
    let out = env
        .keyed()
        .args(["usage", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(json, fixture("usage.json"), "--json is the API's own shape");
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_never_overwrite_a_file() {
    let env = Env::new().await;
    let pdf = b"%PDF-1.7 made-up original".to_vec();
    env.mount(
        Mock::given(method("GET"))
            .and(path(format!("/v1/documents/{DOC_ID}/file")))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/pdf")
                    .set_body_bytes(pdf.clone()),
            ),
    )
    .await;
    env.mount(
        Mock::given(method("GET"))
            .and(path(format!("/v1/bills/{BILL_ID}/file")))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/pdf")
                    .set_body_bytes(pdf.clone()),
            ),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("northwind.pdf");
    env.keyed()
        .args(["documents", "download", DOC_ID, "--out"])
        .arg(&target)
        .assert()
        .success();
    assert_eq!(std::fs::read(&target).unwrap(), pdf);

    std::fs::write(&target, b"keep me").unwrap();
    env.keyed()
        .args(["documents", "download", DOC_ID, "--out"])
        .arg(&target)
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"keep me",
        "the existing file is untouched"
    );

    let out = env
        .keyed()
        .args(["documents", "download", DOC_ID, "--out", "-"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(out, pdf, "`-` writes the file itself to standard output");

    // Without --out, the file keeps the name it was uploaded with.
    env.mount(
        Mock::given(method("GET"))
            .and(path(format!("/v1/documents/{DOC_ID}")))
            .respond_with(ok("document.json")),
    )
    .await;
    env.keyed()
        .args(["documents", "download", DOC_ID])
        .current_dir(dir.path())
        .assert()
        .success();
    assert_eq!(
        std::fs::read(dir.path().join("northwind-2041.pdf")).unwrap(),
        pdf
    );

    let bill = dir.path().join("energy.pdf");
    env.keyed()
        .args(["bills", "download", BILL_ID, "--out"])
        .arg(&bill)
        .assert()
        .success();
    assert_eq!(std::fs::read(&bill).unwrap(), pdf);
}

#[tokio::test(flavor = "multi_thread")]
async fn webhooks_are_created_listed_and_deleted() {
    let env = Env::new().await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/webhook-endpoints"))
            .respond_with(ok("webhooks.json")),
    )
    .await;
    env.mount(
        Mock::given(method("POST"))
            .and(path("/v1/webhook-endpoints"))
            .and(body_string_contains("https://example.com/invoist"))
            .and(body_string_contains("extraction.completed"))
            .respond_with(ResponseTemplate::new(201).set_body_json(
                serde_json::json!({"endpoint": {
                    "id": "01a199ab-0000-7000-8000-5e3a9c1d2b4f",
                    "url": "https://example.com/invoist",
                    "description": "",
                    "events": ["extraction.completed"],
                    "enabled": true,
                    "consecutive_failures": 0,
                    "created_at": "2026-11-03T10:00:00Z",
                    "secret": "whsec_made_up_for_tests_only",
                }}),
            )),
    )
    .await;
    let endpoint = "01a199aa-0000-7000-8000-b02cd01f1763";
    env.mount(
        Mock::given(method("DELETE"))
            .and(path(format!("/v1/webhook-endpoints/{endpoint}")))
            .respond_with(ResponseTemplate::new(204)),
    )
    .await;

    let out = env
        .keyed()
        .args(["webhooks", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    insta::assert_snapshot!("webhooks_list", String::from_utf8(out).unwrap());

    env.keyed()
        .args([
            "webhooks",
            "create",
            "https://example.com/invoist",
            "--event",
            "extraction.completed",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("whsec_made_up_for_tests_only"));

    env.keyed()
        .args(["webhooks", "delete", &endpoint[28..]])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Add --yes"));
    env.keyed()
        .args(["webhooks", "delete", &endpoint[28..], "--yes"])
        .assert()
        .success();
    let deleted = env
        .server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "DELETE")
        .count();
    assert_eq!(deleted, 1, "only the confirmed delete reached the API");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_household_sees_spending_and_held_emails() {
    let env = Env::new().await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/bills/stats"))
            .respond_with(ok("bills_stats.json")),
    )
    .await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/held-emails"))
            .respond_with(ok("held_emails.json")),
    )
    .await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/bills"))
            .respond_with(ok("bills.json")),
    )
    .await;
    env.mount(
        Mock::given(method("GET"))
            .and(path(format!("/v1/bills/{BILL_ID}")))
            .respond_with(ok("bill.json")),
    )
    .await;
    env.mount(
        Mock::given(method("DELETE"))
            .and(path(format!("/v1/bills/{BILL_ID}/paid")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"bill": {}})),
            ),
    )
    .await;

    let out = env
        .keyed()
        .args(["bills", "stats"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    insta::assert_snapshot!("bills_stats", String::from_utf8(out).unwrap());
    let out = env
        .keyed()
        .arg("held-emails")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    insta::assert_snapshot!("held_emails", String::from_utf8(out).unwrap());
    let out = env
        .keyed()
        .args(["bills", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    insta::assert_snapshot!("bills_list", String::from_utf8(out).unwrap());
    env.keyed()
        .args(["bills", "unpaid", BILL_ID, "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("marked as not paid"));
}

#[tokio::test(flavor = "multi_thread")]
async fn profiles_keep_two_sign_ins_apart() {
    let env = Env::new().await;
    env.mount(
        Mock::given(method("GET"))
            .and(path("/v1/usage"))
            .respond_with(ok("usage.json")),
    )
    .await;
    for profile in ["work", "test"] {
        env.cmd()
            .args(["login", "--with-key", "--profile", profile])
            .write_stdin(format!("{TEST_KEY}\n"))
            .assert()
            .success();
    }
    let out = env
        .cmd()
        .args(["profile", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let list: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert_eq!(names, vec!["test", "work"]);

    env.cmd()
        .args(["profile", "use", "test"])
        .assert()
        .success();
    let out = env
        .cmd()
        .args(["status", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let status: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(status["profile"], "test");
    env.cmd().args(["profile", "use", "nope"]).assert().code(4);
    env.cmd()
        .args(["--profile", "work", "status"])
        .assert()
        .success();
}
