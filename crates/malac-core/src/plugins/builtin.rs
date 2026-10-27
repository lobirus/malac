//! Plugins that ship inside the malac binary and are enabled by default
//! (`[plugins] defaults`). They are written to `~/.malac/plugins/builtin/`
//! on first use, so they work offline and never change behind the user's
//! back: a new copy arrives only with a new malac release.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The marketplace name of built-in plugins: `ponytail@malac`.
pub const MARKETPLACE: &str = "malac";

pub struct Builtin {
    pub name: &'static str,
    files: &'static [(&'static str, &'static str)],
}

impl Builtin {
    pub fn id(&self) -> String {
        format!("{}@{MARKETPLACE}", self.name)
    }
}

macro_rules! vendored {
    ($plugin:literal: $($path:literal),+ $(,)?) => {
        &[$(($path, include_str!(concat!("../../plugins/", $plugin, "/", $path)))),+]
    };
}

/// ponytail (MIT, https://ponytail.dev): its skills, unchanged.
const PONYTAIL: Builtin = Builtin {
    name: "ponytail",
    files: vendored!("ponytail":
        "plugin.json",
        "LICENSE",
        "NOTICE.md",
        "skills/ponytail/SKILL.md",
        "skills/ponytail-review/SKILL.md",
        "skills/ponytail-audit/SKILL.md",
        "skills/ponytail-debt/SKILL.md",
        "skills/ponytail-gain/SKILL.md",
        "skills/ponytail-help/SKILL.md",
    ),
};

/// malac's own design guidance: the house style, how to follow an existing
/// product's design language, and how to check a UI.
const DESIGN: Builtin = Builtin {
    name: "design",
    files: vendored!("design":
        "plugin.json",
        "skills/design/SKILL.md",
        "skills/design/references/house-style.md",
        "skills/design/references/tokens.css",
        "skills/design/references/existing-product.md",
        "skills/design/references/polish.md",
        "skills/design/references/verify.md",
        "skills/design/references/audit.js",
        "skills/design-review/SKILL.md",
    ),
};

pub fn all() -> &'static [Builtin] {
    &[PONYTAIL, DESIGN]
}

pub fn get(name: &str) -> Option<&'static Builtin> {
    all().iter().find(|b| b.name == name)
}

/// Writes the plugin under `dir` unless the copy there is current, and
/// returns its root.
pub fn materialize(builtin: &Builtin, dir: &Path) -> Result<PathBuf> {
    let root = dir.join(builtin.name);
    let current = builtin.files.iter().all(|(path, text)| {
        std::fs::read_to_string(root.join(path)).is_ok_and(|on_disk| on_disk == *text)
    });
    if current {
        return Ok(root);
    }
    for (path, text) in builtin.files {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ponytail_is_a_valid_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let root = materialize(&PONYTAIL, dir.path()).unwrap();
        let parsed = super::super::manifest::parse(&root, &root, None).unwrap();
        assert_eq!(parsed.meta.name, "ponytail");
        assert_eq!(parsed.meta.license.as_deref(), Some("MIT"));
        assert_eq!(parsed.components.skills.len(), 6);
        assert!(parsed.components.hooks.is_empty() && parsed.components.mcp.is_empty());
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        // A second call finds the copy current and leaves it alone.
        std::fs::write(root.join("extra.txt"), "x").unwrap();
        materialize(&PONYTAIL, dir.path()).unwrap();
        assert!(root.join("extra.txt").exists());
    }

    #[test]
    fn design_is_a_valid_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let root = materialize(&DESIGN, dir.path()).unwrap();
        let parsed = super::super::manifest::parse(&root, &root, None).unwrap();
        assert_eq!(parsed.meta.name, "design");
        assert_eq!(parsed.components.skills.len(), 2);
        // References only: nothing to run, so nothing needs approving.
        assert!(parsed.components.scripts.is_empty());
        assert!(parsed.components.hooks.is_empty() && parsed.components.mcp.is_empty());
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        for reference in [
            "house-style.md",
            "tokens.css",
            "existing-product.md",
            "polish.md",
            "verify.md",
            "audit.js",
        ] {
            let path = root.join("skills/design/references").join(reference);
            assert!(path.is_file(), "{reference} is missing");
        }
    }
}
