//! The full-screen terminal app: `invoist` on its own, or `invoist ui`.
//!
//! Three things feed one loop: keys and the mouse (read on their own
//! thread), results from the API (the worker), and a tick ten times a
//! second for spinners. Each goes through `App`, which returns requests for
//! the worker, and the screen is redrawn. The terminal is always put back
//! as it was, on quit, on error and on a crash.

pub mod app;
mod theme;
pub mod ui;
mod worker;

use std::time::Duration;

use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use ratatui::crossterm::execute;
use tokio::sync::mpsc;

use crate::error::{CliError, Result};
use crate::session::Session;
use crate::tui::app::App;
use crate::tui::worker::Worker;

/// Today's date from the system clock, as `YYYY-MM-DD`.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

pub async fn run(profile: Option<&str>, base_url: Option<&str>) -> Result<()> {
    let mut session = Session::open(profile, base_url)?;
    eprintln!("Opening Invoist…");
    let kind = session.kind().await?;
    let plain = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
        || matches!(
            anstream::ColorChoice::global(),
            anstream::ColorChoice::Never
        );
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut app = App::new(
        kind,
        session.mode,
        session.profile.clone(),
        today(),
        cwd,
        plain,
    );

    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel();
    let worker = Worker::new(session, msg_tx);
    for request in app.start() {
        worker.send(request);
    }

    // Keys and the mouse, read on a plain thread: crossterm's reader blocks.
    let (input_tx, mut input_rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(e) = event::read() {
            if input_tx.send(e).is_err() {
                break;
            }
        }
    });

    // ratatui::init enters the alternate screen and raw mode, and installs a
    // panic hook that restores the terminal before the message is printed.
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let result = async {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            terminal
                .draw(|f| ui::draw(f, &mut app))
                .map_err(|e| CliError::io("couldn't draw the screen", e))?;
            let requests = tokio::select! {
                Some(input) = input_rx.recv() => match input {
                    Event::Key(key) => app.key(key),
                    Event::Mouse(mouse) => app.mouse(mouse),
                    _ => Vec::new(),
                },
                Some(msg) = msg_rx.recv() => app.message(msg),
                _ = tick.tick() => {
                    app.on_tick();
                    Vec::new()
                }
            };
            for request in requests {
                worker.send(request);
            }
            if app.quit {
                return Ok(());
            }
        }
    }
    .await;
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod clock_tests {
    use super::civil_from_days;

    #[test]
    fn days_since_1970_become_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_760), (2026, 11, 3));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
