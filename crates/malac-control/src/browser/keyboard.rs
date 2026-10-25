//! `Input.dispatchKeyEvent` parameters for a [`KeyCombo`].

use serde_json::{Value, json};

use crate::keys::{Key, KeyCombo, Modifiers};

struct KeyDef {
    key: String,
    code: String,
    key_code: u32,
    text: String,
}

impl KeyDef {
    fn named(name: &str, key_code: u32, text: &str) -> Self {
        Self {
            key: name.into(),
            code: name.into(),
            key_code,
            text: text.into(),
        }
    }
}

/// The key-down and key-up events for `combo`.
pub fn key_events(combo: &KeyCombo) -> [Value; 2] {
    let def = define(combo.key, combo.modifiers.shift);
    let m = combo.modifiers;
    // Shortcuts must not insert their character.
    let text = if m.ctrl || m.alt || m.meta {
        ""
    } else {
        def.text.as_str()
    };
    let modifiers = modifier_mask(m);
    let down = json!({
        "type": if text.is_empty() { "rawKeyDown" } else { "keyDown" },
        "key": def.key,
        "code": def.code,
        "windowsVirtualKeyCode": def.key_code,
        "modifiers": modifiers,
        "text": text,
        "unmodifiedText": text,
    });
    let up = json!({
        "type": "keyUp",
        "key": def.key,
        "code": def.code,
        "windowsVirtualKeyCode": def.key_code,
        "modifiers": modifiers,
    });
    [down, up]
}

fn modifier_mask(m: Modifiers) -> u8 {
    u8::from(m.alt) | u8::from(m.ctrl) << 1 | u8::from(m.meta) << 2 | u8::from(m.shift) << 3
}

fn define(key: Key, shift: bool) -> KeyDef {
    match key {
        Key::Char(c) => {
            let typed = if shift { c.to_ascii_uppercase() } else { c };
            let code = if c.is_ascii_alphabetic() {
                format!("Key{}", c.to_ascii_uppercase())
            } else if c.is_ascii_digit() {
                format!("Digit{c}")
            } else {
                String::new()
            };
            let key_code = if c.is_ascii_alphanumeric() {
                u32::from(c.to_ascii_uppercase())
            } else {
                0
            };
            KeyDef {
                key: typed.to_string(),
                code,
                key_code,
                text: typed.to_string(),
            }
        }
        Key::Enter => KeyDef::named("Enter", 13, "\r"),
        Key::Tab => KeyDef::named("Tab", 9, ""),
        Key::Space => KeyDef {
            key: " ".into(),
            code: "Space".into(),
            key_code: 32,
            text: " ".into(),
        },
        Key::Backspace => KeyDef::named("Backspace", 8, ""),
        Key::Delete => KeyDef::named("Delete", 46, ""),
        Key::Escape => KeyDef::named("Escape", 27, ""),
        Key::Up => KeyDef::named("ArrowUp", 38, ""),
        Key::Down => KeyDef::named("ArrowDown", 40, ""),
        Key::Left => KeyDef::named("ArrowLeft", 37, ""),
        Key::Right => KeyDef::named("ArrowRight", 39, ""),
        Key::Home => KeyDef::named("Home", 36, ""),
        Key::End => KeyDef::named("End", 35, ""),
        Key::PageUp => KeyDef::named("PageUp", 33, ""),
        Key::PageDown => KeyDef::named("PageDown", 34, ""),
        Key::Insert => KeyDef::named("Insert", 45, ""),
        Key::CapsLock => KeyDef::named("CapsLock", 20, ""),
        Key::F(n) => KeyDef::named(&format!("F{n}"), 111 + u32::from(n), ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(combo: &str) -> [Value; 2] {
        key_events(&combo.parse().unwrap())
    }

    #[test]
    fn enter_types_carriage_return() {
        let [down, up] = events("enter");
        assert_eq!(down["type"], "keyDown");
        assert_eq!(down["key"], "Enter");
        assert_eq!(down["windowsVirtualKeyCode"], 13);
        assert_eq!(down["text"], "\r");
        assert_eq!(up["type"], "keyUp");
    }

    #[test]
    fn shortcuts_do_not_insert_text() {
        let [down, _] = events("ctrl+a");
        assert_eq!(down["type"], "rawKeyDown");
        assert_eq!(down["code"], "KeyA");
        assert_eq!(down["windowsVirtualKeyCode"], 65);
        assert_eq!(down["modifiers"], 2);
        assert_eq!(down["text"], "");
    }

    #[test]
    fn shift_uppercases_letters() {
        let [down, _] = events("shift+t");
        assert_eq!(down["key"], "T");
        assert_eq!(down["text"], "T");
        assert_eq!(down["modifiers"], 8);
    }

    #[test]
    fn named_keys() {
        assert_eq!(events("ArrowDown")[0]["key"], "ArrowDown");
        assert_eq!(events("f5")[0]["windowsVirtualKeyCode"], 116);
        assert_eq!(events("cmd+shift+z")[0]["modifiers"], 4 | 8);
    }
}
