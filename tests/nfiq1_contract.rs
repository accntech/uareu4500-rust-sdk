// SPDX-License-Identifier: GPL-3.0-or-later
use std::{path::PathBuf, time::Duration};
use uareu::{
    frame::Image,
    metadata::{CapturedImage, Resolution},
    quality::{LegacyNfiqOutcome, Nfiq1},
};
fn image() -> CapturedImage {
    let mut i = CapturedImage::new(Image {
        width: 400,
        height: 400,
        pixels: vec![128; 160000],
    });
    i.metadata.resolution = Resolution::declared(500.0, 500.0).unwrap();
    i
}
#[test]
fn legacy_quality_rejects_unknown_scale_and_keeps_nfiq2_semantics_separate() {
    let engine = Nfiq1 {
        executable: PathBuf::from("missing-nfiq1"),
        timeout: Duration::from_secs(1),
    };
    let mut i = image();
    assert!(matches!(
        engine.assess(&i),
        LegacyNfiqOutcome::EngineUnavailable { .. }
    ));
    let mut tiny = CapturedImage::new(Image {
        width: 200,
        height: 200,
        pixels: vec![128; 40000],
    });
    tiny.metadata.resolution = Resolution::declared(500.0, 500.0).unwrap();
    assert!(matches!(
        engine.assess(&tiny),
        LegacyNfiqOutcome::UnsupportedInput { .. }
    ));
    i.metadata.resolution = Resolution::Unknown;
    assert!(matches!(
        engine.assess(&i),
        LegacyNfiqOutcome::UnsupportedInput { .. }
    ));
    i.metadata.resolution = Resolution::declared(1000.0, 1000.0).unwrap();
    assert!(matches!(
        engine.assess(&i),
        LegacyNfiqOutcome::UnsupportedInput { .. }
    ));
}

