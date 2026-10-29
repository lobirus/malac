//! Privacy permissions for desktop control. macOS needs Screen Recording for
//! screenshots (without it a capture silently contains only the wallpaper)
//! and Accessibility for mouse and keyboard input. It grants both to the
//! "responsible" app of the process tree, which is the terminal or editor
//! that started it unless the process disclaims that: [`own_permissions`]
//! makes `malac-control` hold them itself, whoever starts it.

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

/// Makes this process hold its own permissions on macOS, so the system asks
/// for and remembers them as `malac-control`'s, not as those of the
/// terminal, editor or daemon that started it. When another app is
/// responsible, the process re-executes itself in place (same pid,
/// arguments and open files) with that responsibility disclaimed. Call it
/// first thing in `main`: on success it does not return to the old image.
/// Elsewhere, and when the process already holds them, it does nothing.
pub fn own_permissions() -> anyhow::Result<()> {
    imp::own_permissions()
}

/// Gives the `malac-control` executable malac's icon on macOS, so System
/// Settings shows it in the permission lists instead of a generic program
/// icon. A file's custom icon lives in its extended attributes, which an
/// update drops with the old file, so this sets it again whenever it is
/// missing. The attributes do not touch the signature or the grants.
/// Elsewhere it does nothing.
pub fn show_icon() -> anyhow::Result<()> {
    imp::show_icon()
}

