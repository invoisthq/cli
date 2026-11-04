//! The terminal app's state, and how it changes.
//!
//! Nothing here touches the network or the terminal. A key press, a mouse
//! event or a result from the API goes in through `update`; requests for
//! the API come out; drawing reads the state. That keeps every screen and
//! every key sequence testable without a server.

use std::path::{Path, PathBuf};
use std::time::Instant;

use invoist::Month;
use invoist::models::{
    Bill, BillDetail, BillsStats, BillsSummary, DocumentDetail, DocumentListItem, HeldEmail,
    ReviewQueueListItem, StatsOverview, Usage, WebhookEndpointList,
};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use crate::config::{AccountKind, Mode};

/// Data the app is waiting for, has, or couldn't get.
#[derive(Debug, Clone, Default)]
pub enum Loadable<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Loadable<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Loadable::Ready(v) => Some(v),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Overview,
    Documents,
    Review,
    Upload,
    Webhooks,
    ToPay,
    Bills,
    Held,
}

impl Screen {
    pub fn title(self) -> &'static str {
        match self {
            Screen::Overview => "Overview",
            Screen::Documents => "Invoices",
            Screen::Review => "To check",
            Screen::Upload => "Upload",
            Screen::Webhooks => "Webhooks",
            Screen::ToPay => "To pay",
            Screen::Bills => "Bills",
            Screen::Held => "Waiting emails",
        }
    }

    pub fn for_kind(kind: AccountKind) -> Vec<Screen> {
        match kind {
            AccountKind::Business => vec![
                Screen::Overview,
                Screen::Documents,
                Screen::Review,
                Screen::Upload,
                Screen::Webhooks,
            ],
            AccountKind::Household => vec![Screen::ToPay, Screen::Bills, Screen::Held],
        }
    }
}

/// Everything the app asks the API for. The worker runs these through the SDK.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Overview,
    Documents {
        search: String,
        flagged: bool,
    },
    Document(String),
    Review,
    Resolve {
        id: String,
        dismiss: bool,
    },
    Webhooks,
    Upload {
        index: usize,
        path: PathBuf,
    },
    ToPay,
    Bills(Month),
    Bill(String),
    Held,
    MarkPaid {
        id: String,
        paid: bool,
    },
    OpenFile {
        id: String,
        filename: String,
        bill: bool,
    },
}

pub struct OverviewData {
    pub stats: StatsOverview,
    pub usage: Usage,
    pub latest: Vec<DocumentListItem>,
}

pub struct ToPayData {
    pub summary: BillsSummary,
    pub unpaid: Vec<Bill>,
}

pub struct BillsData {
    pub bills: Vec<Bill>,
    pub stats: BillsStats,
}

/// What the worker sends back.
pub enum Msg {
    Overview(Result<OverviewData, String>),
    Documents(Result<Vec<DocumentListItem>, String>),
    /// Boxed: a full invoice is large, and every message is as big as the
    /// largest one.
    Document(String, Result<Box<DocumentDetail>, String>),
    Review(Result<Vec<ReviewQueueListItem>, String>),
    Resolved(String, Result<(), String>),
    Webhooks(Result<WebhookEndpointList, String>),
    UploadStep(usize, UploadStatus),
    ToPay(Result<ToPayData, String>),
    Bills(Month, Result<BillsData, String>),
    Bill(String, Result<Box<BillDetail>, String>),
    Held(Result<Vec<HeldEmail>, String>),
    Marked(String, bool, Result<(), String>),
    FileOpened(Result<String, String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum UploadStatus {
    Waiting,
    Uploading,
    Reading,
    Done {
        vendor: String,
        total: String,
        duplicate: bool,
        checks: Vec<String>,
    },
    Failed(String),
}

pub struct UploadItem {
    pub path: PathBuf,
    pub status: UploadStatus,
}

pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub dir: bool,
}

/// A list's selection.
#[derive(Debug, Default, Clone, Copy)]
pub struct Cursor {
    pub index: usize,
}

impl Cursor {
    pub fn up(&mut self) {
        self.index = self.index.saturating_sub(1);
    }
    pub fn down(&mut self, len: usize) {
        if len > 0 {
            self.index = (self.index + 1).min(len - 1);
        }
    }
    pub fn clamp(&mut self, len: usize) {
        self.index = if len == 0 { 0 } else { self.index.min(len - 1) };
    }
    pub fn page(&mut self, len: usize, down: bool, step: usize) {
        if down {
            self.index = (self.index + step).min(len.saturating_sub(1));
        } else {
            self.index = self.index.saturating_sub(step);
        }
    }
}

pub enum Overlay {
    None,
    Help,
    Palette { query: String, cursor: Cursor },
    Confirm { question: String, then: Request },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Good,
    Bad,
}

pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub at: Instant,
}

