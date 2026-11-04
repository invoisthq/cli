//! The terminal app without a terminal or a server: key sequences go in,
//! requests come out, made-up API responses are fed back, and each screen is
//! drawn into an in-memory buffer and compared with its snapshot.

use invoist::Month;
use invoist::models::{
    Bill, BillDetail, BillsStats, BillsSummary, Category, DocumentDetail, DocumentListItem,
    ExtractResult, HeldEmail, ReviewQueueListItem, StatsOverview, Usage, WebhookEndpointList,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use serde::de::DeserializeOwned;

use super::app::{
    App, BillsData, Msg, Overlay, OverviewData, Request, Screen, ToPayData, UploadItem,
    UploadStatus,
};
use super::ui;
use crate::config::{AccountKind, Mode};

fn fixture<T: DeserializeOwned>(json: &str, key: Option<&str>) -> T {
    let value: serde_json::Value = serde_json::from_str(json).expect("fixture is JSON");
    let value = match key {
        Some(k) => value[k].clone(),
        None => value,
    };
    serde_json::from_value(value).expect("fixture matches the SDK's model")
}

fn documents() -> Vec<DocumentListItem> {
    fixture(
        include_str!("../../tests/fixtures/documents.json"),
        Some("documents"),
    )
}
fn document() -> DocumentDetail {
    fixture(include_str!("../../tests/fixtures/document.json"), None)
}
fn review() -> Vec<ReviewQueueListItem> {
    fixture(
        include_str!("../../tests/fixtures/review.json"),
        Some("review_queue"),
    )
}
fn overview() -> OverviewData {
    OverviewData {
        stats: fixture::<StatsOverview>(include_str!("../../tests/fixtures/stats.json"), None),
        usage: fixture::<Usage>(include_str!("../../tests/fixtures/usage.json"), None),
        latest: documents(),
    }
}
fn bills() -> Vec<Bill> {
    fixture(
        include_str!("../../tests/fixtures/bills.json"),
        Some("bills"),
    )
}
fn bill() -> BillDetail {
    fixture(include_str!("../../tests/fixtures/bill.json"), None)
}
fn summary() -> BillsSummary {
    fixture(
        include_str!("../../tests/fixtures/bills_summary.json"),
        None,
    )
}
fn stats() -> BillsStats {
    fixture(include_str!("../../tests/fixtures/bills_stats.json"), None)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn november() -> Month {
    Month::new(2026, 11).expect("valid month")
}

const TODAY: &str = "2026-11-03";

fn business() -> App {
    let cwd = std::env::temp_dir();
    App::new(
        AccountKind::Business,
        Mode::Live,
        "default".into(),
        TODAY.into(),
        cwd,
        true,
    )
}

fn household() -> App {
    let cwd = std::env::temp_dir();
    App::new(
        AccountKind::Household,
        Mode::Test,
        "home".into(),
        TODAY.into(),
        cwd,
        true,
    )
}

/// Draws the app at a fixed size and returns the screen as text.
fn render(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal.draw(|f| ui::draw(f, app)).expect("draws");
    terminal.backend().to_string()
}

#[test]
fn every_fixture_matches_the_sdk_models() {
    let _ = documents();
    let _ = document();
    let _ = review();
    let _ = overview();
    let _ = bills();
    let _ = bill();
    let _ = summary();
    let _ = stats();
    let _: Vec<Category> = fixture(
        include_str!("../../tests/fixtures/categories.json"),
        Some("categories"),
    );
    let _: WebhookEndpointList = fixture(include_str!("../../tests/fixtures/webhooks.json"), None);
    let _: ExtractResult = fixture(include_str!("../../tests/fixtures/extract.json"), None);
    let _: Vec<HeldEmail> = fixture(
        include_str!("../../tests/fixtures/held_emails.json"),
        Some("held_emails"),
    );
}

#[test]
fn a_business_starts_on_the_overview() {
    let mut app = business();
    assert_eq!(app.start(), vec![Request::Overview]);
    assert_eq!(
        app.screens,
        vec![
            Screen::Overview,
            Screen::Documents,
            Screen::Review,
            Screen::Upload,
            Screen::Webhooks
        ]
    );
}

#[test]
fn a_household_starts_on_what_is_left_to_pay() {
    let mut app = household();
    assert_eq!(app.start(), vec![Request::ToPay]);
    assert_eq!(
        app.screens,
        vec![Screen::ToPay, Screen::Bills, Screen::Held]
    );
}

#[test]
fn moving_through_invoices_loads_each_ones_details_once() {
    let mut app = business();
    app.start();
    assert_eq!(
        app.key(key(KeyCode::Char('2'))),
        vec![Request::Documents {
            search: String::new(),
            flagged: false
        }]
    );
    let first = documents()[0].id.clone();
    let second = documents()[1].id.clone();
    assert_eq!(
        app.message(Msg::Documents(Ok(documents()))),
        vec![Request::Document(first.clone())]
    );
    assert_eq!(app.key(key(KeyCode::Down)), vec![Request::Document(second)]);
    // Back up: already loading, so not asked for again.
    assert_eq!(app.key(key(KeyCode::Up)), vec![]);
}

#[test]
fn searching_asks_the_api_and_starts_at_the_top() {
    let mut app = business();
    app.start();
    app.key(key(KeyCode::Char('2')));
    app.message(Msg::Documents(Ok(documents())));
    app.key(key(KeyCode::Down));
    assert!(app.key(key(KeyCode::Char('/'))).is_empty());
    for c in "north".chars() {
        app.key(key(KeyCode::Char(c)));
    }
    let reqs = app.key(key(KeyCode::Enter));
    assert!(reqs.contains(&Request::Documents {
        search: "north".into(),
        flagged: false
    }));
    assert_eq!(app.doc_cursor.index, 0);
    // Typing never runs shortcuts: `q` was text, not quit.
    app.key(key(KeyCode::Char('/')));
    app.key(key(KeyCode::Char('q')));
    assert!(!app.quit);
}

#[test]
fn the_to_check_filter_reloads_the_list() {
    let mut app = business();
    app.start();
    app.key(key(KeyCode::Char('2')));
    let reqs = app.key(key(KeyCode::Char('f')));
    assert!(reqs.contains(&Request::Documents {
        search: String::new(),
        flagged: true
    }));
}

#[test]
fn resolving_an_item_removes_it_and_says_so() {
    let mut app = business();
    app.start();
    assert_eq!(app.key(key(KeyCode::Char('3'))), vec![Request::Review]);
    app.message(Msg::Review(Ok(review())));
    let id = review()[0].id.clone();
    assert_eq!(
        app.key(key(KeyCode::Char('c'))),
        vec![Request::Resolve {
            id: id.clone(),
            dismiss: false
        }]
    );
    // A second press while saving does nothing.
    assert!(app.key(key(KeyCode::Char('c'))).is_empty());
    app.message(Msg::Resolved(id.clone(), Ok(())));
    assert_eq!(app.review.ready().map(Vec::len), Some(1));
    assert!(
        app.toast
            .as_ref()
            .is_some_and(|t| t.text.contains("checked"))
    );
}

#[test]
fn marking_a_bill_paid_asks_first() {
    let mut app = household();
    app.start();
    app.message(Msg::ToPay(Ok(ToPayData {
        summary: summary(),
        unpaid: bills(),
    })));
    assert!(app.key(key(KeyCode::Char('p'))).is_empty());
    assert!(matches!(app.overlay, Overlay::Confirm { .. }));
    // Anything but yes cancels.
    assert!(app.key(key(KeyCode::Char('n'))).is_empty());
    assert!(matches!(app.overlay, Overlay::None));
    app.key(key(KeyCode::Char('p')));
    let reqs = app.key(key(KeyCode::Char('y')));
    assert_eq!(
        reqs,
        vec![Request::MarkPaid {
            id: bills()[0].id.clone(),
            paid: true
        }]
    );
}

#[test]
fn unpaid_bills_are_soonest_first() {
    let mut app = household();
    let mut shuffled = bills();
    shuffled.reverse();
    app.message(Msg::ToPay(Ok(ToPayData {
        summary: summary(),
        unpaid: shuffled,
    })));
    let order: Vec<_> = app
        .unpaid_sorted()
        .iter()
        .map(|b| b.due_date.clone().unwrap_or_default())
        .collect();
    assert_eq!(order, vec!["2026-10-30", "2026-11-15", "2026-11-20"]);
}

#[test]
fn bills_move_between_months() {
    let mut app = household();
    app.start();
    app.key(key(KeyCode::Char('2')));
    assert_eq!(
        app.key(key(KeyCode::Char(']'))),
        vec![Request::Bills(Month::new(2026, 12).unwrap())]
    );
    assert_eq!(
        app.key(key(KeyCode::Char(']'))),
        vec![Request::Bills(Month::new(2027, 1).unwrap())]
    );
    assert_eq!(
        app.key(key(KeyCode::Char('['))),
        vec![Request::Bills(Month::new(2026, 12).unwrap())]
    );
}

#[test]
fn the_palette_finds_commands_by_any_letters() {
    let mut app = business();
    app.start();
    app.key(key(KeyCode::Char(':')));
    for c in "to check".chars() {
        app.key(key(KeyCode::Char(c)));
    }
    let reqs = app.key(key(KeyCode::Enter));
    assert_eq!(app.screen(), Screen::Review);
    assert_eq!(reqs, vec![Request::Review]);
}

#[test]
fn ctrl_c_and_q_quit() {
    let mut app = business();
    app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.quit);
    let mut app = business();
    app.key(key(KeyCode::Char('q')));
    assert!(app.quit);
}

#[test]
fn clicking_a_tab_changes_screen() {
    let mut app = business();
    app.start();
    render(&mut app, 120, 36);
    let (rect, _) = app.hits.tabs[1];
    let reqs = app.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.screen(), Screen::Documents);
    assert!(reqs.contains(&Request::Documents {
        search: String::new(),
        flagged: false
    }));
}

