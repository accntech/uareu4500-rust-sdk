// SPDX-License-Identifier: GPL-3.0-or-later
// Protocol reference: libfprint; Copyright (C) 2007-2008 Daniel Drake,
// Copyright (C) 2012 Timo Teräs. See NOTICE for license origin and scope.
// Rust implementation and modifications: 2026-10-25, UareU 4500 Rust SDK contributors.
use crate::frame::Image;
use std::{sync::atomic::AtomicBool, time::Duration};
pub trait Transport {
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
    Capturing,
}
struct Session<'a, T: Transport> {
    io: &'a mut T,
    cancel: &'a AtomicBool,
    deadline: std::time::Instant,
    active: bool,
    ready: bool,
    finger: bool,
}
impl<T: Transport> Session<'_, T> {
    fn check(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.cancel.load(std::sync::atomic::Ordering::Relaxed),
            "capture cancelled"
        );
        anyhow::ensure!(
            std::time::Instant::now() < self.deadline,
            "capture deadline exceeded"
        );
        Ok(())
    }
    fn allowance(&self) -> anyhow::Result<Duration> {
        self.check()?;
        Ok(self
            .deadline
            .saturating_duration_since(std::time::Instant::now())
            .min(Duration::from_millis(50)))
    }
    fn read(&mut self, register: u16, length: u16) -> anyhow::Result<Vec<u8>> {
        let timeout = self.allowance()?;
        let bytes = self.io.read_registers(register, length, timeout)?;
        self.check()?;
        anyhow::ensure!(
            bytes.len() == usize::from(length),
            "register {register:02x} returned an unexpected length"
        );
        Ok(bytes)
    }
    fn write(&mut self, register: u16, bytes: &[u8]) -> anyhow::Result<()> {
        let timeout = self.allowance()?;
        self.io.write_registers(register, bytes, timeout)?;
        self.check()
    }
    fn poll(&mut self, wait: Duration) -> anyhow::Result<()> {
        let timeout = self.allowance()?.min(wait);
        let event = self.io.interrupt(timeout)?;
        self.check()?;
        match event {
            Some(0x56aa) => self.ready = true,
            Some(0x0101) => self.finger = true,
            Some(0x0200) => self.finger = false,
            Some(0x0800) => anyhow::bail!("reader reported scan failure"),
            _ => {}
        }
        Ok(())
    }
    fn discard_queued_events(&mut self) -> anyhow::Result<()> {
        // A phase boundary needs an observed empty queue, not merely a reset
        // of software latches. Refuse an endless stream of stale interrupts.
        for _ in 0..64 {
            self.check()?;
            let event = self.io.interrupt(Duration::ZERO)?;
            self.check()?;
            if event.is_none() {
                return Ok(());
            }
        }
        anyhow::bail!("interrupt queue did not empty at phase boundary")
    }
    fn pause(&self, duration: Duration) -> anyhow::Result<()> {
        self.check()?;
        std::thread::sleep(
            duration.min(
                self.deadline
                    .saturating_duration_since(std::time::Instant::now()),
            ),
        );
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
            anyhow::ensure!(recovered, "reader recovery did not complete");
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
        anyhow::bail!("reader did not become power ready")
    }
    fn run(
        &mut self,
        options: CaptureOptions,
        callback: &mut impl FnMut(CaptureEvent),
    ) -> anyhow::Result<Image> {
        self.notify(CaptureEvent::Initializing, callback)?;
        self.active = true;
        self.power_up()?;
        self.read(0xf0, 16)?;
        self.notify(CaptureEvent::WaitingForFinger, callback)?;
        self.discard_queued_events()?;
        self.finger = false;
        self.write(0x4e, &[0x10])?;
        while !self.finger {
            self.poll(Duration::from_millis(20))?;
            if !self.finger {
                self.pause(Duration::from_millis(1))?;
            }
        }
        self.notify(CaptureEvent::Capturing, callback)?;
        for _ in 0..3 {
            self.check()?;
            self.io.start_image()?;
            self.check()?;
            self.write(0x4e, &[0x20])?;
            let bytes = loop {
                let timeout = self.allowance()?;
                let image = self.io.image(timeout)?;
                self.check()?;
                if let Some(bytes) = image {
                    break bytes;
                }
                self.poll(Duration::ZERO)?;
                self.pause(Duration::from_millis(1))?;
            };
            let decoded = crate::frame::decode_frame(&bytes, options.rotate180, |index| {
                self.check()?;
                let mut entropy = [0u8; 4];
                getrandom::fill(&mut entropy)?;
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
        anyhow::bail!("reader returned three invalid frames")
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
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
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
    mut event: impl FnMut(CaptureEvent),
) -> anyhow::Result<Image> {
    let now = std::time::Instant::now();
    let Some(deadline) = now.checked_add(options.timeout) else {
        transport.cancel();
        anyhow::bail!("capture timeout is out of range");
    };
    let mut session = Session {
        io: transport,
        cancel,
        deadline,
        active: false,
        ready: false,
        finger: false,
    };
    let outcome = session.run(options, &mut event);
    let cleanup = session.cleanup();
    match outcome {
        Err(error) => Err(error),
        Ok(image) => {
            cleanup?;
            Ok(image)
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
    }
    impl Transport for Fake {
        fn read_registers(&mut self, r: u16, n: u16, _: Duration) -> anyhow::Result<Vec<u8>> {
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
            if r == 0x4e && b == [0x10] {
                self.events.push_back(0x0101);
            }
            if self.fail_stop && r == 0x4e && b == [0x70] {
                anyhow::bail!("stop failed")
            }
            Ok(())
        }
        fn interrupt(&mut self, _: Duration) -> anyhow::Result<Option<u16>> {
            Ok(self.events.pop_front())
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