#[cfg(target_os = "macos")]
pub use imp::{accessibility_error, require_screen_recording};

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::Path;
    use std::process::Command;
    use std::ptr;

    use anyhow::{Context, Error, Result, anyhow, bail};
    use objc2::AnyThread;
    use objc2_app_kit::{NSImage, NSWorkspace, NSWorkspaceIconCreationOptions};
    use objc2_foundation::{NSData, NSString};

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
        fn getxattr(
            path: *const c_char,
            name: *const c_char,
            value: *mut c_void,
            size: usize,
            position: u32,
            options: i32,
        ) -> isize;
        fn posix_spawnattr_init(attr: *mut *mut c_void) -> i32;
        fn posix_spawnattr_setflags(attr: *mut *mut c_void, flags: i16) -> i32;
        fn posix_spawnattr_destroy(attr: *mut *mut c_void) -> i32;
        fn posix_spawn(
            pid: *mut i32,
            path: *const c_char,
            file_actions: *const c_void,
            attr: *const *mut c_void,
            argv: *const *const c_char,
            envp: *const *const c_char,
        ) -> i32;
    }

    /// `RTLD_DEFAULT` on Darwin.
    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
    const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;
    /// Apple's extension: `posix_spawn` replaces the calling process, like `execve`.
    const POSIX_SPAWN_SETEXEC: i16 = 0x0040;
    /// Set for the re-executed process, so a system that ignores the
    /// disclaimer cannot make it re-execute forever.
    const OWN_PERMISSIONS_ENV: &str = "MALAC_CONTROL_OWN_PERMISSIONS";
    /// malac's pig on macOS's rounded icon tile, made from the iOS app icon
    /// (`apps/ios/Malac/Resources/Assets.xcassets/AppIcon.appiconset`).
    const ICON: &[u8] = include_bytes!("../../assets/malac-control.icns");
    /// `kHasCustomIcon`, in the Finder flags at bytes 8 and 9 of `com.apple.FinderInfo`.
    const HAS_CUSTOM_ICON: u16 = 0x0400;

    pub fn own_permissions() -> Result<()> {
        if std::env::var_os(OWN_PERMISSIONS_ENV).is_some()
            || responsible_pid().is_none_or(|pid| pid == std::process::id() as i32)
        {
            return Ok(());
        }
        type Disclaim = unsafe extern "C" fn(*mut *mut c_void, i32) -> i32;
        let symbol = lookup(c"responsibility_spawnattrs_setdisclaim")
            .context("this macOS cannot disclaim responsibility")?;
        // SAFETY: the symbol has this signature on every macOS release that exports it.
        let disclaim: Disclaim = unsafe { std::mem::transmute(symbol) };

        let exe = std::env::current_exe().context("cannot find this executable")?;
        let exe = CString::new(exe.into_os_string().into_vec())?;
        let args = std::env::args_os()
            .map(|arg| CString::new(arg.into_vec()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut env = std::env::vars_os()
            .map(|(key, value)| {
                let mut entry = key.into_vec();
                entry.push(b'=');
                entry.extend(value.into_vec());
                CString::new(entry)
            })
            .collect::<Result<Vec<_>, _>>()?;
        env.push(CString::new(format!("{OWN_PERMISSIONS_ENV}=1"))?);
        let argv = null_terminated(&args);
        let envp = null_terminated(&env);

        let mut attr: *mut c_void = ptr::null_mut();
        // SAFETY: `attr` is initialized before use and destroyed after; the
        // path, argument and environment arrays are null-terminated and
        // outlive the call, which on success never returns.
        unsafe {
            check(posix_spawnattr_init(&mut attr))?;
            let result = check(posix_spawnattr_setflags(&mut attr, POSIX_SPAWN_SETEXEC))
                .and_then(|()| check(disclaim(&mut attr, 1)))
                .and_then(|()| {
                    let mut pid = 0;
                    check(posix_spawn(
                        &mut pid,
                        exe.as_ptr(),
                        ptr::null(),
                        &attr,
                        argv.as_ptr(),
                        envp.as_ptr(),
                    ))
                });
            posix_spawnattr_destroy(&mut attr);
            result.context("cannot re-execute with its own permissions")
        }
    }

    pub fn show_icon() -> Result<()> {
        // The real file, which is what System Settings lists, not a symlink to it.
        let exe = std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .context("cannot find this executable")?;
        if has_custom_icon(&exe) {
            return Ok(());
        }
        let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(ICON))
            .context("cannot read the icon")?;
        let path = NSString::from_str(&exe.to_string_lossy());
        let workspace = NSWorkspace::sharedWorkspace();
        if !workspace.setIcon_forFile_options(
            Some(&image),
            &path,
            NSWorkspaceIconCreationOptions::empty(),
        ) {
            bail!("cannot set the icon of {}", exe.display());
        }
        Ok(())
    }

    fn has_custom_icon(path: &Path) -> bool {
        let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        let mut info = [0u8; 32];
        // SAFETY: both names are null-terminated and the buffer is as large as the size passed.
        let len = unsafe {
            getxattr(
                path.as_ptr(),
                c"com.apple.FinderInfo".as_ptr(),
                info.as_mut_ptr().cast(),
                info.len(),
                0,
                0,
            )
        };
        len >= 10 && u16::from_be_bytes([info[8], info[9]]) & HAS_CUSTOM_ICON != 0
    }

    fn null_terminated(strings: &[CString]) -> Vec<*const c_char> {
        strings
            .iter()
            .map(|string| string.as_ptr())
            .chain([ptr::null()])
            .collect()
    }

    /// The spawn functions return an error number instead of setting `errno`.
    fn check(code: i32) -> std::io::Result<()> {
        match code {
            0 => Ok(()),
            code => Err(std::io::Error::from_raw_os_error(code)),
        }
    }

    /// A libSystem function by name; null when this macOS does not export it.
    fn lookup(name: &CStr) -> Option<*mut c_void> {
        // SAFETY: looks up a symbol by a null-terminated name.
        let symbol = unsafe { dlsym(RTLD_DEFAULT, name.as_ptr()) };
        (!symbol.is_null()).then_some(symbol)
    }

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
                "Grant {} to {} in System Settings > Privacy & Security, then {}.",
                missing.join(" and "),
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
            "macOS has not granted the {permission} permission to {} ({}), which holds this \
             control service's permissions. Turn it on in System Settings > Privacy & Security > \
             {pane}, adding it with + if it is not listed (`malac-control permissions --request` \
             opens the pane), then {}. If it is listed and on, remove it with - and add it again: \
             macOS forgets the grant when a build without a stable signature is replaced.",
            app.name,
            app.path,
            restart_hint(&app)
        )
    }

    /// Restarting the daemon starts a new control service, which picks up
    /// the grant; when another app holds the permissions, that app too.
    fn restart_hint(app: &App) -> String {
        if app.name.starts_with("malac") {
            "restart it (`malac daemon restart`)".into()
        } else {
            format!(
                "restart {} and the malac daemon (`malac daemon restart`)",
                app.name
            )
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
        let symbol = lookup(c"responsibility_get_pid_responsible_for_pid")?;
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

    pub fn own_permissions() -> anyhow::Result<()> {
        Ok(())
    }

    pub fn show_icon() -> anyhow::Result<()> {
        Ok(())
    }

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
