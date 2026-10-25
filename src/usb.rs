// SPDX-License-Identifier: GPL-3.0-or-later
use crate::metadata::CapturedImage;
use crate::{
    error::{CaptureError, CaptureErrorKind, CaptureStage, transport_error},
    reader::{ReaderCapabilities, ReaderMonitor, ReaderStatus},
};
use crate::{
    frame::{Image, MAX_FRAME_SIZE},
    protocol::{self, CaptureEvent, CaptureOptions, Transport},
};
use anyhow::{Context, Result, bail, ensure};
use nusb::{
    DeviceInfo, Endpoint, Interface, MaybeFuture,
    descriptors::{ConfigurationDescriptor, TransferType},
    transfer::{Buffer, Bulk, ControlIn, ControlOut, ControlType, In, Interrupt, Recipient},
};
use std::{sync::atomic::AtomicBool, time::Duration};

/// 4000B and 4500 share this USB ID; the hardware revision is reported by listing.
/// Other family members need different profiles and are deliberately excluded.
pub fn supported_id(vendor: u16, product: u16) -> bool {
    (vendor, product) == (0x05ba, 0x000a)
}

/// Return a matching interface/alternate setting only when both expected endpoints exist.
pub fn capture_interface(config: ConfigurationDescriptor<'_>) -> Result<(u8, u8)> {
    for interface in config.interface_alt_settings() {
        if (
            interface.class(),
            interface.subclass(),
            interface.protocol(),
        ) != (0xff, 0xff, 0xff)
        {
            continue;
        }
        let interrupt = interface.endpoints().any(|e| {
            e.address() == 0x81
                && e.transfer_type() == TransferType::Interrupt
                && e.max_packet_size() == 64
        });
        let image = interface.endpoints().any(|e| {
            e.address() == 0x82
                && e.transfer_type() == TransferType::Bulk
                && e.max_packet_size() == 64
        });
        if interrupt && image {
            return Ok((interface.interface_number(), interface.alternate_setting()));
        }
    }
    bail!("no U.are.U capture interface with interrupt 0x81 and bulk 0x82 (64-byte packets)")
}

/// Enumerate supported readers without claiming interfaces or starting a scan.
pub fn readers() -> Result<Vec<DeviceInfo>> {
    let mut readers: Vec<_> = nusb::list_devices()
        .wait()
        .context("cannot enumerate USB devices")?
        .filter(|d| supported_id(d.vendor_id(), d.product_id()))
        .collect();
    readers.sort_by_key(|d| (d.bus_id().to_owned(), d.device_address()));
    Ok(readers)
}