/// Where things were drawn, so a mouse click can be matched to them.
#[derive(Default)]
pub struct HitAreas {
    pub tabs: Vec<(Rect, usize)>,
    /// The list's area and the index of its first visible row.
    pub list: Option<(Rect, usize)>,
}

/// A command in the palette: what it's called, and what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Go(Screen),
    Refresh,
    Search,
    ToggleFlagged,
    OpenFile,
    Help,
    Quit,
}

pub struct App {
    pub kind: AccountKind,
    pub mode: Mode,
    pub profile: String,
    pub theme_plain: bool,
    pub screens: Vec<Screen>,
    pub current: usize,
    pub overlay: Overlay,
    pub toast: Option<Toast>,
    pub tick: u64,
    pub quit: bool,
    pub hits: HitAreas,

    pub overview: Loadable<OverviewData>,

    pub documents: Loadable<Vec<DocumentListItem>>,
    pub doc_cursor: Cursor,
    pub search: String,
    pub search_input: Option<String>,
    pub flagged_only: bool,
    pub details: std::collections::HashMap<String, Loadable<DocumentDetail>>,

    pub review: Loadable<Vec<ReviewQueueListItem>>,
    pub review_cursor: Cursor,
    pub resolving: Option<String>,

    pub cwd: PathBuf,
    pub entries: Vec<FileEntry>,
    pub file_cursor: Cursor,
    pub uploads: Vec<UploadItem>,

    pub webhooks: Loadable<WebhookEndpointList>,
    pub webhook_cursor: Cursor,

    pub to_pay: Loadable<ToPayData>,
    pub pay_cursor: Cursor,
    pub bill_details: std::collections::HashMap<String, Loadable<BillDetail>>,

    /// Today, `YYYY-MM-DD`, to tell overdue bills apart.
    pub today: String,
    pub month: Month,
    pub bills: Loadable<BillsData>,
    pub bill_cursor: Cursor,

    pub held: Loadable<Vec<HeldEmail>>,
    pub held_cursor: Cursor,
}

pub const PAGE_STEP: usize = 10;

impl App {
    pub fn new(
        kind: AccountKind,
        mode: Mode,
        profile: String,
        today: String,
        cwd: PathBuf,
        theme_plain: bool,
    ) -> App {
        let month = today
            .get(..7)
            .and_then(|m| m.parse::<Month>().ok())
            .unwrap_or_else(|| Month::new(2026, 1).expect("a valid month"));
        let mut app = App {
            kind,
            mode,
            profile,
            theme_plain,
            screens: Screen::for_kind(kind),
            current: 0,
            overlay: Overlay::None,
            toast: None,
            tick: 0,
            quit: false,
            hits: HitAreas::default(),
            overview: Loadable::Idle,
            documents: Loadable::Idle,
            doc_cursor: Cursor::default(),
            search: String::new(),
            search_input: None,
            flagged_only: false,
            details: Default::default(),
            review: Loadable::Idle,
            review_cursor: Cursor::default(),
            resolving: None,
            cwd,
            entries: Vec::new(),
            file_cursor: Cursor::default(),
            uploads: Vec::new(),
            webhooks: Loadable::Idle,
            webhook_cursor: Cursor::default(),
            to_pay: Loadable::Idle,
            pay_cursor: Cursor::default(),
            bill_details: Default::default(),
            today,
            month,
            bills: Loadable::Idle,
            bill_cursor: Cursor::default(),
            held: Loadable::Idle,
            held_cursor: Cursor::default(),
        };
        app.read_dir();
        app
    }

