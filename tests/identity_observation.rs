// SPDX-License-Identifier: GPL-3.0-or-later
use anyhow::{Result, bail};
use std::{collections::VecDeque, sync::atomic::AtomicBool, time::Duration};
use uareu::protocol::{CaptureEvent, CaptureOptions, Transport, capture};

struct ObservedSensor {
    info: Vec<u8>,
    status: u8,
    events: VecDeque<u16>,
    observations: Vec<[u8; 16]>,
    reads: Vec<(u16, u16)>,
    writes: Vec<(u16, Vec<u8>)>,
    fail_info: bool,
}
impl ObservedSensor {
    fn new(info: Vec<u8>) -> Self {
        Self {
            info,
            status: 0x80,
            events: VecDeque::new(),
            observations: vec![],
            reads: vec![],
            writes: vec![],
            fail_info: false,
        }
    }
    fn run(&mut self) -> Result<()> {
        capture(
            self,
            CaptureOptions {
                timeout: Duration::from_secs(1),
                rotate180: false,
            },
            &AtomicBool::new(false),
            |_| {},
        )
        .map(|_| ())
    }
}
impl Transport for ObservedSensor {
    fn read_registers(&mut self, register: u16, length: u16, _: Duration) -> Result<Vec<u8>> {
        self.reads.push((register, length));
        match register {
            7 => Ok(vec![self.status]),
            0xf0 if !self.fail_info => Ok(self.info.clone()),
            _ => bail!("simulated read failure"),
        }
    }
    fn write_registers(&mut self, register: u16, bytes: &[u8], _: Duration) -> Result<()> {
        self.writes.push((register, bytes.to_vec()));
        match (register, bytes) {
            (7, [value]) => {
                self.status = *value;
                if value & 0x80 == 0 {
                    self.events.push_back(0x56aa);
                }
            }
            (0x4e, [0x10]) => self.events.push_back(0x0101),
            _ => {}
        }
        Ok(())
    }
    fn interrupt(&mut self, _: Duration) -> Result<Option<u16>> {
        Ok(self.events.pop_front())
    }
    fn start_image(&mut self) -> Result<()> {
        Ok(())
    }
    fn image(&mut self, _: Duration) -> Result<Option<Vec<u8>>> {
        Ok(Some(
            include_bytes!("../fixtures/synthetic-plain.wire").to_vec(),
        ))
    }
    fn cancel(&mut self) {}
    fn identity_observed(&mut self, raw: [u8; 16]) {
        self.observations.push(raw);
    }
}
#[test]
fn initialization_retains_existing_information_read_without_extra_usb_io_or_phases() {
    let raw: Vec<u8> = (0..16).collect();
    let mut sensor = ObservedSensor::new(raw.clone());
    let mut phases = vec![];
    capture(
        &mut sensor,
        CaptureOptions {
            timeout: Duration::from_secs(1),
            rotate180: false,
        },
        &AtomicBool::new(false),
        |p| phases.push(p),
    )
    .unwrap();
    assert_eq!(sensor.observations, [<[u8; 16]>::try_from(raw).unwrap()]);
    assert_eq!(sensor.reads, [(7, 1), (7, 1), (0xf0, 16)]);
    assert_eq!(
        sensor.writes,
        [
            (7, vec![0]),
            (0x4e, vec![0x10]),
            (0x4e, vec![0x20]),
            (0x4e, vec![0x70]),
            (7, vec![0x80])
        ]
    );
    assert_eq!(
        phases,
        [
            CaptureEvent::Initializing,
            CaptureEvent::WaitingForFinger,
            CaptureEvent::Capturing
        ]
    );
}
#[test]
fn short_long_or_failed_information_reads_publish_no_identity() {
    for size in [0, 15, 17] {
        let mut sensor = ObservedSensor::new(vec![0; size]);
        assert!(sensor.run().is_err());
        assert!(sensor.observations.is_empty());
        assert_eq!(sensor.reads.iter().filter(|(r, _)| *r == 0xf0).count(), 1);
    }
    let mut sensor = ObservedSensor::new(vec![0; 16]);
    sensor.fail_info = true;
    assert!(sensor.run().is_err());
    assert!(sensor.observations.is_empty());
}
