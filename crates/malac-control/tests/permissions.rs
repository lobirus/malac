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
