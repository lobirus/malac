//! Frontmatter of Markdown files (skills, agents, commands, rules): the
//! subset of YAML they use. Scalars may be plain, quoted or `>`/`|` blocks;
//! lists inline (`[a, b]`) or one `- item` per line; maps one level deep.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    List(Vec<String>),
    Map(BTreeMap<String, String>),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frontmatter(BTreeMap<String, Value>);

/// Splits `---` delimited frontmatter from the body. Accepts CRLF line
/// endings and a byte order mark.
pub fn split(text: &str) -> (&str, &str) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return ("", text);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return (&rest[..offset], &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    ("", text)
}

/// Parses the frontmatter of `text` and returns it with the body.
pub fn parse(text: &str) -> (Frontmatter, &str) {
    let (front, body) = split(text);
    (Frontmatter::parse(front), body)
}

impl Frontmatter {
    pub fn parse(front: &str) -> Self {
        let lines: Vec<&str> = front.lines().map(|l| l.trim_end_matches('\r')).collect();
        let mut fields = BTreeMap::new();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i];
            i += 1;
            if line.trim().is_empty() || line.starts_with('#') || line.starts_with([' ', '\t']) {
                continue;
            }
            let Some((key, rest)) = line.split_once(':') else {
                continue;
            };
            let key = unquote(key.trim());
            let rest = rest.trim();
            // The indented lines that belong to this key.
            let start = i;
            while i < lines.len()
                && (lines[i].starts_with([' ', '\t']) || lines[i].trim().is_empty())
            {
                i += 1;
            }
            let nested = &lines[start..i];
            let value = if rest.starts_with('>') || rest.starts_with('|') {
                Value::Str(block_scalar(rest, nested))
            } else if rest.starts_with('[') {
                Value::List(inline_list(rest))
            } else if !rest.is_empty() {
                let mut value = scalar(rest);
                // A plain scalar may continue on indented lines.
                if !rest.starts_with(['"', '\'']) {
                    for more in nested.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
                        value.push(' ');
                        value.push_str(more);
                    }
                }
                Value::Str(value)
            } else if nested
                .iter()
                .any(|l| l.trim_start().starts_with("- ") || l.trim() == "-")
            {
                Value::List(
                    nested
                        .iter()
                        .filter_map(|l| l.trim_start().strip_prefix('-'))
                        .map(|item| scalar(item.trim()))
                        .filter(|item| !item.is_empty())
                        .collect(),
                )
            } else {
                Value::Map(
                    nested
                        .iter()
                        .filter_map(|l| l.trim().split_once(':'))
                        .map(|(k, v)| (unquote(k.trim()), scalar(v.trim())))
                        .collect(),
                )
            };
            fields.insert(key, value);
        }
        Self(fields)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    /// A non-empty string value.
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.0.get(key) {
            Some(Value::Str(s)) if !s.is_empty() => Some(s),
            _ => None,
        }
    }

    pub fn string(&self, key: &str) -> Option<String> {
        self.str(key).map(str::to_owned)
    }

    /// A list, also accepted as one string separated by commas (or, with
    /// `by_space`, whitespace).
    pub fn list(&self, key: &str, by_space: bool) -> Option<Vec<String>> {
        let items: Vec<String> = match self.0.get(key)? {
            Value::List(items) => items.clone(),
            Value::Str(s) if by_space && !s.contains(',') => {
                s.split_whitespace().map(str::to_owned).collect()
            }
            Value::Str(s) => s.split(',').map(|i| i.trim().to_owned()).collect(),
            Value::Map(_) => return None,
        };
        Some(items.into_iter().filter(|i| !i.is_empty()).collect())
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.str(key)?.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" => Some(true),
            "false" | "no" | "off" => Some(false),
            _ => None,
        }
    }

    pub fn map(&self, key: &str) -> Option<&BTreeMap<String, String>> {
        match self.0.get(key) {
            Some(Value::Map(map)) => Some(map),
            _ => None,
        }
    }
}

/// A quoted or plain scalar; plain ones lose a trailing ` # comment`.
fn scalar(text: &str) -> String {
    if text.starts_with(['"', '\'']) {
        return unquote(text);
    }
    let text = match text.find(" #") {
        Some(at) => &text[..at],
        None => text,
    };
    text.trim().to_owned()
}

fn unquote(text: &str) -> String {
    let text = text.trim();
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        let inner = &text[1..text.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        }
        return out;
    }
    if text.len() >= 2 && text.starts_with('\'') && text.ends_with('\'') {
        return text[1..text.len() - 1].replace("''", "'");
    }
    text.to_owned()
}

fn inline_list(text: &str) -> Vec<String> {
    let inner = text.trim_start_matches('[');
    let inner = inner.rsplit_once(']').map_or(inner, |(inner, _)| inner);
    inner
        .split(',')
        .map(|item| scalar(item.trim()))
        .filter(|item| !item.is_empty())
        .collect()
}

/// `>` folds lines into one paragraph per blank-line-separated run; `|`
/// keeps them. Trailing newlines are dropped either way.
fn block_scalar(indicator: &str, lines: &[&str]) -> String {
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let lines: Vec<&str> = lines
        .iter()
        .map(|l| if l.len() >= indent { &l[indent..] } else { "" })
        .collect();
    let text = if indicator.starts_with('|') {
        lines.join("\n")
    } else {
        let mut out = String::new();
        for line in lines {
            if line.trim().is_empty() {
                out.push('\n');
            } else {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push(' ');
                }
                out.push_str(line.trim_end());
            }
        }
        out
    };
    text.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skill_frontmatter() {
        let text = "---\nname: ponytail\ndescription: >\n  Lazy senior dev mode: the smallest\n  change that works.\nargument-hint: \"[lite|full]\"\nlicense: MIT\nmetadata:\n  author: example\n  version: \"1.0\"\n---\n\n# Body\n";
        let (front, body) = parse(text);
        assert_eq!(front.str("name"), Some("ponytail"));
        assert_eq!(
            front.str("description"),
            Some("Lazy senior dev mode: the smallest change that works.")
        );
        assert_eq!(front.str("argument-hint"), Some("[lite|full]"));
        assert_eq!(front.map("metadata").unwrap()["version"], "1.0");
        assert_eq!(body.trim(), "# Body");
    }

    #[test]
    fn parses_lists_and_bools() {
        let text = "---\r\ntools: Read, Grep, Glob\nglobs: [\"src/**/*.ts\", '*.tsx']\nalwaysApply: false\nkeywords:\n  - one\n  - \"two\"\nallowed-tools: Bash(git:*) Read\n---\r\nbody";
        let (front, body) = parse(text);
        assert_eq!(
            front.list("tools", false).unwrap(),
            vec!["Read", "Grep", "Glob"]
        );
        assert_eq!(
            front.list("globs", false).unwrap(),
            vec!["src/**/*.ts", "*.tsx"]
        );
        assert_eq!(front.bool("alwaysApply"), Some(false));
        assert_eq!(front.list("keywords", false).unwrap(), vec!["one", "two"]);
        assert_eq!(
            front.list("allowed-tools", true).unwrap(),
            vec!["Bash(git:*)", "Read"]
        );
        assert_eq!(body, "body");
    }

    #[test]
    fn plain_text_has_no_frontmatter() {
        let (front, body) = parse("# Title\n---\n");
        assert!(front.is_empty());
        assert_eq!(body, "# Title\n---\n");
        let (front, _) = parse("---\nkey: value # note\nliteral: |\n  a\n  b\n---\n");
        assert_eq!(front.str("key"), Some("value"));
        assert_eq!(front.str("literal"), Some("a\nb"));
    }
}
