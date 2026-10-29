//! malac-control holds its macOS permissions itself, not the app that
//! started it (here, whatever runs the tests).

#![cfg(target_os = "macos")]

use std::process::Command;

#[test]
fn holds_its_own_permissions() {
    let output = Command::new(env!("CARGO_BIN_EXE_malac-control"))
        .args(["permissions", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["app"]["name"], "malac-control", "{report}");
    assert!(!report["requested"].as_bool().unwrap());
}

#[test]
fn gives_itself_the_malac_icon() {
    // A copy without extended attributes, like a freshly updated binary,
    // that no other test runs at the same time.
    let dir = std::env::temp_dir().join(format!("malac-control-icon-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("malac-control");
    std::fs::copy(env!("CARGO_BIN_EXE_malac-control"), &exe).unwrap();
    assert!(
        Command::new("xattr")
            .arg("-c")
            .arg(&exe)
            .status()
            .unwrap()
            .success()
    );

    let status = Command::new(&exe).arg("--version").status().unwrap();
    assert!(status.success());
    // The Finder flags are bytes 8 and 9 of com.apple.FinderInfo; 0x0400 is a custom icon.
    let output = Command::new("xattr")
        .args(["-px", "com.apple.FinderInfo"])
        .arg(&exe)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let hex = String::from_utf8_lossy(&output.stdout);
    let bytes: Vec<u8> = hex
        .split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).unwrap())
        .collect();
    assert!(bytes.len() >= 10, "no Finder info: {output:?}");
    assert_ne!(
        u16::from_be_bytes([bytes[8], bytes[9]]) & 0x0400,
        0,
        "{hex}"
    );
}
