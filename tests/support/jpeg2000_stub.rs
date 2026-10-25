// SPDX-License-Identifier: GPL-3.0-or-later
use std::{fs, path::PathBuf};
fn main() {
    let executable = std::env::current_exe().unwrap();
    let mode = fs::read_to_string(executable.parent().unwrap().join("mode")).unwrap();
    if mode == "failure" {
        std::process::exit(2);
    }
    if mode == "hang" {
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
    let args: Vec<_> = std::env::args_os().collect();
    let output = PathBuf::from(&args[args.iter().position(|a| a == "-o").unwrap() + 1]);
    let header = match mode.as_str() {
        "wrong_geometry" => "P5\n3 2\n255\n",
        "wrong_depth" => "P5\n2 3\n65535\n",
        _ => "P5\n# explicit decoder comment\n2 3\n255\n",
    };
    let mut b = header.as_bytes().to_vec();
    b.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
    if mode == "short" {
        b.pop();
    }
    if mode == "trailing" {
        b.push(7);
    }
    fs::write(output, b).unwrap();
}