    pub fn screen(&self) -> Screen {
        self.screens[self.current]
    }

    /// The requests to make when the app opens.
    pub fn start(&mut self) -> Vec<Request> {
        self.enter_screen(false)
    }

    /// Loads a screen's data the first time it's shown, or again with `force`.
    fn enter_screen(&mut self, force: bool) -> Vec<Request> {
        let mut reqs = Vec::new();
        match self.screen() {
            Screen::Overview => {
                if force || matches!(self.overview, Loadable::Idle) {
                    self.overview = Loadable::Loading;
                    reqs.push(Request::Overview);
                }
            }
            Screen::Documents => {
                if force || matches!(self.documents, Loadable::Idle) {
                    self.documents = Loadable::Loading;
                    reqs.push(Request::Documents {
                        search: self.search.clone(),
                        flagged: self.flagged_only,
                    });
                }
            }
            Screen::Review => {
                if force || matches!(self.review, Loadable::Idle) {
                    self.review = Loadable::Loading;
                    reqs.push(Request::Review);
                }
            }
            Screen::Upload => {
                if force {
                    self.read_dir();
                }
            }
            Screen::Webhooks => {
                if force || matches!(self.webhooks, Loadable::Idle) {
                    self.webhooks = Loadable::Loading;
                    reqs.push(Request::Webhooks);
                }
            }
            Screen::ToPay => {
                if force || matches!(self.to_pay, Loadable::Idle) {
                    self.to_pay = Loadable::Loading;
                    reqs.push(Request::ToPay);
                }
            }
            Screen::Bills => {
                if force || matches!(self.bills, Loadable::Idle) {
                    self.bills = Loadable::Loading;
                    reqs.push(Request::Bills(self.month));
                }
            }
            Screen::Held => {
                if force || matches!(self.held, Loadable::Idle) {
                    self.held = Loadable::Loading;
                    reqs.push(Request::Held);
                }
            }
        }
        reqs.extend(self.detail_request());
        reqs
    }

    /// The selected invoice or bill's details, when the screen shows them
    /// and they aren't loaded yet.
    fn detail_request(&mut self) -> Option<Request> {
        match self.screen() {
            Screen::Documents | Screen::Review => {
                let id = self.selected_document_id()?;
                if self.details.contains_key(&id) {
                    return None;
                }
                self.details.insert(id.clone(), Loadable::Loading);
                Some(Request::Document(id))
            }
            Screen::ToPay | Screen::Bills => {
                let id = self.selected_bill()?.id.clone();
                if self.bill_details.contains_key(&id) {
                    return None;
                }
                self.bill_details.insert(id.clone(), Loadable::Loading);
                Some(Request::Bill(id))
            }
            _ => None,
        }
    }

    pub fn selected_document_id(&self) -> Option<String> {
        match self.screen() {
            Screen::Documents => self
                .documents
                .ready()?
                .get(self.doc_cursor.index)
                .map(|d| d.id.clone()),
            Screen::Review => self
                .review
                .ready()?
                .get(self.review_cursor.index)
                .map(|i| i.document_id.clone()),
            _ => None,
        }
    }

    /// The unpaid bills on To pay, overdue and soonest first.
    pub fn unpaid_sorted(&self) -> Vec<&Bill> {
        let Some(data) = self.to_pay.ready() else {
            return Vec::new();
        };
        let mut bills: Vec<&Bill> = data.unpaid.iter().collect();
        bills.sort_by(|a, b| {
            a.due_date
                .as_deref()
                .unwrap_or("9999")
                .cmp(b.due_date.as_deref().unwrap_or("9999"))
        });
        bills
    }

    pub fn selected_bill(&self) -> Option<&Bill> {
        match self.screen() {
            Screen::ToPay => self.unpaid_sorted().get(self.pay_cursor.index).copied(),
            Screen::Bills => self.bills.ready()?.bills.get(self.bill_cursor.index),
            _ => None,
        }
    }

