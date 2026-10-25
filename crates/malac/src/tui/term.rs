//! Entering and leaving the full-screen terminal, and copying to the clipboard.

use std::io::{self, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static ENHANCED: AtomicBool = AtomicBool::new(false);

/// Switches to the alternate screen with raw input, mouse reporting and
/// bracketed paste. Terminals that speak the kitty keyboard protocol are
/// asked to disambiguate keys, which makes Shift+Enter distinguishable.
pub fn enter() -> io::Result<Tui> {
    terminal::enable_raw_mode()?;
    ACTIVE.store(true, Ordering::SeqCst);
    install_panic_hook();
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture
    )?;
    if terminal::supports_keyboard_enhancement().unwrap_or(false) {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        ENHANCED.store(true, Ordering::SeqCst);
    }
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    terminal.clear()?;
    Ok(terminal)
}

/// Restores the terminal. Safe to call more than once, and from the panic hook.
pub fn leave() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut stdout = io::stdout();
    if ENHANCED.swap(false, Ordering::SeqCst) {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        stdout,
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        cursor::Show
    );
    let _ = terminal::disable_raw_mode();
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        leave();
        previous(info);
    }));
}

/// Where log lines go: stderr normally, a file while the interface owns
/// the screen, so warnings cannot scribble over it.
pub fn log_writer() -> Box<dyn Write> {
    if !ACTIVE.load(Ordering::SeqCst) {
        return Box::new(io::stderr());
    }
    let path = malac_core::config::home_dir().join("malac.log");
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(file) => Box::new(file),
        Err(_) => Box::new(io::sink()),
    }
}

/// Mouse reporting off lets the terminal select text natively.
pub fn set_mouse_capture(on: bool) {
    let mut stdout = io::stdout();
    let _ = if on {
        execute!(stdout, EnableMouseCapture)
    } else {
        execute!(stdout, DisableMouseCapture)
    };
}

#[cfg(test)]
thread_local! {
    /// What tests copied, instead of the clipboard.
    pub static COPIED: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Copies through OSC 52, which Ghostty, Kitty, WezTerm and tmux (with
/// `set-clipboard on`) honor, and through `pbcopy` on a local Mac, which
/// covers iTerm2 and Terminal.app with their default settings.
pub fn copy(text: &str) {
    #[cfg(test)]
    COPIED.with(|c| *c.borrow_mut() = Some(text.to_owned()));
    #[cfg(not(test))]
    to_clipboard(text);
}

#[cfg(not(test))]
fn to_clipboard(text: &str) {
    use base64::Engine;
    use std::process::{Command, Stdio};

    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let osc = format!("\x1b]52;c;{encoded}\x07");
    let mut stdout = io::stdout();
    let _ = stdout.write_all(osc.as_bytes());
    if std::env::var_os("TMUX").is_some() {
        // Passed straight to the outer terminal when `allow-passthrough` is on.
        let _ = write!(
            stdout,
            "\x1bPtmux;{}\x1b\\",
            osc.replace('\x1b', "\x1b\x1b")
        );
    }
    let _ = stdout.flush();
    let local = std::env::var_os("SSH_CONNECTION").is_none();
    if cfg!(target_os = "macos") && local {
        let child = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }
}
