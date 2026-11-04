//! Drawing the terminal app. Reads the state; never changes it, except to
//! remember where tabs and lists were drawn so a mouse click can find them.

use invoist::models::{Bill, BillDetail, DocumentDetail, ExtractionStatus, ReviewStatus};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Bar, BarChart, BarGroup, Block, BorderType, Borders, Cell, Clear, Padding, Paragraph, Row,
    Sparkline, Table, Wrap,
};

use crate::config::{AccountKind, Mode};
use crate::output::{capitalise, count, day, money, reason_label, short_id, status_label};
use crate::tui::app::{App, Loadable, Overlay, Screen, ToastKind, UploadStatus};
use crate::tui::theme::Theme;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
pub const MIN_WIDTH: u16 = 64;
pub const MIN_HEIGHT: u16 = 16;

pub fn draw(f: &mut Frame<'_>, app: &mut App) {
    let theme = Theme::new(app.theme_plain);
    let area = f.area();
    app.hits.tabs.clear();
    app.hits.list = None;

    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let msg = Paragraph::new(Text::from(vec![
            Line::styled("Make the window bigger", theme.strong()),
            Line::styled(
                format!("Invoist needs at least {MIN_WIDTH} × {MIN_HEIGHT}."),
                theme.dim(),
            ),
        ]))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
        let [_, middle, _] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(2),
            Constraint::Fill(1),
        ])
        .areas(area);
        f.render_widget(msg, middle);
        return;
    }

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(f, app, &theme, header);
    let body = body.inner(ratatui::layout::Margin::new(1, 0));
    match app.screen() {
        Screen::Overview => overview(f, app, &theme, body),
        Screen::Documents => documents(f, app, &theme, body),
        Screen::Review => review(f, app, &theme, body),
        Screen::Upload => upload(f, app, &theme, body),
        Screen::Webhooks => webhooks(f, app, &theme, body),
        Screen::ToPay => to_pay(f, app, &theme, body),
        Screen::Bills => bills(f, app, &theme, body),
        Screen::Held => held(f, app, &theme, body),
    }
    draw_footer(f, app, &theme, footer);

    match &app.overlay {
        Overlay::None => {}
        Overlay::Help => help(f, app, &theme, area),
        Overlay::Palette { query, cursor } => palette(f, app, &theme, area, query, cursor.index),
        Overlay::Confirm { question, .. } => confirm(f, &theme, area, question),
    }
}

fn spinner(app: &App) -> &'static str {
    SPINNER[(app.tick as usize) % SPINNER.len()]
}

fn draw_header(f: &mut Frame<'_>, app: &mut App, theme: &Theme, area: Rect) {
    let [line, rule] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let mut x = line.x + 1;
    let brand = " ◐ Invoist ";
    f.render_widget(
        Span::styled(brand, theme.accent()),
        Rect::new(x, line.y, brand.chars().count() as u16, 1),
    );
    x += brand.chars().count() as u16 + 2;
    let labels: Vec<String> = app
        .screens
        .iter()
        .enumerate()
        .map(|(i, s)| format!(" {} {} ", i + 1, s.title()))
        .collect();
    let tabs_width: u16 = labels.iter().map(|l| l.chars().count() as u16 + 1).sum();
    let account = format!(
        "{} · {}",
        app.profile,
        if app.kind == AccountKind::Business {
            "business"
        } else {
            "household"
        }
    );
    let right_width = account.chars().count() as u16 + if app.mode == Mode::Test { 8 } else { 1 };
    // Tabs come first; the account label shows only when there's room for both.
    let show_account = x + tabs_width + right_width < line.right();
    for (i, label) in labels.into_iter().enumerate() {
        let w = label.chars().count() as u16;
        if x + w > line.right() {
            break;
        }
        let style = if i == app.current {
            theme.selected()
        } else {
            theme.dim()
        };
        let rect = Rect::new(x, line.y, w, 1);
        f.render_widget(Span::styled(label, style), rect);
        app.hits.tabs.push((rect, i));
        x += w + 1;
    }
    let mut right = vec![Span::styled(
        if show_account { account } else { String::new() },
        theme.dim(),
    )];
    if app.mode == Mode::Test {
        right.push(Span::raw(" "));
        right.push(Span::styled(" TEST ", theme.badge()));
    }
    right.push(Span::raw(" "));
    f.render_widget(
        Paragraph::new(Line::from(right)).alignment(Alignment::Right),
        line,
    );
    f.render_widget(
        Paragraph::new("─".repeat(rule.width as usize)).style(theme.border()),
        rule,
    );
}

