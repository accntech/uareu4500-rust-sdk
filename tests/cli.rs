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
