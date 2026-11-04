//! The terminal app's colours. Named ANSI colours, not fixed RGB, so the
//! app follows the terminal's own palette on dark and light backgrounds and
//! works with 16 colours. With `NO_COLOR` (or `--no-color`) every style is
//! plain, and nothing relies on colour alone: warnings also carry a symbol
//! and a word.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub plain: bool,
}

impl Theme {
    pub fn new(plain: bool) -> Theme {
        Theme { plain }
    }

    fn color(&self, c: Color) -> Style {
        if self.plain {
            Style::new()
        } else {
            Style::new().fg(c)
        }
    }

    pub fn accent(&self) -> Style {
        self.color(Color::Blue).add_modifier(Modifier::BOLD)
    }
    pub fn text(&self) -> Style {
        Style::new()
    }
    pub fn strong(&self) -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }
    pub fn dim(&self) -> Style {
        if self.plain {
            Style::new()
        } else {
            Style::new().fg(Color::DarkGray)
        }
    }
    pub fn good(&self) -> Style {
        self.color(Color::Green)
    }
    pub fn warn(&self) -> Style {
        self.color(Color::Yellow)
    }
    pub fn bad(&self) -> Style {
        self.color(Color::Red)
    }
    /// The selected row and the current screen's tab: the terminal's own
    /// colours, swapped. A fixed pair such as white on blue is unreadable in
    /// some colour schemes, where "blue" is pale and "white" is light grey;
    /// swapping the terminal's own colours reads in every one. Colours set on
    /// parts of the row are reset, so the whole row turns over evenly.
    pub fn selected(&self) -> Style {
        Style::new()
            .fg(Color::Reset)
            .bg(Color::Reset)
            .add_modifier(Modifier::REVERSED | Modifier::BOLD)
    }
    /// An amount written inside the highlighted bar: the bar's colour as the
    /// background, the terminal's background colour as the text.
    pub fn bar_value_strong(&self) -> Style {
        if self.plain {
            self.selected()
        } else {
            Style::new()
                .fg(Color::Blue)
                .add_modifier(Modifier::REVERSED | Modifier::BOLD)
        }
    }
    /// An amount written inside a grey bar, the same way.
    pub fn bar_value(&self) -> Style {
        if self.plain {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::REVERSED)
        }
    }
    pub fn border(&self) -> Style {
        self.dim()
    }
    pub fn border_focus(&self) -> Style {
        self.color(Color::Blue)
    }
    pub fn badge(&self) -> Style {
        if self.plain {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
                .bg(Color::Yellow)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        }
    }
}