fn draw_footer(f: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect) {
    if let Some(toast) = &app.toast {
        let (mark, style) = match toast.kind {
            ToastKind::Good => ("✓", theme.good()),
            ToastKind::Bad => ("!", theme.bad()),
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("{mark} {}", toast.text), style),
            ])),
            area,
        );
        return;
    }
    let hints: &[(&str, &str)] = match app.screen() {
        Screen::Documents if app.search_input.is_some() => {
            &[("enter", "search"), ("esc", "cancel")]
        }
        Screen::Documents => &[
            ("↑↓", "move"),
            ("/", "search"),
            ("f", "to check only"),
            ("o", "open file"),
            ("r", "refresh"),
        ],
        Screen::Review => &[
            ("↑↓", "move"),
            ("c", "checked"),
            ("d", "not a problem"),
            ("o", "open file"),
        ],
        Screen::Upload => &[
            ("↑↓", "move"),
            ("enter", "open folder or upload"),
            ("←", "up a folder"),
        ],
        Screen::ToPay => &[("↑↓", "move"), ("p", "mark paid"), ("o", "open bill")],
        Screen::Bills => &[
            ("←→", "month"),
            ("p", "paid"),
            ("u", "not paid"),
            ("o", "open bill"),
        ],
        _ => &[("↑↓", "move"), ("r", "refresh")],
    };
    let mut spans = vec![Span::raw(" ")];
    for (key, what) in hints.iter().chain(
        [
            ("tab", "screens"),
            (":", "commands"),
            ("?", "keys"),
            ("q", "quit"),
        ]
        .iter(),
    ) {
        spans.push(Span::styled(*key, theme.strong()));
        spans.push(Span::styled(format!(" {what}   "), theme.dim()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn panel<'a>(title: impl Into<String>, theme: &Theme, focused: bool) -> Block<'a> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused {
            theme.border_focus()
        } else {
            theme.border()
        })
        .title(Span::styled(
            format!(" {} ", title.into()),
            if focused {
                theme.accent()
            } else {
                theme.strong()
            },
        ))
        .padding(Padding::horizontal(1))
}

/// A loading spinner, an error with how to retry, or nothing (the data is there).
fn status_block<T>(
    f: &mut Frame<'_>,
    app: &App,
    theme: &Theme,
    area: Rect,
    data: &Loadable<T>,
    what: &str,
) -> bool {
    match data {
        Loadable::Ready(_) => false,
        Loadable::Failed(e) => {
            let text = Text::from(vec![
                Line::styled(
                    format!("Couldn't load {what}"),
                    theme.bad().add_modifier(Modifier::BOLD),
                ),
                Line::raw(""),
                Line::raw(e.clone()),
                Line::raw(""),
                Line::from(vec![
                    Span::styled("r", theme.strong()),
                    Span::styled(" to try again", theme.dim()),
                ]),
            ]);
            f.render_widget(
                Paragraph::new(text)
                    .wrap(Wrap { trim: true })
                    .block(Block::new().padding(Padding::uniform(1))),
                area,
            );
            true
        }
        Loadable::Idle | Loadable::Loading => {
            let line = Line::from(vec![
                Span::styled(spinner(app), theme.accent()),
                Span::styled(format!(" Loading {what}…"), theme.dim()),
            ]);
            f.render_widget(
                Paragraph::new(line).block(Block::new().padding(Padding::uniform(1))),
                area,
            );
            true
        }
    }
}

/// Which row is first on screen, so the selected one is always visible.
fn scroll_offset(selected: usize, height: usize) -> usize {
    if height == 0 || selected < height {
        0
    } else {
        selected + 1 - height
    }
}

/// A list as a table, with the selected row highlighted and its position
/// remembered for the mouse.
#[allow(clippy::too_many_arguments)]
fn list_table(
    f: &mut Frame<'_>,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    header: &[&str],
    widths: &[Constraint],
    rows: Vec<Vec<Cell<'static>>>,
    selected: usize,
) {
    let body_height = area.height.saturating_sub(1) as usize;
    let offset = scroll_offset(selected, body_height);
    let header = Row::new(
        header
            .iter()
            .map(|h| Cell::from(Span::styled(h.to_uppercase(), theme.dim()))),
    );
    let rows: Vec<Row<'_>> = rows
        .into_iter()
        .enumerate()
        .skip(offset)
        .take(body_height)
        .map(|(_, cells)| Row::new(cells))
        .collect();
    f.render_widget(
        Table::new(rows, widths.to_vec())
            .header(header)
            .column_spacing(2),
        area,
    );
    let rows_area = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
    // The selected row is styled over its whole width after drawing, so
    // the colours of its parts don't show through the highlight.
    if let Some(line) = selected.checked_sub(offset).filter(|&l| l < body_height) {
        let row = Rect::new(rows_area.x, rows_area.y + line as u16, rows_area.width, 1);
        f.buffer_mut().set_style(row, theme.selected());
    }
    app.hits.list = Some((rows_area, offset));
}

fn split_detail(area: Rect) -> (Rect, Rect) {
    if area.width >= 110 {
        let [l, r] = Layout::horizontal([Constraint::Percentage(56), Constraint::Percentage(44)])
            .spacing(1)
            .areas(area);
        (l, r)
    } else {
        let [t, b] =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(area);
        (t, b)
    }
}

// ── Business screens ────────────────────────────────────────────────────────

fn overview(f: &mut Frame<'_>, app: &mut App, theme: &Theme, area: Rect) {
    if status_block(f, app, theme, area, &app.overview, "your overview") {
        return;
    }
    let Some(data) = app.overview.ready() else {
        return;
    };
    let [cards, chart, latest] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Length(7),
        Constraint::Fill(1),
    ])
    .spacing(1)
    .areas(area);

    let s = &data.stats;
    let included = s
        .documents_included
        .map(|n| format!("of {n} included"))
        .unwrap_or_else(|| "this month".into());
    let card_data = [
        ("Read this month", count(s.documents_this_month), included),
        (
            "To check",
            count(s.pending_review_count),
            if s.pending_review_count == 0 {
                "all clear".into()
            } else {
                "press 3".into()
            },
        ),
        ("All time", count(s.documents_total), "invoices read".into()),
        (
            "Plan",
            capitalise(s.plan.as_str()),
            format!("since {}", day(&s.period_start)),
        ),
    ];
    let slots = Layout::horizontal([Constraint::Fill(1); 4])
        .spacing(1)
        .split(cards);
    for (i, (label, value, sub)) in card_data.into_iter().enumerate() {
        let warn = i == 1 && s.pending_review_count > 0;
        let text = Text::from(vec![
            Line::styled(label, theme.dim()),
            Line::styled(
                value,
                if warn {
                    theme.warn().add_modifier(Modifier::BOLD)
                } else {
                    theme.strong()
                },
            ),
            Line::styled(sub, theme.dim()),
        ]);
        f.render_widget(
            Paragraph::new(text).block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(theme.border())
                    .padding(Padding::horizontal(1)),
            ),
            slots[i],
        );
    }

    let days: Vec<(String, u64)> = data
        .usage
        .by_day
        .iter()
        .map(|d| {
            let label = d
                .day
                .as_deref()
                .and_then(|s| s.get(8..10))
                .map(|s| s.trim_start_matches('0').to_string())
                .unwrap_or_default();
            (label, d.documents.unwrap_or(0).max(0) as u64)
        })
        .collect();
    let max = days.iter().map(|(_, n)| *n).max().unwrap_or(0);
    let block = panel(
        format!("Invoices read per day this period · busiest day {max}"),
        theme,
        false,
    );
    let inner_width = block.inner(chart).width as usize;
    if days.len() > inner_width / 2 {
        // Too many days for bars with labels: a sparkline fills the width instead.
        let points: Vec<u64> = days.iter().map(|(_, n)| *n).collect();
        f.render_widget(
            Sparkline::default()
                .block(block)
                .data(&points)
                .style(theme.accent()),
            chart,
        );
    } else {
        let gap = 1usize;
        let width = ((inner_width + gap) / days.len().max(1))
            .saturating_sub(gap)
            .clamp(1, 8) as u16;
        let bars: Vec<Bar<'_>> = days
            .iter()
            .map(|(label, n)| {
                Bar::default()
                    .value(*n)
                    .label(Line::from(label.clone()))
                    .style(theme.accent())
                    .value_style(theme.bar_value_strong())
            })
            .collect();
        f.render_widget(
            BarChart::default()
                .block(block)
                .data(BarGroup::default().bars(&bars))
                .bar_width(width)
                .bar_gap(gap as u16),
            chart,
        );
    }

    let block = panel("Latest invoices", theme, false);
    let inner = block.inner(latest);
    f.render_widget(block, latest);
    let rows: Vec<Vec<Cell<'static>>> = data
        .latest
        .iter()
        .map(|d| {
            vec![
                Cell::from(
                    d.vendor_name
                        .clone()
                        .unwrap_or_else(|| d.original_filename.clone()),
                ),
                Cell::from(money(d.total_cents, d.currency.as_deref())),
                Cell::from(Span::styled(
                    if d.flagged {
                        "to check".to_string()
                    } else {
                        status_label(&d.status).to_string()
                    },
                    if d.flagged { theme.warn() } else { theme.dim() },
                )),
                Cell::from(Span::styled(day(&d.created_at), theme.dim())),
            ]
        })
        .collect();
    let widths = [
        Constraint::Fill(3),
        Constraint::Length(14),
        Constraint::Length(14),
        Constraint::Length(10),
    ];
    let header = Row::new(
        ["Supplier", "Total", "Status", "Added"]
            .map(|h| Cell::from(Span::styled(h.to_uppercase(), theme.dim()))),
    );
    f.render_widget(
        Table::new(rows.into_iter().map(Row::new), widths)
            .header(header)
            .column_spacing(2),
        inner,
    );
}

