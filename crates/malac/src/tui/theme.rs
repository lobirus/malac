//! Colors, chosen once per process. The interface is neutral: the
//! terminal's own foreground and background with greys, green for focus,
//! links and success, amber for warnings and pending approvals, red for
//! errors and denials. Truecolor is used when `COLORTERM` says the terminal
//! supports it, otherwise the 256-color palette, which tmux and screen render
//! faithfully. `NO_COLOR` turns colors off entirely.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};

pub struct Theme {
    /// Green: focus, selection, links and success.
    pub accent: Color,
    pub warning: Color,
    pub error: Color,
    /// Secondary text.
    pub dim: Color,
    /// Borders, rules and hints.
    pub faint: Color,
    pub code_bg: Color,
    /// The header and status bars.
    pub bar_bg: Color,
    pub selection_bg: Color,
    /// Labels in the function-key bar.
    pub key_bg: Color,
    pub truecolor: bool,
    pub no_color: bool,
}

impl Theme {
    fn new(truecolor: bool, no_color: bool) -> Self {
        let pick = |rgb: (u8, u8, u8), indexed: u8| {
            if no_color {
                Color::Reset
            } else if truecolor {
                Color::Rgb(rgb.0, rgb.1, rgb.2)
            } else {
                Color::Indexed(indexed)
            }
        };
        Self {
            accent: pick((0x7e, 0xc6, 0x8a), 108),
            warning: pick((0xd9, 0xa4, 0x41), 179),
            error: pick((0xe0, 0x6c, 0x6c), 167),
            dim: pick((0x8f, 0x8f, 0x8f), 245),
            faint: pick((0x58, 0x58, 0x58), 240),
            code_bg: pick((0x26, 0x26, 0x26), 235),
            bar_bg: pick((0x2e, 0x2e, 0x2e), 236),
            selection_bg: pick((0x3a, 0x3a, 0x3a), 237),
            key_bg: pick((0x58, 0x58, 0x58), 240),
            truecolor,
            no_color,
        }
    }

    pub fn fg(&self, color: Color) -> Style {
        Style::new().fg(color)
    }

    pub fn dim(&self) -> Style {
        Style::new().fg(self.dim)
    }

    pub fn faint(&self) -> Style {
        Style::new().fg(self.faint)
    }

    pub fn bold(&self) -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }

    /// The highlighted row of a menu or list.
    pub fn selected(&self) -> Style {
        if self.no_color {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new().bg(self.accent).fg(Color::Black)
        }
    }

    pub fn bar(&self) -> Style {
        Style::new().bg(self.bar_bg)
    }
}

pub fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let truecolor = std::env::var("COLORTERM")
            .is_ok_and(|v| v.eq_ignore_ascii_case("truecolor") || v.eq_ignore_ascii_case("24bit"));
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        Theme::new(truecolor, no_color)
    })
}