/// Owned OS hotplug observation; only the exact opened device instance matches.
/// It never treats a newly attached device at the same address as the old reader.
struct ReaderHotplug {
    stop: std::sync::Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    binding: std::sync::Arc<std::sync::Mutex<ReaderMonitor>>,
}
impl ReaderHotplug {
    fn rebind(&self, monitor: &ReaderMonitor) {
        let mut current = self.binding.lock().unwrap_or_else(|p| p.into_inner());
        let previous = current.snapshot();
        current.hotplug_monitoring(false);
        *current = monitor.clone();
        monitor.hotplug_monitoring(previous.hotplug_monitoring);
        if previous.connected == Some(false) {
            monitor.hotplug_disconnected();
        }
    }
    fn start(id: nusb::DeviceId, monitor: ReaderMonitor) -> Result<Self> {
        use futures_core::Stream;
        use std::{
            pin::Pin,
            sync::{Arc, atomic::Ordering},
            task::{Context, Poll, Wake, Waker},
        };
        struct WakeThread(std::thread::Thread);
        impl Wake for WakeThread {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.0.unpark();
            }
        }
        let mut watch = nusb::watch_devices()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let binding = Arc::new(std::sync::Mutex::new(monitor));
        let observing = binding.clone();
        let worker = std::thread::Builder::new()
            .name("uareu-hotplug".into())
            .spawn(move || {
                struct ObservationGuard(std::sync::Arc<std::sync::Mutex<ReaderMonitor>>);
                impl Drop for ObservationGuard {
                    fn drop(&mut self) {
                        self.0
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .hotplug_monitoring(false);
                    }
                }
                let _guard = ObservationGuard(observing.clone());
                let waker = Waker::from(Arc::new(WakeThread(std::thread::current())));
                let mut context = Context::from_waker(&waker);
                observing
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .hotplug_monitoring(true);
                while !stopping.load(Ordering::Acquire) {
                    match Pin::new(&mut watch).poll_next(&mut context) {
                        Poll::Ready(Some(nusb::hotplug::HotplugEvent::Disconnected(gone)))
                            if gone == id =>
                        {
                            observing
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .hotplug_disconnected();
                            break;
                        }
                        Poll::Ready(Some(_)) => {}
                        Poll::Ready(None) => break,
                        Poll::Pending => std::thread::park_timeout(Duration::from_millis(100)),
                    }
                }
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
            binding,
        })
    }
}
impl Drop for ReaderHotplug {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

/// An exclusively opened reader. Drop releases its USB interface.
pub struct Reader {
    transport: UsbTransport,
    monitor: ReaderMonitor,
    _hotplug: Option<ReaderHotplug>,
}
impl Reader {
    pub fn open(info: &DeviceInfo) -> Result<Self> {
        Self::open_inner(info).map_err(|cause| transport_error(CaptureStage::Open, cause))
    }
    fn open_inner(info: &DeviceInfo) -> Result<Self> {
        if !supported_id(info.vendor_id(), info.product_id()) {
            return Err(CaptureError::new(
                CaptureErrorKind::Unsupported,
                CaptureStage::Open,
                format!(
                    "unsupported USB ID {:04x}:{:04x}",
                    info.vendor_id(),
                    info.product_id()
                ),
            )
            .into());
        }
        let monitor = ReaderMonitor::default();
        let hotplug = ReaderHotplug::start(info.id(), monitor.clone()).ok();
        let device = info.open().wait().context("cannot open reader; on Linux install the supplied udev rule; on macOS check USB accessory approval")?;
        let (number, alternate) = match device.active_configuration() {
            Ok(config) => capture_interface(config)?,
            Err(_) => {
                // macOS may leave vendor-class devices unconfigured. Select a
                // descriptor-verified configuration before claiming the interface.
                let (value, settings) = device
                    .configurations()
                    .find_map(|c| {
                        let value = c.configuration_value();
                        capture_interface(c).ok().map(|settings| (value, settings))
                    })
                    .context("reader has no supported USB configuration")?;
                device
                    .set_configuration(value)
                    .wait()
                    .context("cannot configure reader")?;
                settings
            }
        };
        let interface = device
            .claim_interface(number)
            .wait()
            .context("cannot claim reader interface; stop fprintd or other software using it")?;
        if alternate != 0 {
            interface
                .set_alt_setting(alternate)
                .wait()
                .context("cannot select capture alternate setting")?;
        }
        let mut interrupt = interface.endpoint::<Interrupt, In>(0x81)?;
        let image = interface.endpoint::<Bulk, In>(0x82)?;
        // Queue before power-up so that an early 0x56aa IRQ cannot be lost.
        interrupt.submit(Buffer::new(64));
        monitor.claimed();
        Ok(Self {
            _hotplug: hotplug,
            monitor: monitor.clone(),
            transport: UsbTransport {
                identity_monitor: monitor,
                interface,
                interrupt,
                image,
                cancelled: false,
            },
        })
    }