fn documents(f: &mut Frame<'_>, app: &mut App, theme: &Theme, area: Rect) {
    let (list_area, detail_area) = split_detail(area);
    let mut title = "Invoices".to_string();
    if app.flagged_only {
        title.push_str(" · to check only");
    }
    if !app.search.is_empty() {
        title.push_str(&format!(" · \"{}\"", app.search));
    }
    let block = panel(title, theme, true);
    let inner = block.inner(list_area);
    f.render_widget(block, list_area);

    let inner = if let Some(input) = &app.search_input {
        let [bar, rest] =
            Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(inner);
        let cursor = if app.tick % 10 < 5 { "▏" } else { " " };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Search  ", theme.accent()),
                Span::raw(input.clone()),
                Span::styled(cursor, theme.accent()),
            ])),
            bar,
        );
        rest
    } else {
        inner
    };

    if !status_block(f, app, theme, inner, &app.documents, "invoices") {
        let docs = app
            .documents
            .ready()
            .map(|d| d.as_slice())
            .unwrap_or_default();
        if docs.is_empty() {
            let msg = if app.search.is_empty() && !app.flagged_only {
                "No invoices yet. Upload one on the Upload screen."
            } else {
                "No invoices match."
            };
            f.render_widget(Paragraph::new(Span::styled(msg, theme.dim())), inner);
        } else {
            let rows = docs
                .iter()
                .map(|d| {
                    let status = if d.flagged {
                        Span::styled("! to check", theme.warn())
                    } else {
                        Span::styled(status_label(&d.status), theme.dim())
                    };
                    vec![
                        Cell::from(
                            d.vendor_name
                                .clone()
                                .unwrap_or_else(|| d.original_filename.clone()),
                        ),
                        Cell::from(money(d.total_cents, d.currency.as_deref())),
                        Cell::from(status),
                        Cell::from(Span::styled(day(&d.created_at), theme.dim())),
                    ]
                })
                .collect();
            let selected = app.doc_cursor.index;
            list_table(
                f,
                app,
                theme,
                inner,
                &["Supplier", "Total", "Status", "Added"],
                &[
                    Constraint::Fill(3),
                    Constraint::Length(12),
                    Constraint::Length(13),
                    Constraint::Length(10),
                ],
                rows,
                selected,
            );
        }
    }
    document_detail(f, app, theme, detail_area);
}

