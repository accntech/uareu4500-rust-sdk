// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

#[test]
fn help_exposes_device_listing_and_capture() {
    let output = Command::new(env!("CARGO_BIN_EXE_uareu4500"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("list"), "{help}");
    assert!(help.contains("capture"), "{help}");
}

#[test]
fn capture_rejects_zero_timeout_before_accessing_usb() {
    let output = Command::new(env!("CARGO_BIN_EXE_uareu4500"))
        .args(["capture", "--output", "unused.png", "--timeout", "0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("timeout"));
}

#[test]
fn capture_requires_an_explicit_output_file() {
    let output = Command::new(env!("CARGO_BIN_EXE_uareu4500"))
        .arg("capture")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--output"));
}

#[test]
fn license_command_shows_gpl_grant_attribution_and_warranty_terms() {
    let output = Command::new(env!("CARGO_BIN_EXE_uareu4500"))
        .arg("license")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("GPL-3.0-or-later"));
    assert!(text.contains("Daniel Drake"));
    assert!(text.contains("Timo Ter"));
    assert!(text.contains("GNU GENERAL PUBLIC LICENSE"));
    assert!(text.contains("NO WARRANTY") || text.contains("Disclaimer of Warranty"));
}

#[test]
fn capture_help_documents_continuous_mode_and_directory() {
    let output = Command::new(env!("CARGO_BIN_EXE_uareu4500"))
        .args(["capture", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--continuous"), "{help}");
    assert!(help.contains("--output-dir"), "{help}");
}

#[test]
fn continuous_mode_rejects_single_output_before_usb_access() {
    let output = Command::new(env!("CARGO_BIN_EXE_uareu4500"))
        .args(["capture", "--continuous", "--output", "unused.png"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("cannot be used with"), "{error}");
}

#[test]
fn output_directory_requires_continuous_mode_before_usb_access() {
    let output = Command::new(env!("CARGO_BIN_EXE_uareu4500"))
        .args([
            "capture",
            "--output",
            "unused.png",
            "--output-dir",
            "unused",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("--continuous"), "{error}");
    assert!(!error.contains("Initializing reader"), "{error}");
}