    pub fn capabilities(&self) -> ReaderCapabilities {
        ReaderCapabilities::for_usb_id(0x05ba, 0x000a)
            .expect("Reader only opens the supported profile")
    }
    pub fn status(&self) -> ReaderStatus {
        self.monitor.snapshot()
    }
    pub fn status_monitor(&self) -> ReaderMonitor {
        self.monitor.clone()
    }
    /// Explicit bounded read of documented status register 07, not a capability
    /// or readiness/calibration query. Returned connectivity is historical.
    pub fn probe_status(&mut self) -> Result<crate::reader::HardwareObservation> {
        self.monitor.begin_probe();
        let result = (|| {
            let bytes = self
                .transport
                .read_registers(7, 1, Duration::from_millis(100))
                .map_err(|e| transport_error(CaptureStage::Open, e))?;
            if bytes.len() != 1 {
                return Err(CaptureError::new(
                    CaptureErrorKind::InvalidResponse,
                    CaptureStage::Open,
                    "short hardware status register",
                )
                .into());
            }
            Ok(crate::reader::HardwareObservation {
                source: "usb_register_07",
                observed_unix_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|d| u64::try_from(d.as_millis()).ok()),
                connected_at_observation: true,
                hardware_status_register: bytes[0],
                low_power: bytes[0] & 0x80 != 0,
                hardware_ready: None,
                calibration_required: None,
            })
        })();
        self.monitor.finish_probe(
            result
                .as_ref()
                .cloned()
                .map_err(crate::error::CaptureFailure::from_error),
        );
        result
    }
    /// Known stop/low-power sequence and queue rearm; NOT vendor calibration or
    /// a USB bus reset. Both writes are attempted even if the first fails.
    pub fn recover_interface(&mut self) -> Result<()> {
        self.monitor.begin_probe();
        let first = self
            .transport
            .write_registers(0x4e, &[0x70], Duration::from_millis(100));
        let second = self
            .transport
            .write_registers(7, &[0x80], Duration::from_millis(100));
        self.transport.cancel();
        let rearm = self.transport.rearm();
        let mut result = Ok(());
        for next in [first, second, rearm] {
            let next = next.map_err(|e| transport_error(CaptureStage::Cleanup, e));
            result = match result {
                Ok(()) => next,
                Err(cause) => Err(crate::error::attach_cleanup(cause, next)),
            };
        }
        let probe = self.probe_status();
        let result = match result {
            Ok(()) => probe.map(|_| ()),
            Err(cause) => Err(crate::error::attach_cleanup(cause, probe.map(|_| ()))),
        };
        if let Err(cause) = &result {
            self.monitor
                .finish_probe(Err(crate::error::CaptureFailure::from_error(cause)));
        }
        result
    }
    pub(crate) fn with_monitor(mut self, monitor: ReaderMonitor) -> Self {
        self.monitor.released();
        if let Some(watch) = &self._hotplug {
            watch.rebind(&monitor);
        }
        rebind_identity_monitor(&mut self.transport.identity_monitor, &monitor);
        self.monitor = monitor;
        self.monitor.claimed();
        self
    }

    /// Wait for a finger, capture one image, and stop the reader.
    /// Open again for another scan; consuming self guarantees interface release.
    pub fn capture(
        self,
        options: CaptureOptions,
        cancel: &AtomicBool,
        event: impl FnMut(CaptureEvent),
    ) -> Result<Image> {
        Ok(self.capture_with_metadata(options, cancel, event)?.image)
    }

    /// Single capture with processing/coverage metadata and unknown resolution.
    pub fn capture_with_metadata(
        mut self,
        options: CaptureOptions,
        cancel: &AtomicBool,
        event: impl FnMut(CaptureEvent),
    ) -> Result<CapturedImage> {
        self.capture_next(options, cancel, event)
    }

    /// Capture without releasing the claimed interface. Rearms cancelled queues
    /// after the preceding operation's bounded protocol cleanup.
    pub fn capture_next(
        &mut self,
        options: CaptureOptions,
        cancel: &AtomicBool,
        mut event: impl FnMut(CaptureEvent),
    ) -> Result<CapturedImage> {
        self.monitor.observe(|| {
            self.transport
                .rearm()
                .map_err(|cause| transport_error(CaptureStage::Open, cause))?;
            protocol::capture_with_metadata(&mut self.transport, options, cancel, |phase| {
                self.monitor.event(phase);
                event(phase);
            })
        })
    }

