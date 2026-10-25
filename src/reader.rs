// SPDX-License-Identifier: GPL-3.0-or-later
//! SDK capabilities and cached lifecycle observations, not live hardware probes.
use crate::{error::CaptureFailure, protocol::CaptureEvent};
use serde::Serialize;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Unknown,
    Supported,
    Unsupported,
}

#[derive(Clone, Debug, Serialize)]
pub struct SdkCapabilities {
    pub capture: bool,
    pub cancellation: bool,
    pub phase_callbacks: bool,
    pub continuous_capture_rust: bool,
    pub continuous_capture_c: bool,
    pub asynchronous_capture: bool,
    pub live_streaming: bool,
    pub calibration: bool,
    pub template_extraction: bool,
    pub matching: bool,
    pub persistent_sessions: bool,
    pub nfiq2_c: bool,
    pub display_enhancement: bool,
    pub recognition_backend: &'static str,
    pub wsq_backend: &'static str,
    pub hardware_status_probe: bool,
    pub soft_recovery: bool,
    pub interchange_formats: &'static [&'static str],
}
#[derive(Clone, Debug, Serialize)]
pub struct DeviceCapabilities {
    pub capture: Capability,
    pub streaming: Capability,
    pub calibration: Capability,
    pub piv: Capability,
    pub liveness: Capability,
    pub led_control: Capability,
    pub feature_extraction: Capability,
    pub matching: Capability,
    pub identification: Capability,
    pub onboard_storage: Capability,
    pub power_management: Capability,
    pub resolutions_ppi: Option<Vec<u32>>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ReaderCapabilities {
    pub schema_version: u32,
    pub source: &'static str,
    pub sdk: SdkCapabilities,
    pub device: DeviceCapabilities,
}
impl ReaderCapabilities {
    /// This describes the implemented USB profile, not a device capability query.
    pub fn for_usb_id(vendor: u16, product: u16) -> Option<Self> {
        crate::usb::supported_id(vendor, product).then_some(Self {
            schema_version: 1,
            source: "sdk_profile_not_device_query",
            sdk: SdkCapabilities {
                capture: true,
                cancellation: true,
                phase_callbacks: true,
                continuous_capture_rust: true,
                continuous_capture_c: true,
                asynchronous_capture: true,
                live_streaming: false,
                calibration: false,
                template_extraction: false,
                matching: false,
                persistent_sessions: true,
                nfiq2_c: false,
                display_enhancement: false,
                recognition_backend: "not_implemented",
                wsq_backend: "not_implemented",
                hardware_status_probe: true,
                soft_recovery: true,
                interchange_formats: &[],
            },
            device: DeviceCapabilities {
                capture: Capability::Unknown,
                streaming: Capability::Unknown,
                calibration: Capability::Unknown,
                piv: Capability::Unknown,
                liveness: Capability::Unknown,
                led_control: Capability::Unknown,
                feature_extraction: Capability::Unknown,
                matching: Capability::Unknown,
                identification: Capability::Unknown,
                onboard_storage: Capability::Unknown,
                power_management: Capability::Unknown,
                resolutions_ppi: None,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderOperation {
    Idle,
    Opening,
    Initializing,
    WaitingForFinger,
    WaitingForFingerRemoval,
    Capturing,
    Closed,
    Probing,
}
#[derive(Clone, Debug, Serialize)]
pub struct CaptureOutcome {
    pub success: bool,
    pub failure: Option<CaptureFailure>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ReaderStatus {
    pub schema_version: u32,
    pub source: &'static str,
    /// Monotonic observation counter local to this monitor; not a timestamp.
    pub sequence: u64,
    pub operation: ReaderOperation,
    pub capture_active: bool,
    pub interface_claimed: bool,
    /// Cached observation, valid only for the indicated phase; never a live probe.
    pub finger_present: Option<bool>,
    pub hardware_ready: Option<bool>,
    pub calibration_required: Option<bool>,
    pub connected: Option<bool>,
    pub last_outcome: Option<CaptureOutcome>,
    pub last_hardware_observation: Option<HardwareObservation>,
    pub last_probe_failure: Option<CaptureFailure>,
}
#[derive(Clone, Debug, Serialize)]
pub struct HardwareObservation {
    pub source: &'static str,
    pub observed_unix_ms: Option<u64>,
    pub connected_at_observation: bool,
    pub hardware_status_register: u8,
    pub low_power: bool,
    pub hardware_ready: Option<bool>,
    pub calibration_required: Option<bool>,
}
impl Default for ReaderStatus {
    fn default() -> Self {
        Self {
            schema_version: 1,
            source: "cached_sdk_lifecycle",
            sequence: 0,
            operation: ReaderOperation::Idle,
            capture_active: false,
            interface_claimed: false,
            finger_present: None,
            hardware_ready: None,
            calibration_required: None,
            connected: None,
            last_outcome: None,
            last_hardware_observation: None,
            last_probe_failure: None,
        }
    }
}
/// Clone before consuming Reader in capture, then query from another thread.
/// Queries never perform USB I/O and cannot establish current physical readiness.
#[derive(Clone, Debug, Default)]
pub struct ReaderMonitor(Arc<Mutex<ReaderStatus>>);
impl ReaderMonitor {
    pub(crate) fn begin_probe(&self) {
        self.change(|s| {
            if s.operation != ReaderOperation::Closed {
                s.operation = ReaderOperation::Probing;
            }
            s.capture_active = false;
        });
    }
    pub(crate) fn finish_probe(
        &self,
        result: std::result::Result<HardwareObservation, CaptureFailure>,
    ) {
        self.change(|s| {
            if s.operation != ReaderOperation::Closed {
                s.operation = ReaderOperation::Idle;
            }
            match result {
                Ok(observed) => {
                    s.last_hardware_observation = Some(observed);
                    s.last_probe_failure = None;
                }
                Err(error) => s.last_probe_failure = Some(error),
            }
        });
    }
    fn change(&self, update: impl FnOnce(&mut ReaderStatus)) {
        let mut state = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        state.sequence = state.sequence.saturating_add(1);
        update(&mut state);
    }
    pub fn snapshot(&self) -> ReaderStatus {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }
    pub(crate) fn claimed(&self) {
        self.change(|s| s.interface_claimed = true);
    }
    pub(crate) fn released(&self) {
        self.change(|s| s.interface_claimed = false);
    }
    pub(crate) fn begin(&self) {
        self.change(|s| {
            if s.operation != ReaderOperation::Closed {
                s.operation = ReaderOperation::Opening;
            }
            s.capture_active = true;
            s.finger_present = None;
            s.last_outcome = None;
        });
    }
    pub(crate) fn event(&self, event: CaptureEvent) {
        self.change(|s| {
            // A closed handle stays closed while its retained capture unwinds.
            if s.operation == ReaderOperation::Closed {
                return;
            }
            s.operation = match event {
                CaptureEvent::Initializing => ReaderOperation::Initializing,
                CaptureEvent::WaitingForFinger => ReaderOperation::WaitingForFinger,
                CaptureEvent::WaitingForFingerRemoval => ReaderOperation::WaitingForFingerRemoval,
                CaptureEvent::Capturing => ReaderOperation::Capturing,
            };
            s.finger_present = (event == CaptureEvent::Capturing).then_some(true);
        });
    }
    pub(crate) fn finish(&self, failure: Option<CaptureFailure>) {
        self.change(|s| {
            if s.operation != ReaderOperation::Closed {
                s.operation = ReaderOperation::Idle;
            }
            s.capture_active = false;
            s.finger_present = None;
            s.last_outcome = Some(CaptureOutcome {
                success: failure.is_none(),
                failure,
            });
        });
    }
    pub(crate) fn close(&self) {
        self.change(|s| {
            s.operation = ReaderOperation::Closed;
            s.finger_present = None;
        });
    }
    pub(crate) fn observe<T>(&self, run: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
        struct Guard(ReaderMonitor);
        impl Drop for Guard {
            fn drop(&mut self) {
                if self.0.snapshot().capture_active {
                    self.0.finish(Some(CaptureFailure {
                        kind: if std::thread::panicking() {
                            crate::error::CaptureErrorKind::Panic
                        } else {
                            crate::error::CaptureErrorKind::Other
                        },
                        stage: None,
                        message: "capture aborted before completion".into(),
                        cleanup: None,
                    }));
                }
            }
        }
        self.begin();
        let _guard = Guard(self.clone());
        let result = run();
        self.finish(result.as_ref().err().map(CaptureFailure::from_error));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{CaptureError, CaptureErrorKind, CaptureStage};
    #[test]
    fn capabilities_never_infer_device_features_from_usb_identity() {
        assert!(ReaderCapabilities::for_usb_id(0, 0).is_none());
        let caps = ReaderCapabilities::for_usb_id(0x05ba, 0x000a).unwrap();
        assert!(caps.sdk.capture && caps.sdk.continuous_capture_rust);
        assert!(caps.sdk.continuous_capture_c && caps.sdk.asynchronous_capture);
        let device = serde_json::to_value(caps.device).unwrap();
        for (key, value) in device.as_object().unwrap() {
            if key == "resolutions_ppi" {
                assert!(value.is_null());
            } else {
                assert_eq!(value, "unknown");
            }
        }
    }
    #[test]
    fn monitor_is_reentrant_and_clears_cached_finger_after_failure() {
        let monitor = ReaderMonitor::default();
        monitor.claimed();
        let shared = monitor.clone();
        let error = monitor
            .observe::<()>(|| {
                monitor.event(CaptureEvent::WaitingForFinger);
                assert_eq!(shared.snapshot().finger_present, None);
                monitor.event(CaptureEvent::Capturing);
                let snapshot = std::thread::spawn(move || shared.snapshot())
                    .join()
                    .unwrap();
                assert!(snapshot.capture_active && snapshot.interface_claimed);
                assert_eq!(snapshot.operation, ReaderOperation::Capturing);
                assert_eq!(snapshot.finger_present, Some(true));
                assert_eq!(snapshot.hardware_ready, None);
                assert_eq!(snapshot.connected, None);
                Err(CaptureError::new(
                    CaptureErrorKind::Timeout,
                    CaptureStage::Acquisition,
                    "capture deadline exceeded",
                )
                .into())
            })
            .unwrap_err();
        assert!(error.is::<CaptureError>());
        let status = monitor.snapshot();
        assert!(!status.capture_active);
        assert_eq!(status.finger_present, None);
        assert_eq!(status.operation, ReaderOperation::Idle);
        assert_eq!(
            status.last_outcome.unwrap().failure.unwrap().kind,
            CaptureErrorKind::Timeout
        );
        monitor.released();
        assert!(!monitor.snapshot().interface_claimed);
        monitor.observe(|| Ok(())).unwrap();
        assert!(monitor.snapshot().last_outcome.unwrap().success);
    }
    #[test]
    fn panic_and_close_do_not_leave_a_busy_or_reopened_status() {
        let monitor = ReaderMonitor::default();
        let result = std::panic::catch_unwind(|| {
            monitor.observe::<()>(|| {
                monitor.close();
                monitor.event(CaptureEvent::Capturing);
                assert_eq!(monitor.snapshot().operation, ReaderOperation::Closed);
                panic!("injected callback panic");
            })
        });
        assert!(result.is_err());
        let status = monitor.snapshot();
        assert!(!status.capture_active);
        assert_eq!(status.operation, ReaderOperation::Closed);
        assert!(!status.last_outcome.unwrap().success);
        monitor
            .observe(|| {
                assert_eq!(monitor.snapshot().operation, ReaderOperation::Closed);
                Ok(())
            })
            .unwrap();
        assert_eq!(monitor.snapshot().operation, ReaderOperation::Closed);
    }
}