fn document_detail(f: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect) {
    let block = panel("Details", theme, false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(id) = app.selected_document_id() else {
        f.render_widget(
            Paragraph::new(Span::styled(
                "Choose an invoice to see what was read.",
                theme.dim(),
            )),
            inner,
        );
        return;
    };
    match app.details.get(&id) {
        Some(Loadable::Ready(detail)) => {
            f.render_widget(
                Paragraph::new(document_lines(detail, theme)).wrap(Wrap { trim: false }),
                inner,
            );
        }
        Some(Loadable::Failed(e)) => f.render_widget(
            Paragraph::new(Span::styled(e.clone(), theme.bad())).wrap(Wrap { trim: true }),
            inner,
        ),
        _ => f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(spinner(app), theme.accent()),
                Span::styled(" Loading…", theme.dim()),
            ])),
            inner,
        ),
    }
}

fn document_lines(detail: &DocumentDetail, theme: &Theme) -> Text<'static> {
    let mut lines = Vec::new();
    let doc = &detail.document;
    let Some(e) = &detail.extraction else {
        return Text::from(vec![
            Line::styled(doc.original_filename.clone(), theme.strong()),
            Line::styled("Not read yet.", theme.dim()),
        ]);
    };
    lines.push(Line::styled(
        e.vendor_name
            .clone()
            .unwrap_or_else(|| doc.original_filename.clone()),
        theme.strong(),
    ));
    if e.status == ExtractionStatus::Failed {
        lines.push(Line::styled(
            e.error_message
                .clone()
                .unwrap_or_else(|| "Invoist couldn't read this file.".into()),
            theme.bad(),
        ));
    }
    let mut facts = Vec::new();
    if let Some(n) = &e.invoice_number {
        facts.push(format!("Invoice {n}"));
    }
    if let Some(d) = &e.due_date {
        facts.push(format!("due {d}"));
    }
    if !facts.is_empty() {
        lines.push(Line::styled(facts.join(" · "), theme.dim()));
    }
    lines.push(Line::raw(""));
    let cur = e.currency.as_deref();
    let kv = |k: &str, v: String, strong: bool| {
        Line::from(vec![
            Span::styled(format!("{k:<10}"), theme.dim()),
            Span::styled(v, if strong { theme.strong() } else { theme.text() }),
        ])
    };
    lines.push(kv("Total", money(e.total_cents, cur), true));
    if e.subtotal_cents.is_some() {
        lines.push(kv("Subtotal", money(e.subtotal_cents, cur), false));
    }
    if e.tax_cents.is_some() {
        lines.push(kv("Tax", money(e.tax_cents, cur), false));
    }
    if let Some(d) = &e.invoice_date {
        lines.push(kv("Issued", d.clone(), false));
    }
    if let Some(r) = &e.reference_number {
        lines.push(kv("Reference", r.clone(), false));
    }

    let open: Vec<_> = detail
        .review_queue
        .iter()
        .filter(|i| i.status == ReviewStatus::Pending)
        .collect();
    if !open.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "To check",
            theme.warn().add_modifier(Modifier::BOLD),
        ));
        for item in open {
            lines.push(Line::styled(
                format!("! {}", reason_label(&item.reason)),
                theme.warn(),
            ));
        }
    }
    if let Some(items) = e.line_items.as_ref().filter(|l| !l.is_empty()) {
        lines.push(Line::raw(""));
        lines.push(Line::styled("Lines", theme.dim()));
        for l in items.iter().take(12) {
            let description: String = l.description.chars().take(28).collect();
            lines.push(Line::from(vec![
                Span::raw(format!("{description:<28}")),
                Span::styled(
                    format!(" {:>12}", money(l.line_total_cents, cur)),
                    theme.dim(),
                ),
            ]));
        }
        if items.len() > 12 {
            lines.push(Line::styled(
                format!("and {} more", items.len() - 12),
                theme.dim(),
            ));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        format!("{} · {}", short_id(&doc.id), doc.original_filename),
        theme.dim(),
    ));
    Text::from(lines)
}