    /// Successive placements with metadata, retaining existing cleanup semantics.
    pub fn capture_continuous_with_metadata(
        mut self,
        options: CaptureOptions,
        cancel: &AtomicBool,
        event: impl FnMut(CaptureEvent),
        image: impl FnMut(CapturedImage) -> Result<std::ops::ControlFlow<()>>,
    ) -> Result<()> {
        self.capture_continuous_next(options, cancel, event, image)
    }

    /// Repeated placements while retaining the interface after completion.
    pub fn capture_continuous_next(
        &mut self,
        options: CaptureOptions,
        cancel: &AtomicBool,
        event: impl FnMut(CaptureEvent),
        image: impl FnMut(CapturedImage) -> Result<std::ops::ControlFlow<()>>,
    ) -> Result<()> {
        self.capture_continuous_detailed(options, cancel, event, image)
            .map(|_| ())
    }
    pub fn capture_continuous_detailed(
        &mut self,
        options: CaptureOptions,
        cancel: &AtomicBool,
        mut event: impl FnMut(CaptureEvent),
        image: impl FnMut(CapturedImage) -> Result<std::ops::ControlFlow<()>>,
    ) -> Result<protocol::ContinuousStop> {
        let result = self.monitor.observe(|| {
            self.transport
                .rearm()
                .map_err(|cause| transport_error(CaptureStage::Open, cause))?;
            protocol::capture_continuous_detailed(
                &mut self.transport,
                options,
                cancel,
                |phase| {
                    self.monitor.event(phase);
                    event(phase);
                },
                image,
            )
        });
        if let Ok(protocol::ContinuousStop::Cancelled { stage }) = &result {
            self.monitor
                .finish(Some(crate::error::CaptureFailure::from_error(
                    &CaptureError::new(
                        CaptureErrorKind::Cancelled,
                        *stage,
                        "continuous capture cancelled",
                    )
                    .into(),
                )));
        }
        result
    }
    /// Capture each placement, waiting for finger removal between images.
    /// The callback can break to stop. Cancellation also stops successfully;
    /// device, image, callback, and cleanup failures remain errors.
    pub fn capture_continuous(
        self,
        options: CaptureOptions,
        cancel: &AtomicBool,
        event: impl FnMut(CaptureEvent),
        mut image: impl FnMut(Image) -> Result<std::ops::ControlFlow<()>>,
    ) -> Result<()> {
        self.capture_continuous_with_metadata(options, cancel, event, |captured| {
            image(captured.image)
        })
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        self.monitor.released();
    }
}

