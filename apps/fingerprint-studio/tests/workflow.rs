use fingerprint_studio::{Config, Session};

#[test]
fn recognition_requires_explicit_scale_and_helper() {
    let config = Config::parse_args(["--engine", "/tmp/helper"]).unwrap();
    assert!(!config.recognition_enabled());
    let config = Config::parse_args(["--engine", "/tmp/helper", "--ppi", "500"]).unwrap();
    assert!(config.recognition_enabled());
}

#[test]
fn rejects_invalid_configuration() {
    for args in [
        vec!["--ppi", "NaN"],
        vec!["--ppi", "0"],
        vec!["--ppi", "500", "--threshold", "0"],
        vec!["--unknown"],
        vec!["--engine"],
    ] {
        assert!(Config::parse_args(args).is_err());
    }
}

#[test]
fn reset_discards_preview_and_enrollment() {
    let mut session = Session::new(Config::default());
    session.reset();
    assert!(session.capture.is_none());
    assert_eq!(session.sample_count(), 0);
    assert!(!session.enrolled());
}

#[test]
fn enrollment_and_verification_require_a_capture() {
    let mut session = Session::new(Config::default());
    assert!(session.add_sample().is_err());
    assert!(session.verify().is_err());
    assert_eq!(session.sample_count(), 0);
}

fn capture() -> uareu::metadata::CapturedImage {
    uareu::metadata::CapturedImage::new(uareu::frame::Image {
        width: 16,
        height: 16,
        pixels: vec![128; 256],
    })
}

#[test]
fn new_capture_attempt_discards_previous_image() {
    let mut session = Session::new(Config::default());
    session.set_capture(capture()).unwrap();
    session.begin_capture();
    assert!(session.capture.is_none());
    assert!(session.add_sample().is_err());
}

#[test]
fn ppi_is_only_assigned_when_explicitly_declared() {
    use uareu::metadata::{Resolution, ResolutionProvenance};
    let mut session = Session::new(Config::default());
    session.set_capture(capture()).unwrap();
    assert_eq!(
        session.capture.as_ref().unwrap().metadata.resolution,
        Resolution::Unknown
    );
    let mut session = Session::new(Config::parse_args(["--ppi", "500"]).unwrap());
    session.set_capture(capture()).unwrap();
    assert_eq!(
        session.capture.as_ref().unwrap().metadata.resolution,
        Resolution::Known {
            x_ppi: 500.0,
            y_ppi: 500.0,
            provenance: ResolutionProvenance::CallerDeclared
        }
    );
    session.reset();
    assert!(session.capture.is_none());
}

#[test]
fn invalid_capture_and_missing_helper_do_not_change_enrollment() {
    let mut session = Session::new(
        Config::parse_args(["--engine", "/missing/fingerprint-helper", "--ppi", "500"]).unwrap(),
    );
    let mut invalid = capture();
    invalid.image.pixels.pop();
    assert!(session.set_capture(invalid).is_err());
    assert!(session.capture.is_none());
    session.set_capture(capture()).unwrap();
    assert!(session.add_sample().is_err());
    assert_eq!(session.sample_count(), 0);
    assert!(!session.enrolled());
}

#[test]
fn nominal_scale_is_an_explicit_experimental_choice() {
    let config =
        Config::parse_args(["--engine", "/tmp/helper", "--experimental-ppi", "500"]).unwrap();
    assert!(config.recognition_enabled());
    assert_eq!(config.scale_label(), "500 PPI · nominal, unverified");
    let mut experimental = Session::new(config);
    experimental.set_capture(capture()).unwrap();
    assert!(matches!(
        experimental.capture.as_ref().unwrap().metadata.resolution,
        uareu::metadata::Resolution::Known {
            provenance: uareu::metadata::ResolutionProvenance::CallerDeclared,
            ..
        }
    ));
    let confirmed = Config::parse_args(["--ppi", "500"]).unwrap();
    assert_eq!(confirmed.scale_label(), "500 PPI · caller declared");
    assert!(Config::parse_args(["--ppi", "500", "--experimental-ppi", "512"]).is_err());
    assert!(Config::parse_args(["--experimental-ppi", "NaN"]).is_err());
}