fn review(f: &mut Frame<'_>, app: &mut App, theme: &Theme, area: Rect) {
    let (list_area, detail_area) = split_detail(area);
    let count = app.review.ready().map_or(0, Vec::len);
    let block = panel(format!("To check · {count}"), theme, true);
    let inner = block.inner(list_area);
    f.render_widget(block, list_area);
    if !status_block(f, app, theme, inner, &app.review, "the invoices to check") {
        let items = app.review.ready().map(|v| v.as_slice()).unwrap_or_default();
        if items.is_empty() {
            f.render_widget(
                Paragraph::new(Text::from(vec![
                    Line::styled(
                        "✓ Nothing to check",
                        theme.good().add_modifier(Modifier::BOLD),
                    ),
                    Line::styled(
                        "Invoices that look unusual will wait here for you.",
                        theme.dim(),
                    ),
                ])),
                inner,
            );
        } else {
            let resolving = app.resolving.clone();
            let rows = items
                .iter()
                .map(|i| {
                    let why = if resolving.as_deref() == Some(i.id.as_str()) {
                        Span::styled(format!("{} saving…", spinner(app)), theme.accent())
                    } else {
                        Span::styled(format!("! {}", reason_label(&i.reason)), theme.warn())
                    };
                    vec![
                        Cell::from(why),
                        Cell::from(
                            i.vendor_name
                                .clone()
                                .unwrap_or_else(|| i.original_filename.clone()),
                        ),
                        Cell::from(money(i.total_cents, i.currency.as_deref())),
                    ]
                })
                .collect();
            let selected = app.review_cursor.index;
            list_table(
                f,
                app,
                theme,
                inner,
                &["Why", "Supplier", "Total"],
                &[
                    Constraint::Length(25),
                    Constraint::Fill(2),
                    Constraint::Length(12),
                ],
                rows,
                selected,
            );
        }
    }
    document_detail(f, app, theme, detail_area);
}

fn upload(f: &mut Frame<'_>, app: &mut App, theme: &Theme, area: Rect) {
    let [files_area, queue_area] = if area.width >= 110 {
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .spacing(1)
            .areas(area)
    } else {
        Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(area)
    };
    // The end of a long path is the part that says where you are.
    let room = (files_area.width as usize).saturating_sub(22).max(8);
    let block = panel(
        format!("Choose files · {}", keep_end(&display_dir(&app.cwd), room)),
        theme,
        true,
    );
    let inner = block.inner(files_area);
    f.render_widget(block, files_area);
    let rows = app
        .entries
        .iter()
        .map(|e| {
            vec![Cell::from(if e.dir {
                Span::styled(format!("▸ {}/", e.name), theme.accent())
            } else {
                Span::raw(format!("  {}", e.name))
            })]
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("No PDF, PNG or JPEG files here.", theme.dim())),
            inner,
        );
    } else {
        let selected = app.file_cursor.index;
        list_table(
            f,
            app,
            theme,
            inner,
            &["PDF, PNG and JPEG files"],
            &[Constraint::Fill(1)],
            rows,
            selected,
        );
    }

    let block = panel("Uploads", theme, false);
    let inner = block.inner(queue_area);
    f.render_widget(block, queue_area);
    if app.uploads.is_empty() {
        f.render_widget(
            Paragraph::new(Text::from(vec![
                Line::styled("Press enter on a file to upload it.", theme.dim()),
                Line::styled("Invoist reads it and shows the result here.", theme.dim()),
            ])),
            inner,
        );
        return;
    }
    let mut lines = Vec::new();
    for item in app.uploads.iter().rev() {
        let name = item
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut checks_line = None;
        let (mark, status) = match &item.status {
            UploadStatus::Waiting => (
                Span::styled("·", theme.dim()),
                Span::styled("waiting", theme.dim()),
            ),
            UploadStatus::Uploading => (
                Span::styled(spinner(app), theme.accent()),
                Span::styled("uploading", theme.dim()),
            ),
            UploadStatus::Reading => (
                Span::styled(spinner(app), theme.accent()),
                Span::styled("reading", theme.dim()),
            ),
            UploadStatus::Done {
                vendor,
                total,
                duplicate,
                checks,
            } => {
                let mut text = format!("{vendor} · {total}");
                if *duplicate {
                    text.push_str(" · already uploaded");
                }
                if checks.is_empty() {
                    (Span::styled("✓", theme.good()), Span::raw(text))
                } else {
                    // What to check gets its own line, so it never wraps
                    // into the file name's column.
                    checks_line = Some(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(format!("check: {}", checks.join(", ")), theme.warn()),
                    ]));
                    (Span::styled("!", theme.warn()), Span::raw(text))
                }
            }
            UploadStatus::Failed(e) => (
                Span::styled("✗", theme.bad()),
                Span::styled(e.clone(), theme.bad()),
            ),
        };
        lines.push(Line::from(vec![
            mark,
            Span::raw(" "),
            Span::styled(name, theme.strong()),
        ]));
        lines.push(Line::from(vec![Span::raw("  "), status]));
        lines.extend(checks_line);
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Joins words with a separator into lines no wider than `width`, breaking
/// only between words, so no line starts with the separator.
pub fn join_to_width(words: &[String], sep: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in words {
        let added = if line.is_empty() {
            word.chars().count()
        } else {
            sep.chars().count() + word.chars().count()
        };
        if !line.is_empty() && line.chars().count() + added > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push_str(sep);
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Text no longer than `max` characters, cut from the start with `…`.
pub fn keep_end(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_string();
    }
    let tail: String = text.chars().skip(n - max.saturating_sub(1)).collect();
    format!("…{tail}")
}

/// A folder as people write it: the home folder as `~`.
pub fn display_dir(path: &std::path::Path) -> String {
    if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf())
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display())
        };
    }
    path.display().to_string()
}

