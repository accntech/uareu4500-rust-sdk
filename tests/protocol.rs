// SPDX-License-Identifier: GPL-3.0-or-later
use anyhow::{Result, ensure};
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use uareu4500::protocol::{CaptureEvent, CaptureOptions, Transport, capture};

#[derive(Debug, PartialEq)]
enum Operation {
    Read(u16, u16),
    Write(u16, Vec<u8>),
    QueueImage,
    Cancel,
}

struct SensorModel {
    status: u8,
    ready_announced: bool,
    key_request: Option<(u8, u32)>,
    events: VecDeque<u16>,
    images: VecDeque<Vec<u8>>,
    operations: Vec<Operation>,
    image_queued: bool,
    supply_finger: bool,
    fail_read: Option<u16>,
    fail_stop: bool,
    emit_ready: bool,
}
impl SensorModel {
    fn new(frame: &[u8]) -> Self {
        Self {
            status: 0x80,
            ready_announced: false,
            key_request: None,
            events: VecDeque::new(),
            images: [frame.to_vec()].into(),
            operations: Vec::new(),
            image_queued: false,
            supply_finger: true,
            fail_read: None,
            fail_stop: false,
            emit_ready: true,
        }
    }
    fn plain() -> Self {
        Self::new(include_bytes!("../fixtures/synthetic-plain.wire"))
    }
    fn stopped(&self) -> bool {
        self.operations.ends_with(&[
            Operation::Write(0x4e, vec![0x70]),
            Operation::Write(7, vec![0x80]),
            Operation::Cancel,
        ])
    }
}
impl Transport for SensorModel {
    fn read_registers(&mut self, register: u16, length: u16, timeout: Duration) -> Result<Vec<u8>> {
        assert!(!timeout.is_zero());
        self.operations.push(Operation::Read(register, length));
        ensure!(self.fail_read != Some(register), "simulated disconnect");
        match register {
            7 => Ok(vec![self.status]),
            0xf0 => Ok(vec![0; 16]),
            0x34 => {
                let (index, seed) = self.key_request.expect("key request must precede read");
                let key: u32 = match index {
                    254 => 0x1367_9245,
                    255 => 0x89ab_cdef,
                    0 => 0xf00d_cafe,
                    _ => panic!("unexpected key index"),
                };
                Ok((key ^ seed).to_le_bytes().to_vec())
            }
            _ => anyhow::bail!("unmodeled register {register:x}"),
        }
    }
    fn write_registers(&mut self, register: u16, data: &[u8], timeout: Duration) -> Result<()> {
        assert!(!timeout.is_zero());
        self.operations
            .push(Operation::Write(register, data.to_vec()));
        match (register, data) {
            (7, [value]) if value & 0x80 != 0 => self.status = *value,
            (7, [value]) => {
                self.status = if value & 4 != 0 { 1 } else { 0 };
                if !self.ready_announced && value & 4 == 0 {
                    if self.emit_ready {
                        self.events.push_back(0x56aa);
                    }
                    self.ready_announced = true;
                }
            }
            (0x4e, [0x10]) if self.supply_finger => self.events.push_back(0x0101),
            (0x4e, [0x20]) => assert!(self.image_queued, "bulk must be queued before capture mode"),
            (0x4e, [0x70]) if self.fail_stop => anyhow::bail!("stop request failed"),
            (0x33, request) => {
                assert_eq!(request.len(), 5);
                let seed = u32::from_le_bytes(request[1..].try_into().unwrap());
                assert_eq!(seed >> 31, 0);
                self.key_request = Some((request[0], seed));
            }
            _ => {}
        }
        Ok(())
    }
    fn interrupt(&mut self, timeout: Duration) -> Result<Option<u16>> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        std::thread::sleep(timeout.min(Duration::from_millis(1)));
        Ok(None)
    }
    fn start_image(&mut self) -> Result<()> {
        assert!(!self.image_queued);
        self.image_queued = true;
        self.operations.push(Operation::QueueImage);
        Ok(())
    }
    fn image(&mut self, _: Duration) -> Result<Option<Vec<u8>>> {
        assert!(self.image_queued);
        let image = self.images.pop_front();
        if image.is_some() {
            self.image_queued = false;
        }
        Ok(image)
    }
    fn cancel(&mut self) {
        self.image_queued = false;
        self.operations.push(Operation::Cancel);
    }
}
fn options() -> CaptureOptions {
    CaptureOptions {
        timeout: Duration::from_secs(2),
        rotate180: true,
    }
}

#[test]
fn session_captures_the_synthetic_plain_canvas_and_releases_resources() {
    let mut model = SensorModel::plain();
    let mut notifications = Vec::new();
    let image = capture(&mut model, options(), &AtomicBool::new(false), |event| {
        notifications.push(event)
    })
    .unwrap();
    assert_eq!(
        notifications,
        [
            CaptureEvent::Initializing,
            CaptureEvent::WaitingForFinger,
            CaptureEvent::Capturing
        ]
    );
    assert_eq!(
        image.pixels.as_slice(),
        include_bytes!("../fixtures/synthetic-plain.gray")
    );
    assert!(model.stopped());
}

