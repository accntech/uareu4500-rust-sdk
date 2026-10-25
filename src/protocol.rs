// SPDX-License-Identifier: GPL-3.0-or-later
// Protocol reference: libfprint; Copyright (C) 2007-2008 Daniel Drake,
// Copyright (C) 2012 Timo Teräs. See NOTICE for license origin and scope.
// Rust implementation and modifications: 2026-10-25, UareU Rust SDK contributors.
use crate::error::{
    CaptureError, CaptureErrorKind as Kind, CaptureStage as Stage, transport_error,
};
use crate::frame::Image;
use crate::metadata::CapturedImage;
use std::{sync::atomic::AtomicBool, time::Duration};
pub trait Transport {
    /// Retain an already validated initialization register-f0 read. This hook
    /// performs no extra register request and does not classify firmware/hardware.
    /// Existing transports may ignore the optional observation.
    fn identity_observed(&mut self, _raw: [u8; 16]) {}
    fn read_registers(
        &mut self,
        register: u16,
        length: u16,
        timeout: Duration,
    ) -> anyhow::Result<Vec<u8>>;
    fn write_registers(
        &mut self,
        register: u16,
        bytes: &[u8],
        timeout: Duration,
    ) -> anyhow::Result<()>;
    fn interrupt(&mut self, timeout: Duration) -> anyhow::Result<Option<u16>>;
    fn start_image(&mut self) -> anyhow::Result<()>;
    fn image(&mut self, timeout: Duration) -> anyhow::Result<Option<Vec<u8>>>;
    fn cancel(&mut self);
}
#[derive(Copy, Debug, Clone)]
pub struct CaptureOptions {
    /// Overall deadline for a single capture; initialization/acquisition deadline
    /// for continuous capture, excluding idle placement/removal waits.
    pub timeout: Duration,
    pub rotate180: bool,
}
impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            rotate180: true,
        }
    }
}
#[derive(Copy, Debug, Clone, PartialEq, Eq)]
pub enum CaptureEvent {
    Initializing,
    WaitingForFinger,
    WaitingForFingerRemoval,
    Capturing,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContinuousStop {
    Callback,
    Cancelled { stage: Stage },
}
struct Session<'a, T: Transport> {
    io: &'a mut T,
    cancel: &'a AtomicBool,
    deadline: Option<std::time::Instant>,
    active: bool,
    ready: bool,
    finger: bool,
    stage: Stage,
}
impl<T: Transport> Session<'_, T> {
    fn check(&self) -> anyhow::Result<()> {
        if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(CaptureError::new(Kind::Cancelled, self.stage, "capture cancelled").into());
        }
        if self
            .deadline
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
        {
            return Err(
                CaptureError::new(Kind::Timeout, self.stage, "capture deadline exceeded").into(),
            );
        }
        Ok(())
    }
    fn allowance(&self) -> anyhow::Result<Duration> {
        self.check()?;
        Ok(self.deadline.map_or(Duration::from_millis(50), |deadline| {
            deadline
                .saturating_duration_since(std::time::Instant::now())
                .min(Duration::from_millis(50))
        }))
    }
    fn read(&mut self, register: u16, length: u16) -> anyhow::Result<Vec<u8>> {
        let timeout = self.allowance()?;
        let bytes = self
            .io
            .read_registers(register, length, timeout)
            .map_err(|cause| transport_error(self.stage, cause))?;
        if bytes.len() != usize::from(length) {
            return Err(CaptureError::new(
                Kind::InvalidResponse,
                self.stage,
                format!("register {register:02x} returned an unexpected length"),
            )
            .into());
        }
        self.check()?;
        Ok(bytes)
    }
    fn write(&mut self, register: u16, bytes: &[u8]) -> anyhow::Result<()> {
        let timeout = self.allowance()?;
        self.io
            .write_registers(register, bytes, timeout)
            .map_err(|cause| transport_error(self.stage, cause))?;
        self.check()
    }
    fn poll(&mut self, wait: Duration) -> anyhow::Result<()> {
        let timeout = self.allowance()?.min(wait);
        let event = self
            .io
            .interrupt(timeout)
            .map_err(|cause| transport_error(self.stage, cause))?;
        if event == Some(0x0800) {
            return Err(CaptureError::new(
                Kind::DeviceFailure,
                self.stage,
                "reader reported scan failure",
            )
            .into());
        }
        self.check()?;
        match event {
            Some(0x56aa) => self.ready = true,
            Some(0x0101) => self.finger = true,
            Some(0x0200) => self.finger = false,
            _ => {}
        }
        Ok(())
    }
    fn discard_queued_events(&mut self) -> anyhow::Result<()> {
        // A phase boundary needs an observed empty queue, not merely a reset
        // of software latches. Refuse an endless stream of stale interrupts.
        for _ in 0..64 {
            self.check()?;
            let event = self
                .io
                .interrupt(Duration::ZERO)
                .map_err(|cause| transport_error(self.stage, cause))?;
            self.check()?;
            if event.is_none() {
                return Ok(());
            }
        }
        Err(CaptureError::new(
            Kind::InvalidResponse,
            self.stage,
            "interrupt queue did not empty at phase boundary",
        )
        .into())
    }
    fn pause(&self, duration: Duration) -> anyhow::Result<()> {
        self.check()?;
        std::thread::sleep(duration.min(self.deadline.map_or(duration, |deadline| {
            deadline.saturating_duration_since(std::time::Instant::now())
        })));
        self.check()
    }
    fn notify(
        &self,
        event: CaptureEvent,
        callback: &mut impl FnMut(CaptureEvent),
    ) -> anyhow::Result<()> {
        self.check()?;
        callback(event);
        self.check()
    }
    fn power_up(&mut self) -> anyhow::Result<()> {
        self.discard_queued_events()?;
        let mut status = self.read(7, 1)?[0];
        if status & 0x84 == 0x84 {
            self.write(7, &[status & 0x0f])?;
            let mut recovered = false;
            for _ in 0..20 {
                self.poll(Duration::ZERO)?;
                status = self.read(7, 1)?[0];
                if status & 1 != 0 {
                    recovered = true;
                    break;
                }
                self.pause(Duration::from_millis(5))?;
            }
            if !recovered {
                return Err(CaptureError::new(
                    Kind::DeviceFailure,
                    self.stage,
                    "reader recovery did not complete",
                )
                .into());
            }
        }
        for _ in 0..3 {
            if status & 0x80 == 0 {
                self.write(7, &[status | 0x80])?;
            }
            self.pause(Duration::from_millis(10))?;
            self.discard_queued_events()?;
            self.ready = false;
            let mut awake = false;
            for _ in 0..20 {
                self.write(7, &[status & 0x0f])?;
                self.poll(Duration::ZERO)?;
                status = self.read(7, 1)?[0];
                if status & 0x80 == 0 {
                    awake = true;
                    break;
                }
                self.pause(Duration::from_millis(5))?;
            }
            if awake {
                let ready_deadline = std::time::Instant::now() + Duration::from_millis(300);
                while !self.ready && std::time::Instant::now() < ready_deadline {
                    self.poll(Duration::from_millis(10))?;
                    if !self.ready {
                        self.pause(Duration::from_millis(1))?;
                    }
                }
                if self.ready {
                    return Ok(());
                }
            }
        }
        Err(CaptureError::new(
            Kind::DeviceFailure,
            self.stage,
            "reader did not become power ready",
        )
        .into())
    }
    fn initialize(&mut self, callback: &mut impl FnMut(CaptureEvent)) -> anyhow::Result<()> {
        self.stage = Stage::Initialization;
        self.notify(CaptureEvent::Initializing, callback)?;
        self.active = true;
        self.power_up()?;
        let bytes = self.read(0xf0, 16)?;
        let mut raw = [0; 16];
        raw.copy_from_slice(&bytes);
        self.io.identity_observed(raw);
        self.check()?;
        Ok(())
    }
    fn wait_for_finger(
        &mut self,
        present: bool,
        callback: &mut impl FnMut(CaptureEvent),
    ) -> anyhow::Result<()> {
        self.stage = Stage::FingerWait;
        self.notify(
            if present {
                CaptureEvent::WaitingForFinger
            } else {
                CaptureEvent::WaitingForFingerRemoval
            },
            callback,
        )?;
        self.discard_queued_events()?;
        self.finger = !present;
        self.write(0x4e, &[if present { 0x10 } else { 0x12 }])?;
        while self.finger != present {
            self.poll(Duration::from_millis(20))?;
            if self.finger != present {
                self.pause(Duration::from_millis(1))?;
            }
        }
        Ok(())
    }
    fn acquire_image(
        &mut self,
        options: CaptureOptions,
        callback: &mut impl FnMut(CaptureEvent),
    ) -> anyhow::Result<CapturedImage> {
        self.stage = Stage::Acquisition;
        self.notify(CaptureEvent::Capturing, callback)?;
        for _ in 0..3 {
            self.check()?;
            self.stage = Stage::Acquisition;
            self.io
                .start_image()
                .map_err(|cause| transport_error(self.stage, cause))?;
            self.check()?;
            self.write(0x4e, &[0x20])?;
            let bytes = loop {
                let timeout = self.allowance()?;
                let image = self
                    .io
                    .image(timeout)
                    .map_err(|cause| transport_error(self.stage, cause))?;
                self.check()?;
                if let Some(bytes) = image {
                    break bytes;
                }
                self.poll(Duration::ZERO)?;
                self.pause(Duration::from_millis(1))?;
            };
            self.stage = Stage::Decode;
            let decoded =
                crate::frame::decode_frame_with_metadata(&bytes, options.rotate180, |index| {
                    self.check()?;
                    let mut entropy = [0u8; 4];
                    getrandom::fill(&mut entropy)
                        .map_err(|cause| transport_error(self.stage, cause.into()))?;
                    let seed = u32::from_le_bytes(entropy) & 0x7fff_ffff;
                    let mut request = [0u8; 5];
                    request[0] = index;
                    request[1..].copy_from_slice(&seed.to_le_bytes());
                    self.write(0x33, &request)?;
                    let response = self.read(0x34, 4)?;
                    Ok(u32::from_le_bytes(
                        response
                            .try_into()
                            .map_err(|_| anyhow::anyhow!("invalid image key length"))?,
                    ) ^ seed)
                });
            self.check()?;
            match decoded {
                Ok(image) => return Ok(image),
                Err(error) if error.is::<crate::frame::InvalidFrame>() => continue,
                Err(error) => return Err(error),
            }
        }
        Err(CaptureError::new(
            Kind::InvalidFrame,
            Stage::Decode,
            "reader returned three invalid frames",
        )
        .into())
    }
    fn run(
        &mut self,
        options: CaptureOptions,
        callback: &mut impl FnMut(CaptureEvent),
    ) -> anyhow::Result<CapturedImage> {
        self.initialize(callback)?;
        self.wait_for_finger(true, callback)?;
        self.acquire_image(options, callback)
    }
    fn run_continuous(
        &mut self,
        options: CaptureOptions,
        callback: &mut impl FnMut(CaptureEvent),
        image: &mut impl FnMut(CapturedImage) -> anyhow::Result<std::ops::ControlFlow<()>>,
    ) -> anyhow::Result<()> {
        self.initialize(callback)?;
        loop {
            // Finger placement/removal may take arbitrarily long. USB operations
            // still use short bounded allowances and always observe cancellation.
            self.deadline = None;
            self.wait_for_finger(true, callback)?;
            self.deadline = Some(
                std::time::Instant::now()
                    .checked_add(options.timeout)
                    .ok_or_else(|| {
                        CaptureError::new(
                            Kind::InvalidArgument,
                            Stage::Acquisition,
                            "capture timeout is out of range",
                        )
                    })?,
            );
            let captured = self.acquire_image(options, callback)?;
            let action = image(captured)?;
            if action.is_break() {
                return Ok(());
            }
            self.deadline = None;
            self.check()?;
            self.wait_for_finger(false, callback)?;
        }
    }
    fn cleanup(&mut self) -> anyhow::Result<()> {
        let mut first_error = None;
        if self.active {
            // Each independent cleanup operation receives a bounded allowance.
            for (register, value) in [(0x4e, 0x70), (7, 0x80)] {
                if let Err(error) =
                    self.io
                        .write_registers(register, &[value], Duration::from_millis(100))
                {
                    let error = transport_error(Stage::Cleanup, error);
                    first_error = Some(match first_error.take() {
                        None => error,
                        Some(primary) => crate::error::attach_cleanup(primary, Err(error)),
                    });
                }
            }
        }
        self.io.cancel();
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

pub fn capture(
    transport: &mut impl Transport,
    options: CaptureOptions,
    cancel: &AtomicBool,
    event: impl FnMut(CaptureEvent),
) -> anyhow::Result<Image> {
    Ok(capture_with_metadata(transport, options, cancel, event)?.image)
}

/// Capture with descriptor coverage and processing provenance; resolution is unknown.
pub fn capture_with_metadata(
    transport: &mut impl Transport,
    options: CaptureOptions,
    cancel: &AtomicBool,
    mut event: impl FnMut(CaptureEvent),
) -> anyhow::Result<CapturedImage> {
    let now = std::time::Instant::now();
    let Some(deadline) = now.checked_add(options.timeout) else {
        transport.cancel();
        return Err(CaptureError::new(
            Kind::InvalidArgument,
            Stage::Initialization,
            "capture timeout is out of range",
        )
        .into());
    };
    let mut session = Session {
        io: transport,
        cancel,
        deadline: Some(deadline),
        active: false,
        ready: false,
        finger: false,
        stage: Stage::Initialization,
    };
    let outcome = session.run(options, &mut event);
    let cleanup = session.cleanup();
    match outcome {
        Err(error) => Err(crate::error::attach_cleanup(error, cleanup)),
        Ok(image) => {
            cleanup?;
            Ok(image)
        }
    }
}
/// Capture successive finger placements until cancelled or the callback breaks.
/// Inactivity while waiting for placement/removal is unlimited; initialization
/// and each image acquisition retain `options.timeout` deadlines.
pub fn capture_continuous(
    transport: &mut impl Transport,
    options: CaptureOptions,
    cancel: &AtomicBool,
    event: impl FnMut(CaptureEvent),
    mut image: impl FnMut(Image) -> anyhow::Result<std::ops::ControlFlow<()>>,
) -> anyhow::Result<()> {
    capture_continuous_with_metadata(transport, options, cancel, event, |captured| {
        image(captured.image)
    })
}

/// Continuous capture retaining metadata for each image. No quality work runs here.
pub fn capture_continuous_with_metadata(
    transport: &mut impl Transport,
    options: CaptureOptions,
    cancel: &AtomicBool,
    event: impl FnMut(CaptureEvent),
    image: impl FnMut(CapturedImage) -> anyhow::Result<std::ops::ControlFlow<()>>,
) -> anyhow::Result<()> {
    capture_continuous_detailed(transport, options, cancel, event, image).map(|_| ())
}
/// Like continuous capture, but preserves whether a callback or cancellation stopped it.
pub fn capture_continuous_detailed(
    transport: &mut impl Transport,
    options: CaptureOptions,
    cancel: &AtomicBool,
    mut event: impl FnMut(CaptureEvent),
    mut image: impl FnMut(CapturedImage) -> anyhow::Result<std::ops::ControlFlow<()>>,
) -> anyhow::Result<ContinuousStop> {
    let Some(deadline) = std::time::Instant::now().checked_add(options.timeout) else {
        transport.cancel();
        return Err(CaptureError::new(
            Kind::InvalidArgument,
            Stage::Initialization,
            "capture timeout is out of range",
        )
        .into());
    };
    let mut session = Session {
        io: transport,
        cancel,
        deadline: Some(deadline),
        active: false,
        ready: false,
        finger: false,
        stage: Stage::Initialization,
    };
    let outcome = session.run_continuous(options, &mut event, &mut image);
    let cleanup = session.cleanup();
    match outcome {
        Err(error) => match error.downcast_ref::<CaptureError>() {
            Some(error) if error.kind == Kind::Cancelled => {
                cleanup?;
                Ok(ContinuousStop::Cancelled { stage: error.stage })
            }
            _ => Err(crate::error::attach_cleanup(error, cleanup)),
        },
        Ok(()) => {
            cleanup?;
            Ok(ContinuousStop::Callback)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::atomic::Ordering};
    #[derive(Default)]
    struct Fake {
        writes: Vec<(u16, Vec<u8>)>,
        statuses: VecDeque<u8>,
        events: VecDeque<u16>,
        cancelled: usize,
        started: bool,
        fail_stop: bool,
        invalid_frames: bool,
        key_failure: bool,
        image_calls: usize,
        read_failure: Option<nusb::transfer::TransferError>,
        cancel_on_failure: Option<std::sync::Arc<AtomicBool>>,
        no_finger: bool,
        short_read: bool,
        scan_failure: bool,
    }
    impl Transport for Fake {
        fn read_registers(&mut self, r: u16, n: u16, _: Duration) -> anyhow::Result<Vec<u8>> {
            if self.short_read {
                if let Some(cancel) = &self.cancel_on_failure {
                    cancel.store(true, Ordering::Relaxed);
                }
                return Ok(vec![]);
            }
            if let Some(error) = self.read_failure.take() {
                if let Some(cancel) = &self.cancel_on_failure {
                    cancel.store(true, Ordering::Relaxed);
                }
                return Err(error.into());
            }
            if r == 0x34 && self.key_failure {
                return Err(std::io::Error::other("injected key failure").into());
            }
            Ok(if r == 7 {
                vec![self.statuses.pop_front().unwrap_or(0)]
            } else {
                vec![0; n as usize]
            })
        }
        fn write_registers(&mut self, r: u16, b: &[u8], _: Duration) -> anyhow::Result<()> {
            self.writes.push((r, b.to_vec()));
            if r == 7 && b.len() == 1 && b[0] & 0x80 == 0 {
                self.events.push_back(0x56aa);
            }
            if r == 0x4e && b == [0x10] && !self.no_finger {
                self.events
                    .push_back(if self.scan_failure { 0x0800 } else { 0x0101 });
            }
            if self.fail_stop && r == 0x4e && b == [0x70] {
                anyhow::bail!("stop failed")
            }
            Ok(())
        }
        fn interrupt(&mut self, _: Duration) -> anyhow::Result<Option<u16>> {
            let event = self.events.pop_front();
            if event == Some(0x0800) {
                if let Some(cancel) = &self.cancel_on_failure {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            Ok(event)
        }
        fn start_image(&mut self) -> anyhow::Result<()> {
            self.started = true;
            Ok(())
        }
        fn image(&mut self, _: Duration) -> anyhow::Result<Option<Vec<u8>>> {
            self.image_calls += 1;
            if self.invalid_frames {
                return Ok(Some(vec![]));
            }
            if self.key_failure {
                let mut b = vec![0; 64 + 768];
                b[4] = 2;
                b[16] = 2;
                b[17] = 2;
                for (i, v) in b[64..].iter_mut().enumerate() {
                    *v = if i % 2 == 0 { 0 } else { 255 };
                }
                return Ok(Some(b));
            }
            let mut b = vec![0; 64 + 384];
            b[4] = 1;
            b[17] = 1;
            Ok(Some(b))
        }
        fn cancel(&mut self) {
            self.cancelled += 1;
        }
    }
    #[test]
    fn observed_protocol_failures_are_not_relabelled_by_concurrent_cancellation() {
        for short in [false, true] {
            let cancel = std::sync::Arc::new(AtomicBool::new(false));
            let mut fake = Fake {
                short_read: short,
                scan_failure: !short,
                cancel_on_failure: Some(cancel.clone()),
                ..Default::default()
            };
            let cause = capture(&mut fake, Default::default(), &cancel, |_| {}).unwrap_err();
            assert!(cancel.load(Ordering::Relaxed));
            assert_eq!(
                cause.downcast_ref::<CaptureError>().unwrap().kind,
                if short {
                    Kind::InvalidResponse
                } else {
                    Kind::DeviceFailure
                }
            );
        }
    }
    #[test]
    fn structured_causes_survive_cancellation_races_and_cleanup_failures() {
        for (failure, expected) in [
            (
                nusb::transfer::TransferError::Disconnected,
                Kind::Disconnected,
            ),
            (nusb::transfer::TransferError::Cancelled, Kind::Transport),
            (nusb::transfer::TransferError::Stall, Kind::Transport),
        ] {
            let cancel = std::sync::Arc::new(AtomicBool::new(false));
            let mut fake = Fake {
                read_failure: Some(failure),
                cancel_on_failure: Some(cancel.clone()),
                fail_stop: true,
                ..Default::default()
            };
            let cause = capture(&mut fake, Default::default(), &cancel, |_| {}).unwrap_err();
            assert!(cancel.load(Ordering::Relaxed));
            let typed = cause.downcast_ref::<CaptureError>().unwrap();
            assert_eq!(typed.kind, expected);
            assert_eq!(typed.stage, Stage::Initialization);
            assert!(cause.is::<nusb::transfer::TransferError>());
            let report = crate::error::CaptureFailure::from_error(&cause);
            assert_eq!(report.kind, expected);
            assert_eq!(report.cleanup.as_ref().unwrap().stage, Some(Stage::Cleanup));
            assert_eq!(fake.cancelled, 1);
        }
    }
    #[test]
    fn structured_timeout_cancel_invalid_frame_and_cleanup_are_distinct() {
        let mut fake = Fake::default();
        let cancelled = capture(
            &mut fake,
            Default::default(),
            &AtomicBool::new(true),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(
            cancelled.downcast_ref::<CaptureError>().unwrap().kind,
            Kind::Cancelled
        );
        let mut fake = Fake {
            no_finger: true,
            ..Default::default()
        };
        let mut reached_wait = false;
        let timed_out = capture(
            &mut fake,
            CaptureOptions {
                timeout: Duration::from_millis(100),
                ..Default::default()
            },
            &AtomicBool::new(false),
            |phase| reached_wait |= phase == CaptureEvent::WaitingForFinger,
        )
        .unwrap_err();
        let typed = timed_out.downcast_ref::<CaptureError>().unwrap();
        assert_eq!(typed.kind, Kind::Timeout);
        assert_eq!(
            typed.stage,
            if reached_wait {
                Stage::FingerWait
            } else {
                Stage::Initialization
            }
        );
        let mut fake = Fake {
            invalid_frames: true,
            ..Default::default()
        };
        let invalid = capture(
            &mut fake,
            Default::default(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(
            invalid.downcast_ref::<CaptureError>().unwrap().kind,
            Kind::InvalidFrame
        );
        let mut fake = Fake {
            fail_stop: true,
            ..Default::default()
        };
        let cleanup = capture(
            &mut fake,
            Default::default(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(
            cleanup.downcast_ref::<CaptureError>().unwrap().stage,
            Stage::Cleanup
        );
    }
    #[test]
    fn preflight_always_cancels_without_writes() {
        for expired in [false, true] {
            let mut t = Fake::default();
            let c = AtomicBool::new(!expired);
            let o = CaptureOptions {
                timeout: if expired {
                    Duration::ZERO
                } else {
                    Duration::from_secs(1)
                },
                ..Default::default()
            };
            assert!(capture(&mut t, o, &c, |_| {}).is_err());
            assert!(t.writes.is_empty());
            assert_eq!(t.cancelled, 1);
        }
    }
    #[test]
    fn cancellation_from_notification_cleans_session() {
        let mut t = Fake::default();
        let c = AtomicBool::new(false);
        assert!(
            capture(&mut t, Default::default(), &c, |_| c
                .store(true, Ordering::Relaxed))
            .is_err()
        );
        assert!(!t.started);
        assert_eq!(t.cancelled, 1);
    }
    #[test]
    fn early_ready_event_and_single_capture() {
        let mut t = Fake {
            statuses: VecDeque::from([0x80, 0]),
            ..Default::default()
        };
        let c = AtomicBool::new(false);
        let im = capture(&mut t, Default::default(), &c, |_| {}).unwrap();
        assert_eq!(im.pixels.len(), 384 * 290);
        assert!(t.started);
        assert!(t.writes.ends_with(&[(0x4e, vec![0x70]), (7, vec![0x80])]));
        assert_eq!(t.cancelled, 1);
    }
    #[test]
    fn invalid_frames_have_finite_budget() {
        let mut t = Fake {
            statuses: VecDeque::from([0x80, 0]),
            invalid_frames: true,
            ..Default::default()
        };
        assert!(capture(&mut t, Default::default(), &AtomicBool::new(false), |_| {}).is_err());
        assert_eq!(t.image_calls, 3);
        assert_eq!(t.cancelled, 1);
    }
    #[test]
    fn key_io_error_is_not_retried() {
        let mut t = Fake {
            statuses: VecDeque::from([0x80, 0]),
            key_failure: true,
            ..Default::default()
        };
        let error =
            capture(&mut t, Default::default(), &AtomicBool::new(false), |_| {}).unwrap_err();
        assert!(error.is::<std::io::Error>());
        assert_eq!(t.image_calls, 1);
        assert_eq!(t.cancelled, 1);
    }
    #[test]
    fn capture_notification_cancellation_prevents_image_submission() {
        let mut t = Fake {
            statuses: VecDeque::from([0x80, 0]),
            ..Default::default()
        };
        let c = AtomicBool::new(false);
        assert!(
            capture(&mut t, Default::default(), &c, |e| {
                if e == CaptureEvent::Capturing {
                    c.store(true, Ordering::Relaxed);
                }
            })
            .is_err()
        );
        assert!(!t.started);
        assert!(!t.writes.contains(&(0x4e, vec![0x20])));
        assert_eq!(t.cancelled, 1);
    }
    #[test]
    fn sleeping_status_repeats_activation_write() {
        let mut t = Fake {
            statuses: VecDeque::from([0x80, 0x80, 0]),
            ..Default::default()
        };
        capture(&mut t, Default::default(), &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(
            t.writes
                .iter()
                .filter(|(r, b)| *r == 7 && b == &[0])
                .count(),
            2
        );
    }
    #[test]
    fn cleanup_attempts_power_after_stop_failure() {
        let mut t = Fake {
            statuses: VecDeque::from([0x80, 0]),
            fail_stop: true,
            ..Default::default()
        };
        assert!(capture(&mut t, Default::default(), &AtomicBool::new(false), |_| {}).is_err());
        assert_eq!(t.writes.last(), Some(&(7, vec![0x80])));
        assert_eq!(t.cancelled, 1);
    }
}