fn webhooks(f: &mut Frame<'_>, app: &mut App, theme: &Theme, area: Rect) {
    let block = panel("Webhook endpoints", theme, true);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if status_block(f, app, theme, inner, &app.webhooks, "webhook endpoints") {
        return;
    }
    let Some(list) = app.webhooks.ready() else {
        return;
    };
    let event_lines = join_to_width(&list.known_events, "  ·  ", inner.width as usize);
    let [table_area, events_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(event_lines.len() as u16 + 1),
    ])
    .areas(inner);
    let mut text = vec![Line::styled("Events Invoist can send", theme.dim())];
    text.extend(event_lines.into_iter().map(Line::raw));
    f.render_widget(Paragraph::new(text), events_area);
    if list.endpoints.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                "No endpoints yet. Add one with `invoist webhooks create <url>`.",
                theme.dim(),
            )),
            table_area,
        );
        return;
    }
    let rows = list
        .endpoints
        .iter()
        .map(|e| {
            let state = if !e.enabled {
                Span::styled("✗ paused", theme.bad())
            } else if e.consecutive_failures > 0 {
                Span::styled(
                    format!("! {} failures", e.consecutive_failures),
                    theme.warn(),
                )
            } else {
                Span::styled("✓ working", theme.good())
            };
            let events = if e.events.is_empty() || e.events.iter().any(|v| v == "*") {
                "all".to_string()
            } else {
                e.events.join(", ")
            };
            vec![
                Cell::from(e.url.clone()),
                Cell::from(Span::styled(events, theme.dim())),
                Cell::from(state),
            ]
        })
        .collect();
    let selected = app.webhook_cursor.index;
    list_table(
        f,
        app,
        theme,
        table_area,
        &["URL", "Events", "State"],
        &[
            Constraint::Fill(3),
            Constraint::Fill(2),
            Constraint::Length(14),
        ],
        rows,
        selected,
    );
}

// ── Household screens ───────────────────────────────────────────────────────

fn bill_row(b: &Bill, today: &str, theme: &Theme) -> Vec<Cell<'static>> {
    let late = b.bill.paid_at.is_none() && b.due_date.as_deref().is_some_and(|d| d < today);
    let due = match (&b.bill.paid_at, &b.due_date) {
        (Some(at), _) => Span::styled(format!("✓ paid {}", day(at)), theme.good()),
        (None, Some(d)) if late => Span::styled(format!("! {d} overdue"), theme.warn()),
        (None, Some(d)) => Span::raw(d.clone()),
        (None, None) => Span::styled("no due date", theme.dim()),
    };
    vec![
        Cell::from(
            b.vendor_name
                .clone()
                .unwrap_or_else(|| b.original_filename.clone()),
        ),
        Cell::from(money(b.total_cents, b.currency.as_deref())),
        Cell::from(due),
    ]
}

fn to_pay(f: &mut Frame<'_>, app: &mut App, theme: &Theme, area: Rect) {
    if status_block(f, app, theme, area, &app.to_pay, "your bills") {
        return;
    }
    let Some(data) = app.to_pay.ready() else {
        return;
    };
    let summary = &data.summary;
    let [head, rest] = Layout::vertical([Constraint::Length(4), Constraint::Fill(1)]).areas(area);

    let due: Vec<String> = summary
        .due_soon
        .iter()
        .map(|d| money(Some(d.total_cents), Some(&d.currency)))
        .collect();
    let mut head_lines = vec![Line::from(vec![
        Span::styled(
            format!("{}", summary.unpaid),
            theme.strong().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if summary.unpaid == 1 {
                " bill to pay"
            } else {
                " bills to pay"
            },
            theme.text(),
        ),
        if summary.overdue > 0 {
            Span::styled(format!("  ·  ! {} overdue", summary.overdue), theme.warn())
        } else {
            Span::raw("")
        },
    ])];
    if !due.is_empty() {
        head_lines.push(Line::from(vec![
            Span::styled(due.join(" + "), theme.accent()),
            Span::styled(
                format!(" due in the next {} days", summary.days),
                theme.dim(),
            ),
        ]));
    } else if summary.unpaid == 0 {
        head_lines.push(Line::styled("✓ Nothing left to pay.", theme.good()));
    }
    f.render_widget(
        Paragraph::new(head_lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(theme.border())
                .padding(Padding::horizontal(1)),
        ),
        head,
    );

    let (list_area, detail_area) = split_detail(rest);
    let block = panel("Unpaid, soonest first", theme, true);
    let inner = block.inner(list_area);
    f.render_widget(block, list_area);
    let today = summary.today.clone();
    let rows: Vec<Vec<Cell<'static>>> = app
        .unpaid_sorted()
        .iter()
        .map(|b| bill_row(b, &today, theme))
        .collect();
    if rows.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("No unpaid bills.", theme.dim())),
            inner,
        );
    } else {
        let selected = app.pay_cursor.index;
        list_table(
            f,
            app,
            theme,
            inner,
            &["From", "Amount", "Due"],
            &[
                Constraint::Fill(2),
                Constraint::Length(14),
                Constraint::Length(20),
            ],
            rows,
            selected,
        );
    }
    bill_detail(f, app, theme, detail_area);
}