struct UsbTransport {
    identity_monitor: ReaderMonitor,
    interface: Interface,
    interrupt: Endpoint<Interrupt, In>,
    image: Endpoint<Bulk, In>,
    cancelled: bool,
}
fn rebind_identity_monitor(current: &mut ReaderMonitor, next: &ReaderMonitor) {
    if let Some(observation) = current.snapshot().last_identity_observation {
        next.identity_observed(observation);
    }
    *current = next.clone();
}
impl UsbTransport {
    fn rearm(&mut self) -> Result<()> {
        if !self.cancelled {
            return Ok(());
        }
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        while self.image.pending() > 0 {
            ensure!(
                std::time::Instant::now() < deadline,
                "image cancellation did not drain"
            );
            let _ = self.image.wait_next_complete(Duration::from_millis(20));
        }
        while self.interrupt.pending() > 0 {
            ensure!(
                std::time::Instant::now() < deadline,
                "interrupt cancellation did not drain"
            );
            let _ = self.interrupt.wait_next_complete(Duration::from_millis(20));
        }
        self.interrupt.submit(Buffer::new(64));
        self.cancelled = false;
        Ok(())
    }
}
impl Transport for UsbTransport {
    fn identity_observed(&mut self, raw: [u8; 16]) {
        self.identity_monitor
            .identity_observed(crate::reader::IdentityObservation::from_raw(raw));
    }
    fn read_registers(&mut self, register: u16, length: u16, timeout: Duration) -> Result<Vec<u8>> {
        self.interface
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: 0x04,
                    value: register,
                    index: 0,
                    length,
                },
                timeout,
            )
            .wait()
            .map_err(Into::into)
    }
    fn write_registers(&mut self, register: u16, bytes: &[u8], timeout: Duration) -> Result<()> {
        self.interface
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: 0x04,
                    value: register,
                    index: 0,
                    data: bytes,
                },
                timeout,
            )
            .wait()
            .map_err(Into::into)
    }
    fn interrupt(&mut self, timeout: Duration) -> Result<Option<u16>> {
        ensure!(
            !self.cancelled && self.interrupt.pending() > 0,
            "no interrupt transfer pending"
        );
        let Some(completion) = self.interrupt.wait_next_complete(timeout) else {
            return Ok(None);
        };
        completion
            .status
            .context("reader interrupt transfer failed")?;
        ensure!(
            completion.actual_len >= 2,
            "short reader interrupt: {} bytes",
            completion.actual_len
        );
        let event = u16::from_be_bytes([completion.buffer[0], completion.buffer[1]]);
        self.interrupt.submit(completion.buffer);
        Ok(Some(event))
    }
    fn start_image(&mut self) -> Result<()> {
        ensure!(
            !self.cancelled && self.image.pending() == 0,
            "image endpoint is not idle"
        );
        self.image.submit(Buffer::new(MAX_FRAME_SIZE));
        Ok(())
    }
    fn image(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        ensure!(
            !self.cancelled && self.image.pending() > 0,
            "no image transfer pending"
        );
        let Some(completion) = self.image.wait_next_complete(timeout) else {
            return Ok(None);
        };
        completion.status.context("reader image transfer failed")?;
        // A successful short transfer is normal; never append later frames to it.
        Ok(Some(completion.buffer.to_vec()))
    }
    fn cancel(&mut self) {
        self.cancelled = true;
        self.image.cancel_all();
        self.interrupt.cancel_all();
        // Dropping Reader immediately after capture cancels/drains backend
        // resources and releases the interface, including all endpoint references.
    }
}

#[cfg(test)]
mod hotplug_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[test]
    fn identity_rebinding_preserves_history_and_routes_new_observations_to_current_monitor() {
        let old = ReaderMonitor::default();
        old.identity_observed(crate::reader::IdentityObservation::from_raw([17; 16]));
        let observed_at = old
            .snapshot()
            .last_identity_observation
            .unwrap()
            .observed_unix_ms;
        let current = ReaderMonitor::default();
        let mut bound = old.clone();
        rebind_identity_monitor(&mut bound, &current);
        assert_eq!(
            current.snapshot().last_identity_observation.unwrap().raw,
            [17; 16]
        );
        assert_eq!(
            current
                .snapshot()
                .last_identity_observation
                .unwrap()
                .observed_unix_ms,
            observed_at
        );
        bound.identity_observed(crate::reader::IdentityObservation::from_raw([29; 16]));
        assert_eq!(
            current.snapshot().last_identity_observation.unwrap().raw,
            [29; 16]
        );
        assert_eq!(
            old.snapshot().last_identity_observation.unwrap().raw,
            [17; 16]
        );
        assert_eq!(current.snapshot().hardware_ready, None);
        assert_eq!(current.snapshot().calibration_required, None);
        assert_eq!(current.snapshot().connected, None);
    }
    #[test]
    fn rebinding_preserves_disconnection_and_wakes_current_monitor() {
        let old = ReaderMonitor::default();
        old.hotplug_monitoring(true);
        old.hotplug_disconnected();
        let binding = Arc::new(Mutex::new(old.clone()));
        let watch = ReaderHotplug {
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
            binding: binding.clone(),
        };
        let current = ReaderMonitor::default();
        watch.rebind(&current);
        assert_eq!(current.snapshot().connected, Some(false));
        assert!(current.snapshot().hotplug_monitoring);
        assert!(!old.snapshot().hotplug_monitoring);
        binding.lock().unwrap().hotplug_monitoring(false);
        assert!(!current.snapshot().hotplug_monitoring);
    }
}
