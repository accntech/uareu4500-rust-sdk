// SPDX-License-Identifier: GPL-3.0-or-later
// Process-protocol fixture; scores are candidate marker bytes, not biometrics.
use std::{
    env, fs,
    io::{Read, Write},
    time::Duration,
};

fn main() {
    let root = env::current_exe().unwrap().parent().unwrap().to_owned();
    let mode = fs::read_to_string(root.join("mode.txt")).unwrap();
    let mut launches = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(root.join("launches.txt"))
        .unwrap();
    writeln!(launches, "launch").unwrap();
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let first = input
        .split("\"candidate\":[")
        .nth(1)
        .unwrap()
        .split([',', ']'])
        .next()
        .unwrap()
        .parse::<u8>()
        .unwrap();
    let mut additional = Vec::new();
    if let Some(rest) = input.split("\"additional_candidates\":[").nth(1) {
        let mut depth = 0;
        for (i, c) in rest.char_indices() {
            if c == '[' {
                if depth == 0 {
                    additional.push(
                        rest[i + 1..]
                            .split([',', ']'])
                            .next()
                            .unwrap()
                            .parse::<u8>()
                            .unwrap(),
                    );
                }
                depth += 1;
            } else if c == ']' {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
        }
    }
    if mode == "failure" {
        eprintln!("deliberate failure");
        std::process::exit(7);
    }
    if mode == "hang" {
        std::thread::sleep(Duration::from_secs(60));
    }
    if mode == "oldslow" {
        std::thread::sleep(Duration::from_millis(150));
    }
    if mode == "flood" {
        std::io::stdout()
            .write_all(&vec![b'x'; 17 * 1024 * 1024])
            .unwrap();
        return;
    }
    let extras = if mode == "old" || mode == "oldslow" {
        String::new()
    } else if mode == "null" {
        ",\"additional_scores\":null".into()
    } else if mode == "short" {
        ",\"additional_scores\":[]".into()
    } else if mode == "negative" {
        ",\"additional_scores\":[-1]".into()
    } else {
        format!(",\"additional_scores\":{additional:?}")
    };
    let build = if mode == "build" { "wrong" } else { "fixture" };
    println!(
        "{{\"schema_version\":1,\"engine\":\"SourceAFIS\",\"version\":\"3.14.0\",\"engine_sha256\":\"{build}\",\"host_sha256\":\"fixture\",\"result\":{{\"score\":{first}{extras}}}}}"
    );
}