fn bill_detail(f: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect) {
    let block = panel("Bill", theme, false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(bill) = app.selected_bill() else {
        f.render_widget(
            Paragraph::new(Span::styled(
                "Choose a bill to see who to pay.",
                theme.dim(),
            )),
            inner,
        );
        return;
    };
    match app.bill_details.get(&bill.id) {
        Some(Loadable::Ready(detail)) => f.render_widget(
            Paragraph::new(bill_lines(detail, theme)).wrap(Wrap { trim: false }),
            inner,
        ),
        Some(Loadable::Failed(e)) => f.render_widget(
            Paragraph::new(Span::styled(e.clone(), theme.bad())).wrap(Wrap { trim: true }),
            inner,
        ),
        _ => f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(spinner(app), theme.accent()),
                Span::styled(" Loading…", theme.dim()),
            ])),
            inner,
        ),
    }
}

fn bill_lines(detail: &BillDetail, theme: &Theme) -> Text<'static> {
    let doc = &detail.document;
    let e = detail.extraction.as_ref();
    let mut lines = vec![Line::styled(
        e.and_then(|e| e.vendor_name.clone())
            .unwrap_or_else(|| doc.original_filename.clone()),
        theme.strong(),
    )];
    lines.push(match &detail.bill.paid_at {
        Some(at) => Line::styled(format!("✓ paid {}", day(at)), theme.good()),
        None => Line::styled("to pay", theme.warn()),
    });
    lines.push(Line::raw(""));
    let kv = |k: &str, v: String, strong: bool| {
        Line::from(vec![
            Span::styled(format!("{k:<10}"), theme.dim()),
            Span::styled(v, if strong { theme.strong() } else { theme.text() }),
        ])
    };
    if let Some(e) = e {
        lines.push(kv(
            "Amount",
            money(e.total_cents, e.currency.as_deref()),
            true,
        ));
        if let Some(d) = &e.due_date {
            lines.push(kv("Due", d.clone(), false));
        }
        if let Some(r) = &e.reference_number {
            lines.push(kv("Reference", r.clone(), false));
        }
    }
    if let Some(member) = detail
        .chain
        .as_ref()
        .and_then(|c| c.members.iter().find(|m| m.document_id == doc.id))
    {
        for (label, value) in [
            ("IBAN", &member.vendor_iban),
            ("BIC", &member.vendor_bic),
            ("Account", &member.payment_account),
        ] {
            if let Some(v) = value {
                lines.push(kv(label, v.clone(), false));
            }
        }
    }
    let open: Vec<_> = detail
        .review_queue
        .iter()
        .filter(|i| i.status == ReviewStatus::Pending)
        .collect();
    if !open.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Check before paying",
            theme.warn().add_modifier(Modifier::BOLD),
        ));
        for item in open {
            lines.push(Line::styled(
                format!("! {}", reason_label(&item.reason)),
                theme.warn(),
            ));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        format!("{} · {}", short_id(&doc.id), doc.original_filename),
        theme.dim(),
    ));
    Text::from(lines)
}

fn bills(f: &mut Frame<'_>, app: &mut App, theme: &Theme, area: Rect) {
    let [head, rest] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)])
        .spacing(1)
        .areas(area);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("◂ ", theme.dim()),
            Span::styled(
                month_name(app.month.year(), app.month.month()),
                theme.accent(),
            ),
            Span::styled(" ▸", theme.dim()),
        ])),
        head,
    );
    if status_block(f, app, theme, rest, &app.bills, "this month's bills") {
        return;
    }
    let [chart_area, below] =
        Layout::vertical([Constraint::Length(10), Constraint::Fill(1)]).areas(rest);
    if let Some(data) = app.bills.ready() {
        let currency = data
            .stats
            .totals
            .first()
            .map(|t| t.currency.clone())
            .unwrap_or_default();
        let trend: Vec<_> = data
            .stats
            .trend
            .iter()
            .filter(|t| t.currency == currency)
            .collect();
        let inner_width = chart_area.width.saturating_sub(4) as usize;
        let gap = 2usize;
        let width = ((inner_width + gap) / trend.len().max(1))
            .saturating_sub(gap)
            .clamp(3, 12) as u16;
        let bars: Vec<Bar<'_>> = trend
            .iter()
            .map(|t| {
                let label = t
                    .month
                    .get(5..7)
                    .and_then(|m| m.parse::<u8>().ok())
                    .map(short_month)
                    .unwrap_or("");
                let amount = money(Some(t.total_cents), Some(&t.currency));
                let amount = amount.split('.').next().unwrap_or_default().to_string();
                let mut bar = Bar::default()
                    .value((t.total_cents.max(0) / 100) as u64)
                    .label(Line::from(label))
                    .text_value(if amount.chars().count() <= width as usize {
                        amount
                    } else {
                        String::new()
                    });
                if t.month == data.stats.month {
                    bar = bar
                        .style(theme.accent())
                        .value_style(theme.bar_value_strong());
                } else {
                    bar = bar.style(theme.dim()).value_style(theme.bar_value());
                }
                bar
            })
            .collect();
        let total = data
            .stats
            .totals
            .first()
            .map(|t| money(Some(t.total_cents), Some(&t.currency)))
            .unwrap_or_else(|| "nothing".into());
        let chart = BarChart::default()
            .block(panel(
                format!("Bills by month · {total} this month"),
                theme,
                false,
            ))
            .data(BarGroup::default().bars(&bars))
            .bar_width(width)
            .bar_gap(gap as u16);
        f.render_widget(chart, chart_area);
    }
    let (list_area, detail_area) = split_detail(below);
    let block = panel("This month's bills", theme, true);
    let inner = block.inner(list_area);
    f.render_widget(block, list_area);
    let today = app.today.clone();
    let rows: Vec<Vec<Cell<'static>>> = app
        .bills
        .ready()
        .map(|d| d.bills.iter().map(|b| bill_row(b, &today, theme)).collect())
        .unwrap_or_default();
    if rows.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("No bills this month.", theme.dim())),
            inner,
        );
    } else {
        let selected = app.bill_cursor.index;
        list_table(
            f,
            app,
            theme,
            inner,
            &["From", "Amount", "Due"],
            &[
                Constraint::Fill(2),
                Constraint::Length(14),
                Constraint::Length(20),
            ],
            rows,
            selected,
        );
    }
    bill_detail(f, app, theme, detail_area);
}

