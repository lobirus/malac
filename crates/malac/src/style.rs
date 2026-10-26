//! Styling for command output. It reads like crossterm's `Stylize`, but
//! writes plain text when stdout is not a terminal or `NO_COLOR` is set,
//! so `malac -p … > out.txt` and pipes get no escape codes.

use std::fmt::{self, Display};
use std::io::IsTerminal;
use std::sync::OnceLock;

use crossterm::style::{ContentStyle, StyledContent};

/// Whether output is styled: stdout is a terminal and `NO_COLOR` is unset.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
    })
}

/// Text with a style that shows only when [`enabled`].
pub struct Painted<D: Display>(StyledContent<D>);

impl<D: Display> Painted<D> {
    fn write(&self, f: &mut fmt::Formatter<'_>, styled: bool) -> fmt::Result {
        if styled {
            self.0.fmt(f)
        } else {
            self.0.content().fmt(f)
        }
    }
}

impl<D: Display> Display for Painted<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write(f, enabled())
    }
}

macro_rules! styles {
    ($($name:ident),*) => {
        /// The styles command output uses, for anything printable. Every
        /// style works alone and chained, whether or not both are used yet.
        #[allow(dead_code)]
        pub trait Paint: Display + Sized {
            $(fn $name(self) -> Painted<Self> {
                Painted(crossterm::style::Stylize::$name(StyledContent::new(ContentStyle::new(), self)))
            })*
        }

        impl<D: Display> Paint for D {}

        /// Styles combine: `"x".dark_grey().italic()`.
        #[allow(dead_code)]
        impl<D: Display> Painted<D> {
            $(pub fn $name(self) -> Self {
                Painted(crossterm::style::Stylize::$name(self.0))
            })*
        }
    };
}

styles!(
    bold,
    italic,
    underlined,
    red,
    green,
    yellow,
    cyan,
    dark_red,
    dark_green,
    dark_yellow,
    dark_cyan,
    dark_grey,
    reset
);

#[cfg(test)]
mod tests {
    use super::*;

    struct Show<'a, D: Display>(&'a Painted<D>, bool);

    impl<D: Display> Display for Show<'_, D> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.write(f, self.1)
        }
    }

    #[test]
    fn plain_output_has_no_escape_codes() {
        let painted = "done".dark_grey().italic();
        assert_eq!(Show(&painted, false).to_string(), "done");
        let styled = Show(&painted, true).to_string();
        assert!(styled.contains("\x1b[") && styled.contains("done"));
        assert_eq!(
            Show(&format!("{} files", 3).bold(), false).to_string(),
            "3 files"
        );
    }
}