#[test]
fn the_upload_screen_lists_only_readable_files() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["b.pdf", "a.PNG", "notes.txt", ".hidden.pdf"] {
        std::fs::write(dir.path().join(name), b"x").unwrap();
    }
    std::fs::create_dir(dir.path().join("scans")).unwrap();
    let mut app = App::new(
        AccountKind::Business,
        Mode::Live,
        "default".into(),
        TODAY.into(),
        dir.path().to_path_buf(),
        true,
    );
    let names: Vec<_> = app.entries.iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, vec!["..", "scans", "a.PNG", "b.pdf"]);
    app.current = 3;
    app.file_cursor.index = 3;
    assert_eq!(
        app.key(key(KeyCode::Enter)),
        vec![Request::Upload {
            index: 0,
            path: dir.path().join("b.pdf")
        }]
    );
    // The same file isn't queued twice.
    assert!(app.key(key(KeyCode::Enter)).is_empty());
}

#[test]
fn snapshot_overview() {
    let mut app = business();
    app.start();
    app.message(Msg::Overview(Ok(overview())));
    insta::assert_snapshot!(render(&mut app, 120, 34));
}

#[test]
fn snapshot_invoices_with_details() {
    let mut app = business();
    app.start();
    app.key(key(KeyCode::Char('2')));
    app.message(Msg::Documents(Ok(documents())));
    app.message(Msg::Document(
        documents()[0].id.clone(),
        Ok(Box::new(document())),
    ));
    insta::assert_snapshot!(render(&mut app, 120, 30));
}

