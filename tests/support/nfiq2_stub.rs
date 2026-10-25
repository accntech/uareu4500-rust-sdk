// SPDX-License-Identifier: GPL-3.0-or-later
// Process-contract fixture, not a biometric algorithm or conformance oracle.
use std::{env, fs, io::Write, time::Duration};
fn main() {
    let executable = env::current_exe().unwrap();
    let mode = fs::read_to_string(executable.parent().unwrap().join("mode.txt")).unwrap();
    let args: Vec<_> = env::args().skip(1).collect();
    if args.is_empty() {
        println!("NFIQ 2: {} (Date: fixture, Commit: fixture)", if mode == "version" { "9.0.0" } else { "2.3.0" });
        if mode == "embedded" { println!("Embedded model present"); }
        return;
    }
    assert!(args.contains(&"-v".into()) && args.contains(&"-a".into()) && args.contains(&"-F".into()));
    assert_eq!(args.contains(&"-m".into()), mode != "embedded");
    let input = fs::read(args.last().unwrap()).unwrap();
    fs::write(executable.parent().unwrap().join("observed_path.txt"), args.last().unwrap()).unwrap();
    assert_eq!(&input[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(u32::from_be_bytes(input[16..20].try_into().unwrap()), if mode == "crop" { 200 } else { 384 });
    assert_eq!(u32::from_be_bytes(input[20..24].try_into().unwrap()), if mode == "crop" { 210 } else { 290 });
    if mode == "nonzero" { std::process::exit(7); }
    if mode == "hang" { std::thread::sleep(Duration::from_secs(60)); }
    if mode == "flood" { std::io::stdout().write_all(&vec![b'x'; 256 * 1024]).unwrap(); return; }
    println!("Filename,QualityScore,OptionalError,Quantized,Resampled,UniformImage,EmptyImageOrContrastTooLow,FingerprintImageWithMinutiae,SufficientFingerprintForeground");
    println!("\"input.png\",{},NA,0,{},35.6,163.2,55,98722", if mode == "badscore" { "255" } else { "90" }, if mode == "resampled" { 1 } else { 0 });
}
