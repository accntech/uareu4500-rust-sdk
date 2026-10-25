// SPDX-License-Identifier: GPL-3.0-or-later
// Deliberately not a codec: tests host process/NISTCOM handling only.
use std::{env, fs, path::PathBuf, time::Duration};
fn main() {
    let mode =
        fs::read_to_string(env::current_exe().unwrap().parent().unwrap().join("mode")).unwrap();
    if mode == "failure" {
        std::process::exit(9);
    }
    if mode == "hang" {
        std::thread::sleep(Duration::from_secs(60));
    }
    if mode.starts_with("encode") {
        let args: Vec<_> = env::args().collect();
        let rate: f64 = args[1].parse().unwrap();
        assert_eq!(args[4], "-raw_in");
        assert_eq!(args[5], "256,256,8,500");
        if mode == "encode_then_fail" && rate < 2.0 {
            std::process::exit(9);
        }
        if mode == "encode_then_hang" && rate < 2.0 {
            std::thread::sleep(Duration::from_secs(60));
        }
        let mut bytes = vec![0; (rate * 1000.0).round() as usize + 100];
        bytes[..2].copy_from_slice(&[0xff, 0xa0]);
        let end = bytes.len();
        bytes[end - 2..].copy_from_slice(&[0xff, 0xa1]);
        fs::write(PathBuf::from(&args[3]).with_extension("wsq"), bytes).unwrap();
        return;
    }
    let input = PathBuf::from(env::args_os().nth(2).unwrap());
    let ppi = if mode == "unknown_ppi" {
        "-1"
    } else if mode == "extreme_ppi" {
        "1e300"
    } else {
        "500"
    };
    let mut ncm = format!("NIST_COM 6\nPIX_WIDTH 2\nPIX_HEIGHT 3\nPIX_DEPTH 8\nPPI {ppi}\n");
    if mode == "duplicate" {
        ncm.push_str("PPI 500\n");
    }
    fs::write(input.with_extension("ncm"), ncm).unwrap();
    fs::write(
        input.with_extension("raw"),
        if mode == "short_pixels" {
            vec![1]
        } else {
            vec![1, 2, 3, 4, 5, 6]
        },
    )
    .unwrap();
}