#[test]
fn snapshot_invoices_on_a_narrow_terminal() {
    let mut app = business();
    app.start();
    app.key(key(KeyCode::Char('2')));
    app.message(Msg::Documents(Ok(documents())));
    app.message(Msg::Document(
        documents()[0].id.clone(),
        Ok(Box::new(document())),
    ));
    insta::assert_snapshot!(render(&mut app, 80, 30));
}

#[test]
fn snapshot_to_check() {
    let mut app = business();
    app.start();
    app.key(key(KeyCode::Char('3')));
    app.message(Msg::Review(Ok(review())));
    app.message(Msg::Document(
        review()[0].document_id.clone(),
        Ok(Box::new(document())),
    ));
    insta::assert_snapshot!(render(&mut app, 120, 30));
}

#[test]
fn snapshot_to_pay() {
    let mut app = household();
    app.start();
    app.message(Msg::ToPay(Ok(ToPayData {
        summary: summary(),
        unpaid: bills(),
    })));
    app.message(Msg::Bill(bills()[0].id.clone(), Ok(Box::new(bill()))));
    insta::assert_snapshot!(render(&mut app, 120, 30));
}

#[test]
fn snapshot_bills_by_month() {
    let mut app = household();
    app.start();
    app.key(key(KeyCode::Char('2')));
    app.message(Msg::Bills(
        november(),
        Ok(BillsData {
            bills: bills(),
            stats: stats(),
        }),
    ));
    app.message(Msg::Bill(bills()[0].id.clone(), Ok(Box::new(bill()))));
    insta::assert_snapshot!(render(&mut app, 120, 34));
}

#[test]
fn snapshot_help() {
    let mut app = business();
    app.start();
    app.message(Msg::Overview(Ok(overview())));
    app.key(key(KeyCode::Char('?')));
    insta::assert_snapshot!(render(&mut app, 100, 30));
}

#[test]
fn snapshot_loading_and_failure() {
    let mut app = business();
    app.start();
    let loading = render(&mut app, 80, 20);
    assert!(loading.contains("Loading your overview"));
    app.message(Msg::Overview(Err(
        "Couldn't reach Invoist. Check your connection and try again.".into(),
    )));
    insta::assert_snapshot!(render(&mut app, 80, 20));
}

#[test]
fn a_tiny_terminal_asks_for_more_room() {
    let mut app = business();
    let screen = render(&mut app, 50, 10);
    assert!(screen.contains("Make the window bigger"));
}

