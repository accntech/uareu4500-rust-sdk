// SPDX-License-Identifier: GPL-3.0-or-later
//! Owned reader worker for explicit capture, continuous enrollment, and verify modes.
use crate::{Config, SampleAcceptance, Session};
use anyhow::{Context, Result, ensure};
use std::{
    cell::{Cell, RefCell},
    ops::ControlFlow,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread::JoinHandle,
};
use uareu::{
    error::{CaptureErrorKind, CaptureFailure},
    metadata::CapturedImage,
    protocol::{CaptureEvent, CaptureOptions, ContinuousStop},
    recognition::MatchResult,
    usb::{self, Reader},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Capture,
    Enroll,
    Verify,
}
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Capture => "Capture",
            Self::Enroll => "Enroll",
            Self::Verify => "Verify",
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Command {
    Refresh,
    Start(Mode),
    Reset,
}
#[derive(Debug)]
pub enum Event {
    Status(String),
    Preview {
        capture: CapturedImage,
        details: String,
    },
    Verification {
        result: MatchResult,
        attempt: usize,
    },
    Progress {
        samples: usize,
        enrolled: bool,
    },
    Complete {
        message: String,
        failed: bool,
        devices: usize,
        samples: usize,
        enrolled: bool,
    },
}
pub struct Worker {
    commands: Option<Sender<Command>>,
    events: Receiver<Event>,
    cancel: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn start(config: Config) -> Result<Self> {
        let (commands, requests) = mpsc::channel();
        let (output, events) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let busy = Arc::new(AtomicBool::new(false));
        let working = busy.clone();
        let worker = std::thread::Builder::new().name("fingerprint-studio-reader".into()).spawn(move || {
            let recognition = config.recognition_enabled();
            let mut session = Session::new(config);
            let mut devices = Vec::new();
            let mut removal_pending = false;
            while let Ok(command) = requests.recv() {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<String> {
                    match command {
                        Command::Refresh => {
                            devices = usb::readers()?;
                            Ok(if devices.is_empty() { "Connect a U.are.U reader, then refresh.".into() } else {
                                format!("{} reader(s) connected. Using the first reader: {}.", devices.len(), devices[0].product_string().unwrap_or("U.are.U"))
                            })
                        }
                        Command::Reset => { session.reset(); Ok("Enrollment and preview cleared.".into()) }
                        Command::Start(mode) => {
                            if mode == Mode::Verify { ensure!(session.enrolled(), "Enroll three impressions before verification."); }
                            if mode != Mode::Capture { ensure!(recognition, "Configure the SourceAFIS helper and an explicit pixel scale to use this mode."); }
                            if mode == Mode::Enroll { ensure!(!session.enrolled(), "Enrollment is complete. Switch to Verify, or reset to enroll another finger."); }
                            let info = devices.first().context("No reader connected. Connect a reader and refresh.")?;
                            let mut reader = Reader::open(info)?;
                            match mode {
                                Mode::Enroll => enroll(&mut reader, &mut session, &signal, &output, &mut removal_pending),
                                Mode::Capture => {
                                    session.begin_capture();
                                    removal_pending = true;
                                    let capture = reader.capture_next(CaptureOptions::default(), &signal, |event| phase(event, &output))?;
                                    session.set_capture(capture)?;
                                    preview(&session, &output)?;
                                    Ok("Fingerprint captured.".into())
                                }
                                Mode::Verify => {
                                    let attempts = verify_continuously(
                                        |phase, image| reader.capture_continuous_detailed(CaptureOptions::default(), &signal, phase, image),
                                        &signal, &output, &mut removal_pending,
                                        |capture| {
                                            session.begin_capture();
                                            session.set_capture(capture)?;
                                            preview(&session, &output)?;
                                            if signal.load(Ordering::Acquire) { return Ok(None); }
                                            let _ = output.send(Event::Status("Comparing with the enrolled finger…".into()));
                                            session.verify().map(Some)
                                        },
                                    )?;
                                    Ok(format!("Verification stopped. {attempts} impressions compared. Enrollment retained."))
                                }
                            }
                        }
                    }
                }));
                let (message, failed) = match outcome {
                    Ok(Ok(message)) => (message, false),
                    Ok(Err(error)) if { let failure = CaptureFailure::from_error(&error); failure.kind == CaptureErrorKind::Cancelled && failure.cleanup.is_none() } => ("Operation stopped. Accepted enrollment samples are retained.".into(), false),
                    Ok(Err(error)) => (format!("{error:#}"), true),
                    Err(_) => ("The reader operation stopped unexpectedly. Refresh and try again.".into(), true),
                };
                working.store(false, Ordering::Release);
                if output.send(Event::Complete { message, failed, devices: devices.len(), samples: session.sample_count(), enrolled: session.enrolled() }).is_err() { break; }
            }
        }).context("Cannot start fingerprint worker")?;
        Ok(Self {
            commands: Some(commands),
            events,
            cancel,
            busy,
            worker: Some(worker),
        })
    }
    /// Exactly one request may be in flight; cancellation is never reset by the
    /// worker, so cancelling immediately after Send cannot race its startup.
    pub fn send(&self, command: Command) -> Result<()> {
        ensure!(
            self.busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            "A reader operation is already running."
        );
        self.cancel.store(false, Ordering::Release);
        if self
            .commands
            .as_ref()
            .context("Fingerprint worker is closed")?
            .send(command)
            .is_err()
        {
            self.busy.store(false, Ordering::Release);
            anyhow::bail!("Fingerprint worker is unavailable. Restart the app.");
        }
        Ok(())
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn try_recv(&self) -> std::result::Result<Event, TryRecvError> {
        self.events.try_recv()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        self.commands.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn phase(event: CaptureEvent, output: &Sender<Event>) {
    let _ = output.send(Event::Status(
        match event {
            CaptureEvent::Initializing => "Preparing the reader…",
            CaptureEvent::WaitingForFinger => "Place your finger on the reader.",
            CaptureEvent::WaitingForFingerRemoval => "Lift your finger before the next impression.",
            CaptureEvent::Capturing => "Capturing fingerprint…",
        }
        .into(),
    ));
}
fn preview(session: &Session, output: &Sender<Event>) -> Result<()> {
    if let Some(capture) = &session.capture {
        let diagnostics = uareu::quality::diagnostics(capture)?;
        let details = format!(
            "{} × {} px · contrast {:.1}",
            capture.image.width, capture.image.height, diagnostics.standard_deviation
        );
        let _ = output.send(Event::Preview {
            capture: capture.clone(),
            details,
        });
    }
    Ok(())
}
type PhaseCallback<'a> = &'a mut dyn FnMut(CaptureEvent);
type ImageCallback<'a> = &'a mut dyn FnMut(CapturedImage) -> Result<ControlFlow<()>>;
/// Keep one continuous reader session open. The SDK enforces lift/place cycles;
/// the gate also prevents a held placement surviving a pause from becoming a probe.
fn verify_continuously(
    source: impl FnOnce(PhaseCallback<'_>, ImageCallback<'_>) -> Result<ContinuousStop>,
    cancel: &AtomicBool,
    output: &Sender<Event>,
    removal_pending: &mut bool,
    mut compare: impl FnMut(CapturedImage) -> Result<Option<MatchResult>>,
) -> Result<usize> {
    let placement = RefCell::new(PlacementGate::new(*removal_pending));
    let mut attempts = 0;
    let result = source(
        &mut |event| {
            if event == CaptureEvent::WaitingForFinger {
                placement.borrow_mut().waiting_for_placement();
            } else if event == CaptureEvent::Capturing {
                placement.borrow_mut().pending = true;
            }
            phase(event, output);
        },
        &mut |capture| {
            placement.borrow_mut().pending = true;
            if cancel.load(Ordering::Acquire) {
                return Ok(ControlFlow::Break(()));
            }
            if !placement.borrow_mut().accept_acquisition() {
                let _ = output.send(Event::Status(
                    "Lift your finger, then place it again for a fresh verification impression."
                        .into(),
                ));
                return Ok(ControlFlow::Continue(()));
            }
            let Some(result) = compare(capture)? else {
                return Ok(ControlFlow::Break(()));
            };
            attempts += 1;
            let _ = output.send(Event::Verification {
                result,
                attempt: attempts,
            });
            Ok(if cancel.load(Ordering::Acquire) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            })
        },
    );
    *removal_pending = placement.borrow().removal_pending();
    result?;
    Ok(attempts)
}

struct PlacementGate {
    pending: bool,
    skip_initial: bool,
    started: bool,
}
impl PlacementGate {
    fn new(pending: bool) -> Self {
        Self {
            pending,
            skip_initial: pending,
            started: false,
        }
    }
    fn waiting_for_placement(&mut self) {
        if self.started {
            self.pending = false;
        }
        self.started = true;
    }
    fn accept_acquisition(&mut self) -> bool {
        self.pending = true;
        if self.skip_initial {
            self.skip_initial = false;
            false
        } else {
            true
        }
    }
    fn removal_pending(&self) -> bool {
        self.pending
    }
}
fn enroll(
    reader: &mut Reader,
    session: &mut Session,
    cancel: &AtomicBool,
    output: &Sender<Event>,
    removal_pending: &mut bool,
) -> Result<String> {
    // After the third accepted sample, wait for its removal too. Stop before
    // another acquisition, so verification starts after a completed placement.
    let placement = RefCell::new(PlacementGate::new(*removal_pending));
    let finished = Cell::new(false);
    let rejected = Cell::new(false);
    let result = reader.capture_continuous_detailed(CaptureOptions::default(), cancel, |event| {
        if event == CaptureEvent::WaitingForFinger { placement.borrow_mut().waiting_for_placement(); }
        else if event == CaptureEvent::Capturing { placement.borrow_mut().pending = true; }
        if event == CaptureEvent::WaitingForFinger && finished.get() {
            cancel.store(true, Ordering::Release);
        } else if event == CaptureEvent::WaitingForFingerRemoval && finished.get() {
            let _ = output.send(Event::Status("Lift your finger to finish enrollment.".into()));
        } else if event == CaptureEvent::WaitingForFingerRemoval && rejected.get() {
            let _ = output.send(Event::Status("Impression not accepted. Lift and reposition the same finger to retry.".into()));
        } else { phase(event, output); }
    }, |capture| {
        placement.borrow_mut().pending = true;
        if cancel.load(Ordering::Acquire) { return Ok(ControlFlow::Break(())); }
        if !placement.borrow_mut().accept_acquisition() {
            let _ = output.send(Event::Status("Lift your finger to resume with a separate impression.".into()));
            return Ok(ControlFlow::Continue(()));
        }
        session.begin_capture();
        session.set_capture(capture)?;
        preview(session, output)?;
        let _ = output.send(Event::Status("Checking this impression…".into()));
        match session.try_add_sample()? {
            SampleAcceptance::Accepted(samples) => {
                let _ = output.send(Event::Progress { samples, enrolled: session.enrolled() });
                finished.set(session.enrolled());
                rejected.set(false);
                let _ = output.send(Event::Status(if session.enrolled() {
                    "Three impressions accepted. Lift your finger to finish enrollment.".into()
                } else { format!("Impression {samples} of 3 accepted. Lift and reposition the same finger.") }));
            }
            SampleAcceptance::Rejected { .. } => {
                rejected.set(true);
                let _ = output.send(Event::Status("This impression did not match. Lift and reposition the enrolled finger to retry.".into()));
            }
        }
        Ok(ControlFlow::Continue(()))
    });
    *removal_pending = placement.borrow().removal_pending();
    result?;
    Ok(if session.enrolled() {
        "Enrollment complete. Switch to Verify to test a new impression.".into()
    } else {
        format!(
            "Enrollment paused. {} of 3 impressions retained.",
            session.sample_count()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{PlacementGate, verify_continuously};
    use std::sync::{atomic::AtomicBool, mpsc};
    use uareu::{
        frame::Image,
        metadata::CapturedImage,
        protocol::{CaptureEvent, ContinuousStop},
    };

    #[test]
    fn paused_held_finger_is_discarded_until_a_removal_cycle() {
        let mut gate = PlacementGate::new(true);
        gate.waiting_for_placement();
        assert!(!gate.accept_acquisition());
        assert!(gate.removal_pending());
        gate.waiting_for_placement();
        assert!(!gate.removal_pending());
        assert!(gate.accept_acquisition());
    }
    #[test]
    fn first_enrollment_accepts_first_placement_and_records_removal_need() {
        let mut gate = PlacementGate::new(false);
        gate.waiting_for_placement();
        assert!(gate.accept_acquisition());
        assert!(gate.removal_pending());
        gate.waiting_for_placement();
        assert!(!gate.removal_pending());
    }
    #[test]
    fn verification_discards_unresolved_enrollment_placement() {
        let cancel = AtomicBool::new(false);
        let mut pending = true;
        let (output, _events) = mpsc::channel();
        let capture = |marker| {
            CapturedImage::new(Image {
                width: 16,
                height: 16,
                pixels: vec![marker; 256],
            })
        };
        let compared = verify_continuously(
            |phase, image| {
                phase(CaptureEvent::WaitingForFinger);
                assert!(
                    image(capture(1))?.is_continue(),
                    "the held enrollment finger cannot become a probe"
                );
                phase(CaptureEvent::WaitingForFingerRemoval);
                phase(CaptureEvent::WaitingForFinger);
                assert!(image(capture(2))?.is_break());
                Ok(ContinuousStop::Callback)
            },
            &cancel,
            &output,
            &mut pending,
            |capture| {
                assert_eq!(capture.image.pixels[0], 2);
                cancel.store(true, std::sync::atomic::Ordering::Release);
                Ok(Some(uareu::recognition::MatchResult {
                    matched: true,
                    score: 80.0,
                    threshold: 40.0,
                }))
            },
        )
        .unwrap();
        assert_eq!(compared, 1);
        assert!(
            pending,
            "the verification placement must be lifted before another recognition mode"
        );
    }
    #[test]
    fn verification_keeps_matching_new_placements_until_stopped() {
        use std::sync::atomic::Ordering;
        use uareu::recognition::MatchResult;
        let cancel = AtomicBool::new(false);
        let mut pending = false;
        let (output, events) = mpsc::channel();
        let scans = verify_continuously(
            |phase, image| {
                for marker in [10, 20, 30] {
                    phase(CaptureEvent::WaitingForFinger);
                    let flow = image(CapturedImage::new(Image {
                        width: 16,
                        height: 16,
                        pixels: vec![marker; 256],
                    }))?;
                    if marker == 30 {
                        assert!(flow.is_break());
                    } else {
                        assert!(
                            flow.is_continue(),
                            "both match and no-match must keep scanning"
                        );
                        phase(CaptureEvent::WaitingForFingerRemoval);
                    }
                }
                Ok(ContinuousStop::Callback)
            },
            &cancel,
            &output,
            &mut pending,
            |capture| {
                let marker = capture.image.pixels[0];
                if marker == 30 {
                    cancel.store(true, Ordering::Release);
                }
                Ok(Some(MatchResult {
                    matched: marker != 20,
                    score: if marker == 20 { 5.0 } else { 80.0 },
                    threshold: 40.0,
                }))
            },
        )
        .unwrap();
        assert_eq!(scans, 3);
        assert!(
            pending,
            "stopping with a held finger must retain removal state"
        );
        let matches: Vec<_> = events
            .try_iter()
            .filter_map(|event| match event {
                super::Event::Verification { result, attempt } => Some((attempt, result.matched)),
                _ => None,
            })
            .collect();
        assert_eq!(matches, [(1, true), (2, false), (3, true)]);
    }

    #[test]
    fn stopped_verification_never_starts_a_comparison() {
        let cancel = AtomicBool::new(true);
        let mut pending = true;
        let (output, _events) = mpsc::channel();
        let scans = verify_continuously(
            |_phase, image| {
                assert!(
                    image(CapturedImage::new(Image {
                        width: 16,
                        height: 16,
                        pixels: vec![1; 256]
                    }))?
                    .is_break()
                );
                Ok(ContinuousStop::Callback)
            },
            &cancel,
            &output,
            &mut pending,
            |_| panic!("cancelled scan must not invoke a comparison"),
        )
        .unwrap();
        assert_eq!(scans, 0);
    }
    #[test]
    fn cancelling_during_acquisition_preserves_held_placement() {
        let cancel = AtomicBool::new(false);
        let mut pending = false;
        let (output, _events) = mpsc::channel();
        verify_continuously(
            |phase, _image| {
                phase(CaptureEvent::WaitingForFinger);
                phase(CaptureEvent::Capturing);
                cancel.store(true, std::sync::atomic::Ordering::Release);
                Ok(ContinuousStop::Cancelled {
                    stage: uareu::error::CaptureStage::Acquisition,
                })
            },
            &cancel,
            &output,
            &mut pending,
            |_| panic!("no completed capture to compare"),
        )
        .unwrap();
        assert!(
            pending,
            "a finger held during interrupted acquisition must be lifted on restart"
        );
        let mut restarted = PlacementGate::new(pending);
        restarted.waiting_for_placement();
        assert!(!restarted.accept_acquisition());
    }
}
