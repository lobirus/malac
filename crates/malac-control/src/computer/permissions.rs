//! Privacy permissions for desktop control. macOS needs Screen Recording for
//! screenshots (without it a capture silently contains only the wallpaper)
//! and Accessibility for mouse and keyboard input. It grants both to the
//! "responsible" app of the process tree, usually the terminal or editor
//! that started it, not to `malac-control` itself.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub platform: String,
    /// `None` where the platform has no such permission.
    pub screen_recording: Option<bool>,
    pub accessibility: Option<bool>,
    /// The app the permissions belong to.
    pub app: Option<App>,
    /// Whether the system prompts and settings were opened.
    pub requested: bool,
    /// What to do next, in one or two sentences.
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct App {
    pub name: String,
    pub path: String,
}

impl Report {
    pub fn granted(&self) -> bool {
        self.screen_recording != Some(false) && self.accessibility != Some(false)
    }
}

/// The current permission state. With `request`, macOS shows its prompts
/// for missing permissions and System Settings opens at the matching pane.
pub fn report(request: bool) -> Report {
    imp::report(request)
}

#[cfg(target_os = "macos")]
pub use imp::{accessibility_error, require_screen_recording};

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{CStr, c_char, c_void};
    use std::path::Path;
    use std::process::Command;

    use anyhow::{Error, Result, anyhow};

    use super::{App, Report};

    const SCREEN_PANE: &str =
        "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
    const ACCESSIBILITY_PANE: &str =
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
        static kAXTrustedCheckOptionPrompt: *const c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFBooleanTrue: *const c_void;
        static kCFTypeDictionaryKeyCallBacks: u8;
        static kCFTypeDictionaryValueCallBacks: u8;
        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            count: isize,
            key_callbacks: *const u8,
            value_callbacks: *const u8,
        ) -> *const c_void;
        fn CFRelease(object: *const c_void);
    }

    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn proc_pidpath(pid: i32, buffer: *mut c_void, size: u32) -> i32;
    }

    /// `RTLD_DEFAULT` on Darwin.
    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
    const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;

    fn screen_recording() -> bool {
        // SAFETY: takes no arguments and only queries the permission state.
        unsafe { CGPreflightScreenCaptureAccess() }
    }

    fn accessibility() -> bool {
        // SAFETY: takes no arguments and only queries the permission state.
        unsafe { AXIsProcessTrusted() }
    }

    fn request_screen_recording() {
        // SAFETY: takes no arguments; shows the system prompt at most once.
        unsafe {
            CGRequestScreenCaptureAccess();
        }
    }

    fn request_accessibility() {
        // SAFETY: the dictionary holds two immortal CoreFoundation constants
        // and is released after use.
        unsafe {
            let keys = [kAXTrustedCheckOptionPrompt];
            let values = [kCFBooleanTrue];
            let options = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &raw const kCFTypeDictionaryKeyCallBacks,
                &raw const kCFTypeDictionaryValueCallBacks,
            );
            if options.is_null() {
                return;
            }
            AXIsProcessTrustedWithOptions(options);
            CFRelease(options);
        }
    }

    pub fn report(request: bool) -> Report {
        let app = responsible_app();
        let mut screen = screen_recording();
        let mut access = accessibility();
        if request {
            if !screen {
                request_screen_recording();
                open_settings(SCREEN_PANE);
            }
            if !access {
                request_accessibility();
                open_settings(ACCESSIBILITY_PANE);
            }
            screen = screen_recording();
            access = accessibility();
        }
        let hint = if screen && access {
            format!("{} has Screen Recording and Accessibility.", app.name)
        } else {
            let missing: Vec<&str> = [(!screen, "Screen Recording"), (!access, "Accessibility")]
                .into_iter()
                .filter_map(|(missing, name)| missing.then_some(name))
                .collect();
            format!(
                "Grant {} to {} in System Settings > Privacy & Security, then restart {}{}.",
                missing.join(" and "),
                app.name,
                app.name,
                restart_hint(&app)
            )
        };
        Report {
            platform: "macos".into(),
            screen_recording: Some(screen),
            accessibility: Some(access),
            app: Some(app),
            requested: request,
            hint,
        }
    }

    pub fn require_screen_recording() -> Result<()> {
        if screen_recording() {
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
        let app = responsible_app();
        anyhow!(
            "macOS has not granted the {permission} permission to {} ({}), the app this control \
             service runs under. Enable {} in System Settings > Privacy & Security > {pane}, adding \
             it with + if it is not listed (`malac-control permissions --request` opens the pane), \
             then restart {}{}.",
            app.name,
            app.path,
            app.name,
            app.name,
            restart_hint(&app)
        )
    }

    /// When the permissions belong to malac itself (a daemon started without
    /// a terminal), restarting the daemon is what applies them.
    fn restart_hint(app: &App) -> &'static str {
        if app.name.starts_with("malac") {
            " (`malac daemon restart`)"
        } else {
            " and the malac daemon (`malac daemon restart`)"
        }
    }

    fn open_settings(url: &str) {
        let _ = Command::new("open").arg(url).status();
    }

    /// The app macOS attributes this process's permissions to. Asks the
    /// system for the responsible process, which survives `setsid` and
    /// re-parenting, and falls back to walking the parent processes.
    fn responsible_app() -> App {
        let path = responsible_pid()
            .and_then(process_path)
            .or_else(parent_app)
            .unwrap_or_else(|| {
                std::env::current_exe()
                    .map(|exe| exe.display().to_string())
                    .unwrap_or_else(|_| "malac-control".into())
            });
        let path = app_bundle(&path).unwrap_or(&path).to_owned();
        App {
            name: file_stem(&path),
            path,
        }
    }

    fn responsible_pid() -> Option<i32> {
        type Responsible = unsafe extern "C" fn(i32) -> i32;
        let name = c"responsibility_get_pid_responsible_for_pid";
        // SAFETY: looks up a libSystem function by name; a missing symbol yields null.
        let symbol = unsafe { dlsym(RTLD_DEFAULT, name.as_ptr()) };
        if symbol.is_null() {
            return None;
        }
        // SAFETY: the symbol has this signature on every macOS release that exports it.
        let responsible: Responsible = unsafe { std::mem::transmute(symbol) };
        let pid = unsafe { responsible(std::process::id() as i32) };
        (pid > 0).then_some(pid)
    }

    fn process_path(pid: i32) -> Option<String> {
        let mut buffer = [0u8; PROC_PIDPATHINFO_MAXSIZE];
        // SAFETY: the buffer is as large as the size passed.
        let len = unsafe {
            proc_pidpath(
                pid,
                buffer.as_mut_ptr().cast(),
                PROC_PIDPATHINFO_MAXSIZE as u32,
            )
        };
        if len <= 0 {
            return None;
        }
        let path = CStr::from_bytes_until_nul(&buffer).ok()?;
        Some(path.to_string_lossy().into_owned())
    }

    /// The first `.app` bundle among the parent processes.
    fn parent_app() -> Option<String> {
        let mut pid = std::process::id();
        for _ in 0..32 {
            let (parent, command) = process_info(pid)?;
            if let Some(bundle) = app_bundle(&command) {
                return Some(bundle.to_owned());
            }
            if parent <= 1 {
                return None;
            }
            pid = parent;
        }
        None
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

    /// The outermost `.app` bundle in an executable path, so helper apps
    /// nested inside another bundle resolve to the app the user sees.
    pub(super) fn app_bundle(command: &str) -> Option<&str> {
        let end = command.find(".app/")? + ".app".len();
        Some(&command[..end])
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::Report;

    pub fn report(request: bool) -> Report {
        Report {
            platform: std::env::consts::OS.into(),
            screen_recording: None,
            accessibility: None,
            app: None,
            requested: request,
            hint: "This platform needs no extra permissions for desktop control.".into(),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::imp::app_bundle;

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

    #[test]
    fn reports_without_prompting() {
        let report = super::report(false);
        assert_eq!(report.platform, "macos");
        assert!(report.screen_recording.is_some());
        assert!(!report.requested);
        assert!(report.app.is_some());
    }
}