    fn list_len(&self) -> usize {
        match self.screen() {
            Screen::Overview => 0,
            Screen::Documents => self.documents.ready().map_or(0, Vec::len),
            Screen::Review => self.review.ready().map_or(0, Vec::len),
            Screen::Upload => self.entries.len(),
            Screen::Webhooks => self.webhooks.ready().map_or(0, |w| w.endpoints.len()),
            Screen::ToPay => self.unpaid_sorted().len(),
            Screen::Bills => self.bills.ready().map_or(0, |b| b.bills.len()),
            Screen::Held => self.held.ready().map_or(0, Vec::len),
        }
    }

    fn cursor_mut(&mut self) -> Option<&mut Cursor> {
        Some(match self.screen() {
            Screen::Overview => return None,
            Screen::Documents => &mut self.doc_cursor,
            Screen::Review => &mut self.review_cursor,
            Screen::Upload => &mut self.file_cursor,
            Screen::Webhooks => &mut self.webhook_cursor,
            Screen::ToPay => &mut self.pay_cursor,
            Screen::Bills => &mut self.bill_cursor,
            Screen::Held => &mut self.held_cursor,
        })
    }

    fn move_cursor(&mut self, f: impl FnOnce(&mut Cursor, usize)) -> Vec<Request> {
        let len = self.list_len();
        if let Some(c) = self.cursor_mut() {
            f(c, len);
        }
        self.detail_request().into_iter().collect()
    }

    fn go(&mut self, index: usize) -> Vec<Request> {
        if index < self.screens.len() {
            self.current = index;
            self.search_input = None;
            return self.enter_screen(false);
        }
        Vec::new()
    }

    pub fn notify(&mut self, text: impl Into<String>, kind: ToastKind) {
        self.toast = Some(Toast {
            text: text.into(),
            kind,
            at: Instant::now(),
        });
    }