fn fixture(mode: &str) -> (PathBuf, Nfiq1) {
    use std::{
        fs,
        process::Command,
        sync::{
            OnceLock,
            atomic::{AtomicU64, Ordering},
        },
    };
    static BASE: OnceLock<PathBuf> = OnceLock::new();
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let base=BASE.get_or_init(|| {
  let base=std::env::temp_dir().join(format!("uareu-nfiq1-contract-{}",std::process::id()));
  fs::create_dir_all(&base).unwrap();
  let source=base.join("stub.rs");fs::write(&source,r#"
use std::{env,fs,path::Path,thread,time::Duration};
fn main(){
 let a:Vec<_>=env::args().collect();let mode=Path::new(&a[0]).file_stem().unwrap().to_str().unwrap();
 if mode=="shared" {thread::sleep(Duration::from_millis(160));}
 if a.get(1).map(String::as_str)==Some("-version") {
  if mode=="version" {println!("Release 4.0");return;}
  if mode=="oversized" {print!("{}","x".repeat(131073));return;}
  println!("Standard Version: ANSI/NIST-ITL 1-2007\nNBIS Non-Export Control Software Version: Release 5.0.0");return;
 }
 assert_eq!(a.len(),5);assert_eq!(a[1],"-o");assert_eq!(a[2],"-raw");
 let g:Vec<usize>=a[3].split(',').map(|n|n.parse().unwrap()).collect();
 assert_eq!(&g[2..],&[8,500]);let p=fs::read(&a[4]).unwrap();assert_eq!(p.len(),g[0]*g[1]);
 if mode=="crop" {assert_eq!(g[0],256);assert_eq!(g[1],300);assert!(p.iter().all(|p|*p==128));}
 match mode {
 "failure"=>{eprintln!("deliberate child failure");std::process::exit(7);},
 "malformed"=>println!("1\n2"), "zero"=>println!("0"), "six"=>println!("6"),
 "hang"=>thread::sleep(Duration::from_secs(20)), _=>println!("3"),
 }
}
"#).unwrap();
  let executable=base.join(format!("stub{}",std::env::consts::EXE_SUFFIX));
  assert!(Command::new(std::env::var_os("RUSTC").unwrap_or_else(||"rustc".into())).arg(&source).arg("-o").arg(&executable).status().unwrap().success());executable
 });
    let dir = base.parent().unwrap().join(format!(
        "case-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    let executable = dir.join(format!("{mode}{}", std::env::consts::EXE_SUFFIX));
    fs::copy(base, &executable).unwrap();
    (
        dir,
        Nfiq1 {
            executable,
            timeout: Duration::from_secs(2),
        },
    )
}
#[test]
fn legacy_process_validates_contract_scores_and_preserves_region() {
    let (dir, engine) = fixture("crop");
    let mut input = image();
    input.metadata.valid_region = Some(uareu::metadata::Region {
        x: 7,
        y: 8,
        width: 256,
        height: 300,
    });
    let original = input.clone();
    let LegacyNfiqOutcome::Scored {
        score,
        input_region,
        input_resolution,
        evaluation_ppi,
        engine: identity,
        ..
    } = engine.assess(&input)
    else {
        panic!("expected legacy score")
    };
    assert_eq!(score, 3);
    assert_eq!(evaluation_ppi, 500);
    assert_eq!(identity.name, "NFIQ 1");
    assert_eq!(input_region, input.metadata.valid_region.unwrap());
    assert_eq!(input_resolution, input.metadata.resolution);
    assert_eq!(input, original);
    let json = serde_json::to_value(engine.assess(&input)).unwrap();
    assert_eq!(json["score"], 3);
    assert_eq!(json["score_direction"], "lower_is_better");
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn legacy_process_rejects_bad_versions_scores_failure_and_excess_output() {
    for mode in [
        "version",
        "malformed",
        "zero",
        "six",
        "failure",
        "oversized",
    ] {
        let (dir, engine) = fixture(mode);
        assert!(
            matches!(
                engine.assess(&image()),
                LegacyNfiqOutcome::EvaluationFailed { .. }
            ),
            "{mode}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn legacy_process_has_one_deadline_for_version_and_score() {
    for mode in ["hang", "shared"] {
        let (dir, mut engine) = fixture(mode);
        engine.timeout = Duration::from_millis(250);
        let start = std::time::Instant::now();
        let result = engine.assess(&image());
        assert!(
            matches!(result,LegacyNfiqOutcome::EvaluationFailed{ref reason} if reason.contains("timed out")),
            "{result:?}"
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        std::fs::remove_dir_all(dir).unwrap();
    }
    let (dir, mut engine) = fixture("crop");
    engine.timeout = Duration::ZERO;
    assert!(matches!(
        engine.assess(&image()),
        LegacyNfiqOutcome::EvaluationFailed { .. }
    ));
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn legacy_record_view_keeps_ppcm_declaration_and_rejects_opaque_input() {
    use uareu::image_record::{ImageCompression, ImageView, MultiImageRecord};
    let (dir, engine) = fixture("valid");
    let mut record = MultiImageRecord {
        capture_device_id: 0,
        acquisition_level: 0,
        scale_units: 2,
        scan_resolution_x: 197,
        scan_resolution_y: 197,
        image_resolution_x: 197,
        image_resolution_y: 197,
        compression: ImageCompression::Raw8Bit,
        images: vec![ImageView {
            finger_position: 1,
            view_count: 1,
            view_number: 1,
            quality: 0,
            impression: 0,
            width: 400,
            height: 400,
            data: vec![128; 160000],
        }],
    };
    let LegacyNfiqOutcome::Scored {
        input_resolution,
        evaluation_ppi,
        ..
    } = engine.assess_record_view(&record, 0)
    else {
        panic!("expected legacy score")
    };
    assert_eq!(evaluation_ppi, 500);
    assert_eq!(
        input_resolution,
        record.raw_image(0).unwrap().metadata.resolution
    );
    assert!(matches!(
        engine.assess_record_view(&record, 1),
        LegacyNfiqOutcome::UnsupportedInput { .. }
    ));
    record.compression = ImageCompression::OpaqueWsq;
    assert!(matches!(
        engine.assess_record_view(&record, 0),
        LegacyNfiqOutcome::UnsupportedInput { .. }
    ));
    std::fs::remove_dir_all(dir).unwrap();
}