fn held(f: &mut Frame<'_>, app: &mut App, theme: &Theme, area: Rect) {
    let block = panel("Emails waiting for approval", theme, true);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if status_block(f, app, theme, inner, &app.held, "waiting emails") {
        return;
    }
    let list = app.held.ready().map(|v| v.as_slice()).unwrap_or_default();
    if list.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("✓ No emails are waiting.", theme.good())),
            inner,
        );
        return;
    }
    let [table_area, note] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    let rows = list
        .iter()
        .map(|m| {
            vec![
                Cell::from(m.from_email.clone()),
                Cell::from(m.subject.clone()),
                Cell::from(Span::styled(
                    format!("{} files", m.files.len()),
                    theme.dim(),
                )),
                Cell::from(Span::styled(
                    m.held_until.as_deref().map(day).unwrap_or_default(),
                    theme.dim(),
                )),
            ]
        })
        .collect();
    let selected = app.held_cursor.index;
    list_table(
        f,
        app,
        theme,
        table_area,
        &["From", "Subject", "Files", "Kept until"],
        &[
            Constraint::Fill(2),
            Constraint::Fill(3),
            Constraint::Length(8),
            Constraint::Length(10),
        ],
        rows,
        selected,
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            "Approve or block senders in the Invoist app.",
            theme.dim(),
        )),
        note,
    );
}

// ── Overlays ────────────────────────────────────────────────────────────────

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(4));
    let h = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn help(f: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect) {
    let mut rows: Vec<(&str, &str)> = vec![
        ("tab, shift-tab, 1–9", "change screen"),
        ("↑ ↓ or j k", "move"),
        ("page up, page down", "move by ten"),
        ("g, G", "first, last"),
        ("r", "refresh"),
        ("o", "open the original file"),
        (": or ctrl-k", "commands"),
        ("q", "quit"),
    ];
    match app.screen() {
        Screen::Documents => rows.extend([
            ("/", "search"),
            ("f", "only invoices to check"),
            ("esc", "clear the search"),
        ]),
        Screen::Review => rows.extend([("c or enter", "mark as checked"), ("d", "not a problem")]),
        Screen::Upload => rows.extend([
            ("enter", "open a folder, or upload a file"),
            ("← or backspace", "up a folder"),
        ]),
        Screen::ToPay | Screen::Bills => rows.extend([
            ("p", "mark paid"),
            ("u", "mark not paid"),
            ("← →", "change month (Bills)"),
        ]),
        _ => {}
    }
    let rect = centered(area, 56, rows.len() as u16 + 4);
    f.render_widget(Clear, rect);
    let lines: Vec<Line<'_>> = rows
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!("{k:<22}"), theme.strong()),
                Span::styled(*v, theme.dim()),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(panel("Keys", theme, true).padding(Padding::uniform(1))),
        rect,
    );
}

fn palette(f: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect, query: &str, selected: usize) {
    let matches = app.matching_actions(query);
    let rect = centered(area, 60, (matches.len() as u16 + 5).min(18));
    f.render_widget(Clear, rect);
    let block = panel("Commands", theme, true);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [input, list] = Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(inner);
    let cursor = if app.tick % 10 < 5 { "▏" } else { " " };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("› ", theme.accent()),
            Span::raw(query.to_string()),
            Span::styled(cursor, theme.accent()),
        ])),
        input,
    );
    let lines: Vec<Line<'_>> = matches
        .iter()
        .enumerate()
        .map(|(i, (name, _))| {
            if i == selected {
                Line::styled(format!(" {name} "), theme.selected())
            } else {
                Line::styled(format!(" {name}"), theme.text())
            }
        })
        .collect();
    if lines.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(" No command matches.", theme.dim())),
            list,
        );
    } else {
        f.render_widget(Paragraph::new(lines), list);
    }
}

fn confirm(f: &mut Frame<'_>, theme: &Theme, area: Rect, question: &str) {
    let rect = centered(area, 60, 7);
    f.render_widget(Clear, rect);
    let text = Text::from(vec![
        Line::raw(question.to_string()),
        Line::raw(""),
        Line::from(vec![
            Span::styled("y", theme.strong()),
            Span::styled(" yes    ", theme.dim()),
            Span::styled("n", theme.strong()),
            Span::styled(" no", theme.dim()),
        ]),
    ]);
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: true })
            .block(panel("Are you sure?", theme, true)),
        rect,
    );
}

fn short_month(m: u8) -> &'static str {
    [
        "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .get(m as usize)
    .copied()
    .unwrap_or("")
}

fn month_name(year: u16, month: u8) -> String {
    let name = [
        "",
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ]
    .get(month as usize)
    .copied()
    .unwrap_or("");
    format!("{name} {year}")
}