#[test]
fn session_performs_fresh_seed_exchanges_for_each_synthetic_key_index() {
    let mut model = SensorModel::new(include_bytes!("../fixtures/synthetic-scrambled.wire"));
    let image = capture(&mut model, options(), &AtomicBool::new(false), |_| {}).unwrap();
    let expected = include_bytes!("../fixtures/synthetic-scrambled.gray");
    assert_eq!(image.pixels.len(), expected.len());
    assert!(image.pixels.iter().zip(expected).all(|(a, b)| a == b));
    let indices: Vec<_> = model
        .operations
        .iter()
        .filter_map(|operation| match operation {
            Operation::Write(0x33, bytes) => Some(bytes[0]),
            _ => None,
        })
        .collect();
    assert_eq!(indices, [254, 255, 0]);
    assert!(model.stopped());
}

#[test]
fn malformed_packets_are_retried_without_losing_the_capture_session() {
    let mut model = SensorModel::plain();
    model.images.push_front(vec![0; 63]);
    capture(&mut model, options(), &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(
        model
            .operations
            .iter()
            .filter(|op| **op == Operation::QueueImage)
            .count(),
        2
    );
    assert!(model.stopped());
}

#[test]
fn abnormal_power_status_recovers_and_reaches_capture() {
    let mut model = SensorModel::plain();
    model.status = 0x85;
    capture(&mut model, options(), &AtomicBool::new(false), |_| {}).unwrap();
    assert!(model.operations.contains(&Operation::Write(7, vec![5])));
    assert!(model.stopped());
}

#[test]
fn cancellation_at_notifications_prevents_further_session_io_but_cleans_up() {
    for stop_at in [
        CaptureEvent::Initializing,
        CaptureEvent::WaitingForFinger,
        CaptureEvent::Capturing,
    ] {
        let mut model = SensorModel::plain();
        let cancelled = AtomicBool::new(false);
        let result = capture(&mut model, options(), &cancelled, |event| {
            if event == stop_at {
                cancelled.store(true, Ordering::Relaxed);
            }
        });
        assert!(result.is_err());
        assert!(model.stopped() || model.operations == vec![Operation::Cancel]);
        if stop_at == CaptureEvent::Capturing {
            assert!(!model.operations.contains(&Operation::QueueImage));
        }
        if stop_at == CaptureEvent::WaitingForFinger {
            assert!(
                !model
                    .operations
                    .contains(&Operation::Write(0x4e, vec![0x10]))
            );
        }
    }
}

#[test]
fn preflight_failure_only_cancels_queued_transfers() {
    for (timeout, cancelled) in [
        (Duration::ZERO, false),
        (Duration::MAX, false),
        (Duration::from_secs(1), true),
    ] {
        let mut model = SensorModel::plain();
        assert!(
            capture(
                &mut model,
                CaptureOptions {
                    timeout,
                    ..options()
                },
                &AtomicBool::new(cancelled),
                |_| {}
            )
            .is_err()
        );
        assert_eq!(model.operations, [Operation::Cancel]);
    }
}

#[test]
fn finger_deadline_expiry_and_disconnect_stop_the_session() {
    for disconnected in [false, true] {
        let mut model = SensorModel::plain();
        model.supply_finger = false;
        if disconnected {
            model.fail_read = Some(0xf0);
        }
        let result = capture(
            &mut model,
            CaptureOptions {
                timeout: Duration::from_millis(20),
                ..options()
            },
            &AtomicBool::new(false),
            |_| {},
        );
        assert!(result.is_err());
        assert!(model.stopped());
    }
}

#[test]
fn key_io_failure_is_not_retried_as_a_bad_packet() {
    let mut model = SensorModel::new(include_bytes!("../fixtures/synthetic-scrambled.wire"));
    model.fail_read = Some(0x34);
    let error = capture(&mut model, options(), &AtomicBool::new(false), |_| {}).unwrap_err();
    assert!(format!("{error:#}").contains("disconnect"));
    assert_eq!(
        model
            .operations
            .iter()
            .filter(|op| **op == Operation::QueueImage)
            .count(),
        1
    );
    assert!(model.stopped());
}

#[test]
fn stop_failure_still_attempts_power_down_and_cancellation() {
    let mut model = SensorModel::plain();
    model.fail_stop = true;
    let result = capture(&mut model, options(), &AtomicBool::new(false), |_| {});
    assert!(result.is_err());
    assert!(model.stopped());
}

#[test]
fn old_power_ready_events_do_not_validate_a_new_activation() {
    let mut model = SensorModel::plain();
    model.emit_ready = false;
    model.events.push_back(0x56aa);
    let result = capture(
        &mut model,
        CaptureOptions {
            timeout: Duration::from_millis(80),
            ..options()
        },
        &AtomicBool::new(false),
        |_| {},
    );
    assert!(result.is_err());
    assert!(!model.operations.contains(&Operation::Read(0xf0, 16)));
    assert!(model.stopped());
}

#[test]
fn a_stale_finger_present_cannot_override_its_queued_absence() {
    let mut model = SensorModel::plain();
    model.supply_finger = false;
    model.events.extend([0x56aa, 0x0101, 0x0200]);
    let result = capture(
        &mut model,
        CaptureOptions {
            timeout: Duration::from_millis(80),
            ..options()
        },
        &AtomicBool::new(false),
        |_| {},
    );
    assert!(result.is_err());
    assert!(!model.operations.contains(&Operation::QueueImage));
    assert!(model.stopped());
}
