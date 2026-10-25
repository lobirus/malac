//! macOS privacy permissions. Without Screen Recording a capture silently
//! contains only the wallpaper, so it is checked before capturing.

use std::path::Path;
use std::process::Command;

use anyhow::{Error, Result, anyhow};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

pub fn require_screen_recording() -> Result<()> {
    // SAFETY: takes no arguments and only queries the permission state.
    if unsafe { CGPreflightScreenCaptureAccess() } {
        return Ok(());
    }
    Err(missing(
        "Screen Recording",
        "Screen & System Audio Recording (Screen Recording before macOS 15)",
    ))
}

pub fn accessibility_error() -> Error {
    missing("Accessibility", "Accessibility")
}

fn missing(permission: &str, pane: &str) -> Error {
    let (name, path) = responsible_app();
    anyhow!(
        "macOS has not granted the {permission} permission to {name} ({path}). Enable it in \
         System Settings > Privacy & Security > {pane}, adding it with + if it is not listed, \
         then restart {name}."
    )
}

/// The name and path of the app macOS attributes permissions to: the app
/// that started the process tree, such as the terminal or editor, or else
/// this binary.
fn responsible_app() -> (String, String) {
    let mut pid = std::process::id();
    for _ in 0..32 {
        let Some((parent, command)) = process_info(pid) else {
            break;
        };
        if let Some(bundle) = app_bundle(&command) {
            return (file_stem(bundle), bundle.to_owned());
        }
        if parent <= 1 {
            break;
        }
        pid = parent;
    }
    let exe = std::env::current_exe()
        .map(|exe| exe.display().to_string())
        .unwrap_or_else(|_| "malac-control".into());
    (file_stem(&exe), exe)
}

fn file_stem(path: &str) -> String {
    Path::new(path).file_stem().map_or_else(
        || path.to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    )
}

fn process_info(pid: u32) -> Option<(u32, String)> {
    let output = Command::new("ps")
        .args(["-o", "ppid=,comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let (parent, command) = text.trim().split_once(char::is_whitespace)?;
    Some((parent.trim().parse().ok()?, command.trim().to_owned()))
}

/// The outermost `.app` bundle in an executable path, so helper apps nested
/// inside another bundle resolve to the app the user sees.
fn app_bundle(command: &str) -> Option<&str> {
    let end = command.find(".app/")? + ".app".len();
    Some(&command[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_outermost_bundle() {
        assert_eq!(
            app_bundle("/Applications/iTerm.app/Contents/MacOS/iTerm2"),
            Some("/Applications/iTerm.app")
        );
        assert_eq!(
            app_bundle(
                "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper"
            ),
            Some("/Applications/Visual Studio Code.app")
        );
        assert_eq!(app_bundle("/bin/zsh"), None);
    }
}