#[test]
fn snapshot_upload() {
    let dir = tempfile::tempdir().expect("temp dir");
    let folder = dir.path().join("invoices");
    std::fs::create_dir_all(folder.join("scans")).unwrap();
    for name in [
        "northwind-2041.pdf",
        "harbor-print-0193.pdf",
        "receipt-photo.jpg",
        "notes.txt",
        ".hidden.pdf",
    ] {
        std::fs::write(folder.join(name), b"made-up test file").unwrap();
    }
    let mut app = App::new(
        AccountKind::Business,
        Mode::Live,
        "default".into(),
        TODAY.into(),
        folder.clone(),
        true,
    );
    app.start();
    app.key(key(KeyCode::Char('4')));
    app.uploads = vec![
        UploadItem {
            path: folder.join("northwind-2041.pdf"),
            status: UploadStatus::Done {
                vendor: "Northwind Paper Co.".into(),
                total: "$1,204.20".into(),
                duplicate: false,
                checks: vec!["bank details changed".into()],
            },
        },
        UploadItem {
            path: folder.join("harbor-print-0193.pdf"),
            status: UploadStatus::Done {
                vendor: "Harbor Print".into(),
                total: "$318.00".into(),
                duplicate: true,
                checks: vec![],
            },
        },
        UploadItem {
            path: folder.join("receipt-photo.jpg"),
            status: UploadStatus::Reading,
        },
    ];
    let screen = render(&mut app, 120, 20);
    // The temporary folder's path changes on every run and every system:
    // the title keeps its end, which is always the folder's own name.
    let mut lines: Vec<String> = screen.lines().map(String::from).collect();
    let title = lines
        .iter_mut()
        .find(|l| l.contains("Choose files · "))
        .expect("a title");
    // Its length differs too, so everything up to the corner is replaced:
    // a long path fills the title, where a short one leaves a gap.
    let start = title.find("Choose files · ").unwrap() + "Choose files · ".len();
    let end = start + title[start..].find(" ─").expect("the title ends");
    assert!(title[start..end].ends_with("invoices"), "{title}");
    let corner = start + title[start..].find('╮').expect("the box has a corner");
    let width = title[start..corner].chars().count();
    title.replace_range(start..corner, &format!("{:─<width$}", "~/invoices "));
    let screen = lines.join("\n");
    insta::assert_snapshot!(screen);
}

#[test]
fn snapshot_webhooks() {
    let mut app = business();
    app.start();
    app.key(key(KeyCode::Char('5')));
    app.message(Msg::Webhooks(Ok(fixture(
        include_str!("../../tests/fixtures/webhooks.json"),
        None,
    ))));
    insta::assert_snapshot!(render(&mut app, 120, 16));
}

#[test]
fn words_are_joined_to_fit_without_leading_separators() {
    let words: Vec<String> = ["alpha", "beta", "gamma", "delta"]
        .map(String::from)
        .to_vec();
    assert_eq!(
        ui::join_to_width(&words, " · ", 14),
        vec!["alpha · beta", "gamma · delta"]
    );
    assert_eq!(
        ui::join_to_width(&words, " · ", 3),
        vec!["alpha", "beta", "gamma", "delta"]
    );
    assert!(ui::join_to_width(&[], " · ", 10).is_empty());
}

#[test]
fn long_paths_keep_their_end() {
    assert_eq!(ui::keep_end("~/invoices", 20), "~/invoices");
    assert_eq!(
        ui::keep_end("/var/folders/ab/T/scans/2026", 12),
        "…/scans/2026"
    );
    assert_eq!(
        ui::keep_end("/var/folders/ab/T/scans/2026", 12)
            .chars()
            .count(),
        12
    );
}

#[test]
fn the_selected_row_is_highlighted_evenly_across_its_width() {
    use ratatui::style::{Color, Modifier};
    let mut app = App::new(
        AccountKind::Business,
        Mode::Live,
        "default".into(),
        TODAY.into(),
        std::env::temp_dir(),
        false,
    );
    app.start();
    app.key(key(KeyCode::Char('2')));
    app.message(Msg::Documents(Ok(documents())));
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("test terminal");
    terminal.draw(|f| ui::draw(f, &mut app)).expect("draws");
    let buffer = terminal.backend().buffer();
    let y = (0..buffer.area.height)
        .find(|&y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains("Northwind")
        })
        .expect("the first invoice is on screen");
    let row: Vec<_> = (0..buffer.area.width)
        .map(|x| &buffer[(x, y)])
        .filter(|c| c.modifier.contains(Modifier::REVERSED))
        .collect();
    assert!(row.len() > 60, "most of the row is highlighted");
    // Every highlighted cell, the dimmed date and the yellow mark included,
    // uses the terminal's own colours, so no part is unreadable.
    assert!(
        row.iter()
            .all(|c| c.fg == Color::Reset && c.bg == Color::Reset)
    );
}
