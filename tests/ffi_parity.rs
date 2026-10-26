// SPDX-License-Identifier: GPL-3.0-or-later
//! Compare the native image boundary with the Rust/CLI path on identical inputs.
#[cfg(feature = "cli")]
use std::process::Command;
use std::{fs, path::PathBuf, ptr::null_mut};
use uareu::{ffi::*, frame::decode_frame_with_metadata, quality};

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn native_boundary_and_cli_preserve_contrast_pixels_and_metadata() {
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).unwrap();
    let dir = Directory(std::env::temp_dir().join(format!(
        "uareu-ffi-parity-{:032x}",
        u128::from_le_bytes(nonce)
    )));
    fs::create_dir(&dir.0).unwrap();
    for (name, wire) in [
        (
            "plain",
            include_bytes!("../fixtures/synthetic-plain.wire").as_slice(),
        ),
        (
            "scrambled",
            include_bytes!("../fixtures/synthetic-scrambled.wire").as_slice(),
        ),
        (
            "pale",
            include_bytes!("../fixtures/synthetic-plain.wire").as_slice(),
        ),
    ] {
        for rotate in [false, true] {
            let mut captured = decode_frame_with_metadata(wire, rotate, |index| {
                Ok(match index {
                    254 => 0x1367_9245,
                    255 => 0x89ab_cdef,
                    0 => 0xf00d_cafe,
                    _ => panic!("unexpected synthetic key index"),
                })
            })
            .unwrap();
            if name == "pale" {
                // Preserve deliberately low contrast; no auto-levels at the ABI.
                for (index, pixel) in captured.image.pixels.iter_mut().enumerate() {
                    *pixel = 230 + (index % 12) as u8;
                }
                captured.metadata.valid_region = Some(uareu::metadata::Region {
                    x: 0,
                    y: 0,
                    width: captured.image.width,
                    height: captured.image.height,
                });
            }
            let original = dir.0.join(format!("{name}-{rotate}.png"));
            let saved = dir.0.join(format!("{name}-{rotate}-native.png"));
            captured.save_png(&original).unwrap();
            let input_path = original.to_str().unwrap().as_bytes();
            let output_path = saved.to_str().unwrap().as_bytes();
            let mut handle = 0;
            unsafe {
                assert_eq!(
                    uareu_image_load_png(input_path.as_ptr(), input_path.len(), &mut handle),
                    OK
                );
                let mut required = 0;
                assert_eq!(
                    uareu_image_get_pixels(handle, null_mut(), 0, &mut required),
                    BUFFER_TOO_SMALL
                );
                let mut pixels = vec![0; required];
                assert_eq!(
                    uareu_image_get_pixels(
                        handle,
                        pixels.as_mut_ptr(),
                        pixels.len(),
                        &mut required
                    ),
                    OK
                );
                assert_eq!(pixels, captured.image.pixels, "{name}, rotate={rotate}");
                assert_eq!(
                    uareu_image_get_report_json(handle, null_mut(), 0, &mut required),
                    BUFFER_TOO_SMALL
                );
                let mut report = vec![0; required];
                assert_eq!(
                    uareu_image_get_report_json(
                        handle,
                        report.as_mut_ptr(),
                        report.len(),
                        &mut required
                    ),
                    OK
                );
                let native_report: serde_json::Value =
                    serde_json::from_slice(&report[..report.len() - 1]).unwrap();
                let expected: serde_json::Value = serde_json::from_slice(
                    &serde_json::to_vec(&quality::assess(&captured, None).unwrap()).unwrap(),
                )
                .unwrap();
                assert_eq!(native_report, expected);
                assert_eq!(
                    uareu_image_save_png(handle, output_path.as_ptr(), output_path.len()),
                    OK
                );
                assert_eq!(uareu_image_free(handle), OK);
                // Same PNG encoder, pixels and provenance, not merely similar contrast.
                assert_eq!(fs::read(&original).unwrap(), fs::read(&saved).unwrap());
                #[cfg(feature = "cli")]
                {
                    let cli = Command::new(env!("CARGO_BIN_EXE_uareu"))
                        .args(["quality", "--input"])
                        .arg(&saved)
                        .output()
                        .unwrap();
                    assert!(
                        cli.status.success(),
                        "{}",
                        String::from_utf8_lossy(&cli.stderr)
                    );
                    assert_eq!(
                        serde_json::from_slice::<serde_json::Value>(&cli.stdout).unwrap(),
                        native_report
                    );
                }
            }
        }
    }
}
