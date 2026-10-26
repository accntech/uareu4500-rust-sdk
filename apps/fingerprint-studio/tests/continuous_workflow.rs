// SPDX-License-Identifier: GPL-3.0-or-later
use fingerprint_studio::{
    Config, SampleAcceptance, Session,
    workflow::{Command, Event, Mode, Worker},
};
use std::{
    fs,
    path::PathBuf,
    process::Command as Process,
    sync::OnceLock,
    time::{Duration, Instant},
};
use uareu::{frame::Image, metadata::CapturedImage};

const HELPER: &str = r#"
use std::io::Read;
fn first(input:&str,key:&str)->u8 { input.split(&format!("\"{key}\":[")).nth(1).unwrap().split([',',']']).next().unwrap().parse().unwrap() }
fn main() {
 let mut input=String::new();std::io::stdin().read_to_string(&mut input).unwrap();
 let result=if input.contains("\"operation\":\"extract\"") {
   let marker=first(&input,"pixels"); if marker==99 {eprintln!("helper failure");std::process::exit(4);}
   format!("{{\"data\":[{marker}]}}")
 } else {let score=if first(&input,"probe")==first(&input,"candidate"){80}else{5};format!("{{\"score\":{score}}}")};
 println!("{{\"schema_version\":1,\"engine\":\"SourceAFIS\",\"version\":\"3.14.0\",\"engine_sha256\":\"fixture\",\"host_sha256\":\"fixture\",\"result\":{result}}}");
}
"#;
fn config() -> Config {
    static HELPER_PATH: OnceLock<PathBuf> = OnceLock::new();
    let path = HELPER_PATH.get_or_init(|| {
        let root =
            std::env::temp_dir().join(format!("fingerprint-studio-helper-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("fixture.rs");
        fs::write(&source, HELPER).unwrap();
        let executable = root.join(format!("fixture{}", std::env::consts::EXE_SUFFIX));
        assert!(
            Process::new("rustc")
                .args(["--edition=2024"])
                .arg(&source)
                .arg("-o")
                .arg(&executable)
                .status()
                .unwrap()
                .success()
        );
        executable
    });
    Config::parse_args([
        "--engine".to_owned(),
        path.display().to_string(),
        "--experimental-ppi".into(),
        "500".into(),
    ])
    .unwrap()
}
fn captured(marker: u8) -> CapturedImage {
    CapturedImage::new(Image {
        width: 16,
        height: 16,
        pixels: vec![marker; 256],
    })
}
#[test]
fn enrollment_commits_three_independent_samples_then_verifies_a_new_capture() {
    let mut session = Session::new(config());
    for expected in 1..=3 {
        session.set_capture(captured(50)).unwrap();
        assert_eq!(
            session.try_add_sample().unwrap(),
            SampleAcceptance::Accepted(expected)
        );
        assert!(
            session.try_add_sample().is_err(),
            "one image cannot count twice"
        );
        assert_eq!(session.enrolled(), expected == 3);
    }
    assert!(
        session.verify().is_err(),
        "enrollment image is not a fresh verification probe"
    );
    session.set_capture(captured(50)).unwrap();
    assert!(session.verify().unwrap().matched);
    session.reset();
    assert!(!session.enrolled());
    assert_eq!(session.sample_count(), 0);
}
#[test]
fn inconsistency_is_retryable_but_helper_failure_is_fatal_without_losing_samples() {
    let mut session = Session::new(config());
    session.set_capture(captured(50)).unwrap();
    session.try_add_sample().unwrap();
    session.set_capture(captured(60)).unwrap();
    assert!(matches!(
        session.try_add_sample().unwrap(),
        SampleAcceptance::Rejected {
            score: 5.0,
            threshold: 40.0
        }
    ));
    assert_eq!(session.sample_count(), 1);
    assert!(
        session.try_add_sample().is_err(),
        "a rejected image cannot be reused"
    );
    session.set_capture(captured(99)).unwrap();
    assert!(session.try_add_sample().is_err());
    assert_eq!(session.sample_count(), 1);
    session.set_capture(captured(50)).unwrap();
    assert_eq!(
        session.try_add_sample().unwrap(),
        SampleAcceptance::Accepted(2)
    );
}
fn completion(worker: &Worker) -> (String, bool, usize, bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match worker.try_recv() {
            Ok(Event::Complete {
                message,
                failed,
                samples,
                enrolled,
                ..
            }) => return (message, failed, samples, enrolled),
            Ok(_) => {}
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("worker disconnected: {error}"),
        }
    }
}
#[test]
fn mode_preconditions_do_not_open_a_reader_and_worker_remains_usable() {
    let worker = Worker::start(Config::default()).unwrap();
    worker.send(Command::Start(Mode::Verify)).unwrap();
    let (message, failed, _, _) = completion(&worker);
    assert!(failed);
    assert!(message.contains("Enroll"));
    worker.send(Command::Start(Mode::Enroll)).unwrap();
    assert!(completion(&worker).1);
    worker.send(Command::Reset).unwrap();
    let (_, failed, samples, enrolled) = completion(&worker);
    assert!(!failed);
    assert_eq!(samples, 0);
    assert!(!enrolled);
}