    /// Reads the upload screen's folder: folders first, then the files
    /// Invoist can read.
    pub fn read_dir(&mut self) {
        let mut entries = Vec::new();
        if let Some(parent) = self.cwd.parent() {
            entries.push(FileEntry {
                name: "..".into(),
                path: parent.to_path_buf(),
                dir: true,
            });
        }
        if let Ok(read) = std::fs::read_dir(&self.cwd) {
            let mut found: Vec<FileEntry> = read
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') {
                        return None;
                    }
                    let path = e.path();
                    let dir = path.is_dir();
                    (dir || readable(&path)).then_some(FileEntry { name, path, dir })
                })
                .collect();
            found.sort_by(|a, b| {
                b.dir
                    .cmp(&a.dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            entries.extend(found);
        }
        self.entries = entries;
        self.file_cursor.clamp(self.entries.len());
    }

    pub fn actions(&self) -> Vec<(String, Action)> {
        let mut list: Vec<(String, Action)> = self
            .screens
            .iter()
            .map(|s| (format!("Go to {}", s.title()), Action::Go(*s)))
            .collect();
        list.push(("Refresh this screen".into(), Action::Refresh));
        if self.kind == AccountKind::Business {
            list.push(("Search invoices".into(), Action::Search));
            list.push(("Show only invoices to check".into(), Action::ToggleFlagged));
        }
        list.push(("Open the original file".into(), Action::OpenFile));
        list.push(("Show keys".into(), Action::Help));
        list.push(("Quit".into(), Action::Quit));
        list
    }

    /// The palette's actions matching `query`, best first.
    pub fn matching_actions(&self, query: &str) -> Vec<(String, Action)> {
        let mut scored: Vec<(i64, (String, Action))> = self
            .actions()
            .into_iter()
            .filter_map(|(name, a)| fuzzy_score(query, &name).map(|s| (s, (name, a))))
            .collect();
        scored.sort_by_key(|a| std::cmp::Reverse(a.0));
        scored.into_iter().map(|(_, a)| a).collect()
    }

    fn run_action(&mut self, action: Action) -> Vec<Request> {
        match action {
            Action::Go(screen) => {
                let index = self.screens.iter().position(|s| *s == screen).unwrap_or(0);
                self.go(index)
            }
            Action::Refresh => self.refresh(),
            Action::Search => {
                if let Some(i) = self.screens.iter().position(|s| *s == Screen::Documents) {
                    let mut reqs = self.go(i);
                    self.search_input = Some(self.search.clone());
                    reqs.retain(|r| !matches!(r, Request::Document(_)));
                    return reqs;
                }
                Vec::new()
            }
            Action::ToggleFlagged => {
                if let Some(i) = self.screens.iter().position(|s| *s == Screen::Documents) {
                    self.current = i;
                    self.flagged_only = !self.flagged_only;
                    self.doc_cursor = Cursor::default();
                    return self.enter_screen(true);
                }
                Vec::new()
            }
            Action::OpenFile => self.open_file(),
            Action::Help => {
                self.overlay = Overlay::Help;
                Vec::new()
            }
            Action::Quit => {
                self.quit = true;
                Vec::new()
            }
        }
    }

    fn refresh(&mut self) -> Vec<Request> {
        match self.screen() {
            Screen::Documents | Screen::Review => self.details.clear(),
            Screen::ToPay | Screen::Bills => self.bill_details.clear(),
            _ => {}
        }
        self.enter_screen(true)
    }

    fn open_file(&mut self) -> Vec<Request> {
        match self.screen() {
            Screen::Documents => {
                let Some(d) = self
                    .documents
                    .ready()
                    .and_then(|l| l.get(self.doc_cursor.index))
                else {
                    return Vec::new();
                };
                vec![Request::OpenFile {
                    id: d.id.clone(),
                    filename: d.original_filename.clone(),
                    bill: false,
                }]
            }
            Screen::Review => {
                let Some(i) = self
                    .review
                    .ready()
                    .and_then(|l| l.get(self.review_cursor.index))
                else {
                    return Vec::new();
                };
                vec![Request::OpenFile {
                    id: i.document_id.clone(),
                    filename: i.original_filename.clone(),
                    bill: false,
                }]
            }
            Screen::ToPay | Screen::Bills => {
                let Some(b) = self.selected_bill() else {
                    return Vec::new();
                };
                vec![Request::OpenFile {
                    id: b.id.clone(),
                    filename: b.original_filename.clone(),
                    bill: true,
                }]
            }
            _ => Vec::new(),
        }
    }

    /// A key press: what changes, and what to ask the API.
    pub fn key(&mut self, key: KeyEvent) -> Vec<Request> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.quit = true;
            return Vec::new();
        }

        // Overlays take keys first.
        match &mut self.overlay {
            Overlay::Help => {
                self.overlay = Overlay::None;
                return Vec::new();
            }
            Overlay::Confirm { then, .. } => {
                return match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                        let then = then.clone();
                        self.overlay = Overlay::None;
                        vec![then]
                    }
                    _ => {
                        self.overlay = Overlay::None;
                        Vec::new()
                    }
                };
            }
            Overlay::Palette { query, cursor } => {
                let (mut query, mut index) = (query.clone(), cursor.index);
                match key.code {
                    KeyCode::Esc => {
                        self.overlay = Overlay::None;
                        return Vec::new();
                    }
                    KeyCode::Up => index = index.saturating_sub(1),
                    KeyCode::Down => {
                        let len = self.matching_actions(&query).len();
                        index = (index + 1).min(len.saturating_sub(1));
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        index = 0;
                    }
                    KeyCode::Char(c) if !ctrl => {
                        query.push(c);
                        index = 0;
                    }
                    KeyCode::Enter => {
                        let picked = self.matching_actions(&query).get(index).map(|(_, a)| *a);
                        self.overlay = Overlay::None;
                        return picked.map(|a| self.run_action(a)).unwrap_or_default();
                    }
                    _ => {}
                }
                self.overlay = Overlay::Palette {
                    query,
                    cursor: Cursor { index },
                };
                return Vec::new();
            }
            Overlay::None => {}
        }

        // Typing a search.
        if let Some(input) = &mut self.search_input {
            match key.code {
                KeyCode::Esc => self.search_input = None,
                KeyCode::Enter => {
                    self.search = input.trim().to_string();
                    self.search_input = None;
                    self.doc_cursor = Cursor::default();
                    return self.enter_screen(true);
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) if !ctrl => input.push(c),
                _ => {}
            }
            return Vec::new();
        }

        if (ctrl && key.code == KeyCode::Char('k')) || key.code == KeyCode::Char(':') {
            self.overlay = Overlay::Palette {
                query: String::new(),
                cursor: Cursor::default(),
            };
            return Vec::new();
        }

        match key.code {
            KeyCode::Char('q') => {
                self.quit = true;
                Vec::new()
            }
            KeyCode::Char('?') => {
                self.overlay = Overlay::Help;
                Vec::new()
            }
            KeyCode::Tab => {
                let next = (self.current + 1) % self.screens.len();
                self.go(next)
            }
            KeyCode::BackTab => {
                let prev = (self.current + self.screens.len() - 1) % self.screens.len();
                self.go(prev)
            }
            KeyCode::Char(c @ '1'..='9') => self.go(c as usize - '1' as usize),
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(|c, _| c.up()),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(|c, len| c.down(len)),
            KeyCode::PageUp => self.move_cursor(|c, len| c.page(len, false, PAGE_STEP)),
            KeyCode::PageDown => self.move_cursor(|c, len| c.page(len, true, PAGE_STEP)),
            KeyCode::Home | KeyCode::Char('g') => self.move_cursor(|c, _| c.index = 0),
            KeyCode::End | KeyCode::Char('G') => {
                self.move_cursor(|c, len| c.index = len.saturating_sub(1))
            }
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('o') => self.open_file(),
            _ => self.screen_key(key),
        }
    }

    /// Keys that mean something on one screen only.
    fn screen_key(&mut self, key: KeyEvent) -> Vec<Request> {
        match (self.screen(), key.code) {
            (Screen::Documents, KeyCode::Char('/')) => {
                self.search_input = Some(self.search.clone());
                Vec::new()
            }
            (Screen::Documents, KeyCode::Char('f')) => self.run_action(Action::ToggleFlagged),
            (Screen::Documents, KeyCode::Esc) if !self.search.is_empty() => {
                self.search.clear();
                self.doc_cursor = Cursor::default();
                self.enter_screen(true)
            }
            (Screen::Review, KeyCode::Char('c') | KeyCode::Enter) => self.resolve(false),
            (Screen::Review, KeyCode::Char('d')) => self.resolve(true),
            (Screen::Upload, KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')) => {
                self.upload_enter()
            }
            (Screen::Upload, KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h')) => {
                if let Some(parent) = self.cwd.parent() {
                    self.cwd = parent.to_path_buf();
                    self.file_cursor = Cursor::default();
                    self.read_dir();
                }
                Vec::new()
            }
            (Screen::ToPay | Screen::Bills, KeyCode::Char('p')) => self.ask_mark(true),
            (Screen::ToPay | Screen::Bills, KeyCode::Char('u')) => self.ask_mark(false),
            (Screen::Bills, KeyCode::Char('[') | KeyCode::Left) => self.change_month(-1),
            (Screen::Bills, KeyCode::Char(']') | KeyCode::Right) => self.change_month(1),
            _ => Vec::new(),
        }
    }

    fn resolve(&mut self, dismiss: bool) -> Vec<Request> {
        if self.resolving.is_some() {
            return Vec::new();
        }
        let Some(item) = self
            .review
            .ready()
            .and_then(|l| l.get(self.review_cursor.index))
        else {
            return Vec::new();
        };
        let id = item.id.clone();
        self.resolving = Some(id.clone());
        vec![Request::Resolve { id, dismiss }]
    }

    fn upload_enter(&mut self) -> Vec<Request> {
        let Some(entry) = self.entries.get(self.file_cursor.index) else {
            return Vec::new();
        };
        if entry.dir {
            self.cwd = entry.path.clone();
            self.file_cursor = Cursor::default();
            self.read_dir();
            return Vec::new();
        }
        let path = entry.path.clone();
        if self
            .uploads
            .iter()
            .any(|u| u.path == path && !matches!(u.status, UploadStatus::Failed(_)))
        {
            self.notify("That file is already in the list.", ToastKind::Bad);
            return Vec::new();
        }
        let index = self.uploads.len();
        self.uploads.push(UploadItem {
            path: path.clone(),
            status: UploadStatus::Waiting,
        });
        vec![Request::Upload { index, path }]
    }

    fn ask_mark(&mut self, paid: bool) -> Vec<Request> {
        let Some(bill) = self.selected_bill() else {
            return Vec::new();
        };
        let is_paid = bill.bill.paid_at.is_some();
        if paid == is_paid {
            self.notify(
                if paid {
                    "That bill is already marked paid."
                } else {
                    "That bill isn't marked paid."
                },
                ToastKind::Bad,
            );
            return Vec::new();
        }
        let vendor = bill
            .vendor_name
            .clone()
            .unwrap_or_else(|| bill.original_filename.clone());
        let amount = crate::output::money(bill.total_cents, bill.currency.as_deref());
        self.overlay = Overlay::Confirm {
            question: if paid {
                format!("Mark the {vendor} bill ({amount}) as paid?")
            } else {
                format!("Mark the {vendor} bill ({amount}) as not paid?")
            },
            then: Request::MarkPaid {
                id: bill.id.clone(),
                paid,
            },
        };
        Vec::new()
    }

    fn change_month(&mut self, by: i32) -> Vec<Request> {
        let total = self.month.year() as i32 * 12 + (self.month.month() as i32 - 1) + by;
        if let Ok(m) = Month::new((total / 12) as u16, (total % 12 + 1) as u8) {
            self.month = m;
            self.bill_cursor = Cursor::default();
            self.bills = Loadable::Loading;
            return vec![Request::Bills(m)];
        }
        Vec::new()
    }

    /// A mouse event: click a tab or a row, or scroll a list.
    pub fn mouse(&mut self, event: MouseEvent) -> Vec<Request> {
        if !matches!(self.overlay, Overlay::None) {
            return Vec::new();
        }
        let at = Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(_, index)) = self.hits.tabs.iter().find(|(r, _)| r.contains(at)) {
                    return self.go(index);
                }
                if let Some((rect, first)) = self.hits.list
                    && rect.contains(at)
                {
                    let row = first + (at.y - rect.y) as usize;
                    if row < self.list_len() {
                        return self.move_cursor(|c, _| c.index = row);
                    }
                }
                Vec::new()
            }
            MouseEventKind::ScrollDown => self.move_cursor(|c, len| c.down(len)),
            MouseEventKind::ScrollUp => self.move_cursor(|c, _| c.up()),
            _ => Vec::new(),
        }
    }

    /// A result from the API.
    pub fn message(&mut self, msg: Msg) -> Vec<Request> {
        let mut reqs = Vec::new();
        match msg {
            Msg::Overview(r) => self.overview = loaded(r),
            Msg::Documents(r) => {
                self.documents = loaded(r);
                self.doc_cursor.clamp(self.list_len_of(Screen::Documents));
                if self.screen() == Screen::Documents {
                    reqs.extend(self.detail_request());
                }
            }
            Msg::Document(id, r) => {
                self.details.insert(id, loaded(r.map(|d| *d)));
            }
            Msg::Review(r) => {
                self.review = loaded(r);
                self.review_cursor.clamp(self.list_len_of(Screen::Review));
                if self.screen() == Screen::Review {
                    reqs.extend(self.detail_request());
                }
            }
            Msg::Resolved(id, r) => {
                self.resolving = None;
                match r {
                    Ok(()) => {
                        if let Loadable::Ready(list) = &mut self.review {
                            list.retain(|i| i.id != id);
                        }
                        self.review_cursor.clamp(self.list_len_of(Screen::Review));
                        self.notify("Marked as checked.", ToastKind::Good);
                        // Counts on the overview and the invoice list are now stale.
                        self.overview = Loadable::Idle;
                        self.documents = Loadable::Idle;
                        if self.screen() == Screen::Review {
                            reqs.extend(self.detail_request());
                        }
                    }
                    Err(e) => self.notify(e, ToastKind::Bad),
                }
            }
            Msg::Webhooks(r) => self.webhooks = loaded(r),
            Msg::UploadStep(index, status) => {
                let finished = matches!(status, UploadStatus::Done { .. });
                if let Some(item) = self.uploads.get_mut(index) {
                    item.status = status;
                }
                if finished {
                    self.overview = Loadable::Idle;
                    self.documents = Loadable::Idle;
                    self.review = Loadable::Idle;
                }
            }
            Msg::ToPay(r) => {
                self.to_pay = loaded(r);
                self.pay_cursor.clamp(self.list_len_of(Screen::ToPay));
                if self.screen() == Screen::ToPay {
                    reqs.extend(self.detail_request());
                }
            }
            Msg::Bills(month, r) => {
                if month == self.month {
                    self.bills = loaded(r);
                    self.bill_cursor.clamp(self.list_len_of(Screen::Bills));
                    if self.screen() == Screen::Bills {
                        reqs.extend(self.detail_request());
                    }
                }
            }
            Msg::Bill(id, r) => {
                self.bill_details.insert(id, loaded(r.map(|b| *b)));
            }
            Msg::Held(r) => self.held = loaded(r),
            Msg::Marked(id, paid, r) => match r {
                Ok(()) => {
                    self.notify(
                        if paid {
                            "Marked as paid."
                        } else {
                            "Marked as not paid."
                        },
                        ToastKind::Good,
                    );
                    self.bill_details.remove(&id);
                    self.to_pay = Loadable::Idle;
                    self.bills = Loadable::Idle;
                    reqs.extend(self.enter_screen(true));
                }
                Err(e) => self.notify(e, ToastKind::Bad),
            },
            Msg::FileOpened(r) => match r {
                Ok(name) => self.notify(format!("Opened {name}"), ToastKind::Good),
                Err(e) => self.notify(e, ToastKind::Bad),
            },
        }
        reqs
    }

    fn list_len_of(&self, screen: Screen) -> usize {
        match screen {
            Screen::Documents => self.documents.ready().map_or(0, Vec::len),
            Screen::Review => self.review.ready().map_or(0, Vec::len),
            Screen::ToPay => self.to_pay.ready().map_or(0, |d| d.unpaid.len()),
            Screen::Bills => self.bills.ready().map_or(0, |b| b.bills.len()),
            _ => 0,
        }
    }

    /// Called about ten times a second: moves the spinner, clears old toasts.
    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self
            .toast
            .as_ref()
            .is_some_and(|t| t.at.elapsed().as_secs() >= 4)
        {
            self.toast = None;
        }
    }
}

fn loaded<T>(r: Result<T, String>) -> Loadable<T> {
    match r {
        Ok(v) => Loadable::Ready(v),
        Err(e) => Loadable::Failed(e),
    }
}

pub fn readable(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("pdf" | "png" | "jpg" | "jpeg")
    )
}

/// How well `query` matches `text`: every query letter must appear in order.
/// Matches at the start of words and runs of letters score higher. `None`
/// when it doesn't match at all.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i64> {
    let query = query.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let text_l = text.to_lowercase();
    let chars: Vec<char> = text_l.chars().collect();
    let mut score = 0i64;
    let mut at = 0usize;
    let mut last: Option<usize> = None;
    for q in query.chars().filter(|c| !c.is_whitespace()) {
        let found = (at..chars.len()).find(|&i| chars[i] == q)?;
        score += 1;
        if found == 0 || chars[found - 1] == ' ' {
            score += 8;
        }
        if last.is_some_and(|l| l + 1 == found) {
            score += 5;
        }
        last = Some(found);
        at = found + 1;
    }
    Some(score * 100 - chars.len() as i64)
}
