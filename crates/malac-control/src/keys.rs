//! Key combinations such as `cmd+shift+t`, shared by desktop and browser input.

use std::str::FromStr;

use anyhow::{Error, Result, bail};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Command on macOS, Super/Windows elsewhere.
    pub meta: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Space,
    Backspace,
    Delete,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    CapsLock,
    /// F1 to F20.
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl From<Key> for KeyCombo {
    fn from(key: Key) -> Self {
        Self {
            modifiers: Modifiers::default(),
            key,
        }
    }
}

impl FromStr for KeyCombo {
    type Err = Error;

    fn from_str(combo: &str) -> Result<Self> {
        let combo = combo.trim();
        let (modifiers, key) = match combo.strip_suffix('+') {
            Some("") => ("", "+"),
            Some(rest) if rest.ends_with('+') => (&rest[..rest.len() - 1], "+"),
            _ => combo.rsplit_once('+').unwrap_or(("", combo)),
        };
        if key.is_empty() {
            bail!("missing key in `{combo}`");
        }
        let mut parsed = Modifiers::default();
        for name in modifiers.split('+').filter(|m| !m.is_empty()) {
            let flag = match name.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut parsed.ctrl,
                "alt" | "option" | "opt" => &mut parsed.alt,
                "shift" => &mut parsed.shift,
                "cmd" | "command" | "meta" | "super" | "win" | "windows" => &mut parsed.meta,
                _ => bail!("unknown modifier `{name}` in `{combo}`"),
            };
            *flag = true;
        }
        Ok(Self {
            modifiers: parsed,
            key: parse_key(key)
                .ok_or_else(|| anyhow::anyhow!("unknown key `{key}` in `{combo}`"))?,
        })
    }
}

fn parse_key(name: &str) -> Option<Key> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(Key::Char(c.to_ascii_lowercase()));
    }
    let lower = name.to_ascii_lowercase();
    let key = match lower.as_str() {
        "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "escape" | "esc" => Key::Escape,
        "up" | "arrowup" => Key::Up,
        "down" | "arrowdown" => Key::Down,
        "left" | "arrowleft" => Key::Left,
        "right" | "arrowright" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "pgup" => Key::PageUp,
        "pagedown" | "pgdn" => Key::PageDown,
        "insert" | "ins" => Key::Insert,
        "capslock" => Key::CapsLock,
        "plus" => Key::Char('+'),
        "minus" => Key::Char('-'),
        _ => {
            let n: u8 = lower.strip_prefix('f')?.parse().ok()?;
            return (1..=20).contains(&n).then_some(Key::F(n));
        }
    };
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(combo: &str) -> KeyCombo {
        combo.parse().unwrap()
    }

    #[test]
    fn parses_modifiers_and_key() {
        let combo = parse("cmd+shift+t");
        assert_eq!(
            combo.modifiers,
            Modifiers {
                meta: true,
                shift: true,
                ..Default::default()
            }
        );
        assert_eq!(combo.key, Key::Char('t'));

        let combo = parse("Ctrl+C");
        assert!(combo.modifiers.ctrl);
        assert_eq!(combo.key, Key::Char('c'));
    }

    #[test]
    fn parses_named_keys() {
        assert_eq!(parse("enter").key, Key::Enter);
        assert_eq!(parse("Return").key, Key::Enter);
        assert_eq!(parse("escape").key, Key::Escape);
        assert_eq!(parse("esc").key, Key::Escape);
        assert_eq!(parse("f5").key, Key::F(5));
        assert_eq!(parse("F12").key, Key::F(12));
        assert_eq!(parse("ArrowDown").key, Key::Down);
        assert_eq!(parse("shift+tab").key, Key::Tab);
        assert_eq!(parse("  space ").key, Key::Space);
    }

    #[test]
    fn meta_aliases() {
        for combo in ["cmd+l", "command+l", "super+l", "meta+l", "win+l"] {
            let parsed = parse(combo);
            assert!(parsed.modifiers.meta, "{combo}");
            assert_eq!(parsed.key, Key::Char('l'));
        }
        assert!(parse("option+a").modifiers.alt);
    }

    #[test]
    fn plus_key() {
        assert_eq!(parse("+").key, Key::Char('+'));
        let combo = parse("ctrl++");
        assert!(combo.modifiers.ctrl);
        assert_eq!(combo.key, Key::Char('+'));
        assert_eq!(parse("cmd+plus").key, Key::Char('+'));
    }

    #[test]
    fn rejects_bad_combos() {
        assert!("".parse::<KeyCombo>().is_err());
        assert!("ctrl+".parse::<KeyCombo>().is_err());
        assert!("hyper+a".parse::<KeyCombo>().is_err());
        assert!("ctrl+foo".parse::<KeyCombo>().is_err());
        assert!("f0".parse::<KeyCombo>().is_err());
        assert!("f21".parse::<KeyCombo>().is_err());
    }
}
