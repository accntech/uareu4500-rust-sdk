// SPDX-License-Identifier: GPL-3.0-or-later
//! SDK capabilities and cached lifecycle observations, not live hardware probes.
use crate::{error::CaptureFailure, protocol::CaptureEvent};
use serde::Serialize;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

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
    pub status_change_wait: bool,
    pub multiview_interchange: bool,
    pub record_editing: bool,
    pub wsq_target_size: bool,
    pub quality_record_view: bool,
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
                template_extraction: true,
                matching: true,
                persistent_sessions: true,
                nfiq2_c: true,
                display_enhancement: true,
                recognition_backend: "optional_local_sourceafis_helper_required",
                wsq_backend: "optional_external_nbis_tools_required",
                hardware_status_probe: true,
                soft_recovery: true,
                status_change_wait: true,
                multiview_interchange: true,
                record_editing: true,
                wsq_target_size: true,
                quality_record_view: true,
                interchange_formats: &[
                    "iso19794_4_2005_raw_single_view",
                    "iso19794_2_2005_single_view",
                    "ansi378_2004_single_view",
                    "iso19794_2_2005_multiview",
                    "ansi378_2004_multiview",
                    "iso19794_4_2005_raw_multi_image",
                    "iso19794_4_2005_opaque_wsq_multi_image",
                    "iso19794_4_2005_opaque_jpeg2000_multi_image",
                ],
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
    /// Historical raw initialization information, not a live identity/version query.
    pub last_identity_observation: Option<IdentityObservation>,
    pub last_probe_failure: Option<CaptureFailure>,
    /// True while an owned OS USB hotplug subscription is active.
    pub hotplug_monitoring: bool,
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
/// Opaque register-f0 information retained from ordinary initialization. Upstream
/// logs these byte pairs as versions but explicitly does not know which is
/// hardware versus firmware. Neither pair supplies DPI or capability evidence.
#[derive(Clone, Debug, Serialize)]
pub struct IdentityObservation {
    pub source: &'static str,
    pub protocol_reference: &'static str,
    pub observed_unix_ms: Option<u64>,
    pub raw: [u8; 16],
    pub version_pair_10_11: [u8; 2],
    pub version_pair_4_5: [u8; 2],
}
impl IdentityObservation {
    pub(crate) fn from_raw(raw: [u8; 16]) -> Self {
        Self {
            source: "usb_register_f0_during_initialization",
            protocol_reference: "https://gitlab.freedesktop.org/libfprint/libfprint/-/blob/6f9479c3d55f847c1b3769f28ceb99227f9858cf/libfprint/drivers/uru4000.c#L1192-1206",
            observed_unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .and_then(|d| u64::try_from(d.as_millis()).ok()),
            version_pair_10_11: [raw[10], raw[11]],
            version_pair_4_5: [raw[4], raw[5]],
            raw,
        }
    }
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
            last_identity_observation: None,
            last_probe_failure: None,
            hotplug_monitoring: false,
        }
    }
}
/// Clone before consuming Reader in capture, then query from another thread.
/// Queries never perform USB I/O and cannot establish current physical readiness.
#[derive(Clone, Debug, Default)]
pub struct ReaderMonitor(Arc<(Mutex<ReaderStatus>, Condvar)>);
impl ReaderMonitor {
    pub(crate) fn identity_observed(&self, observation: IdentityObservation) {
        self.change(|s| s.last_identity_observation = Some(observation));
    }
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
        let mut state = self.0.0.lock().unwrap_or_else(|poison| poison.into_inner());
        state.sequence = state.sequence.saturating_add(1);
        update(&mut state);
        drop(state);
        self.0.1.notify_all();
    }
    pub fn snapshot(&self) -> ReaderStatus {
        self.0
            .0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }
    /// Wait for a newer lifecycle or USB-hotplug observation. No polling USB I/O.
    /// Returns None on timeout or if already closed without a newer sequence.
    /// Deliberately caps a single wait to five minutes; callers can wait again.
    pub fn wait_for_change(&self, after_sequence: u64, timeout: Duration) -> Option<ReaderStatus> {
        let state = self.0.0.lock().unwrap_or_else(|p| p.into_inner());
        let (state, _) = self
            .0
            .1
            .wait_timeout_while(state, timeout.min(Duration::from_secs(300)), |s| {
                s.sequence <= after_sequence && s.operation != ReaderOperation::Closed
            })
            .unwrap_or_else(|p| p.into_inner());
        (state.sequence > after_sequence).then(|| state.clone())
    }
    pub(crate) fn hotplug_monitoring(&self, active: bool) {
        self.change(|s| {
            s.hotplug_monitoring = active;
            if active {
                s.source = "sdk_lifecycle_and_usb_hotplug";
            }
        });
    }
    pub(crate) fn hotplug_disconnected(&self) {
        self.change(|s| {
            s.connected = Some(false);
            s.hardware_ready = None;
            s.finger_present = None;
        });
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
    fn identity_observation_is_historical_raw_evidence_without_inferred_device_status() {
        let monitor = ReaderMonitor::default();
        assert!(monitor.snapshot().last_identity_observation.is_none());
        let raw = std::array::from_fn(|i| i as u8);
        let after = monitor.snapshot().sequence;
        monitor.identity_observed(IdentityObservation::from_raw(raw));
        let observed = monitor.wait_for_change(after, Duration::ZERO).unwrap();
        let info = observed.last_identity_observation.unwrap();
        assert_eq!(info.raw, raw);
        assert_eq!(info.version_pair_10_11, [10, 11]);
        assert_eq!(info.version_pair_4_5, [4, 5]);
        assert!(
            info.protocol_reference
                .contains("6f9479c3d55f847c1b3769f28ceb99227f9858cf")
        );
        assert_eq!(observed.connected, None);
        assert_eq!(observed.hardware_ready, None);
        assert_eq!(observed.calibration_required, None);
        monitor.finish(Some(CaptureFailure {
            kind: CaptureErrorKind::Other,
            stage: Some(CaptureStage::Initialization),
            message: "failure after information read".into(),
            cleanup: None,
        }));
        monitor.hotplug_disconnected();
        monitor.close();
        let retained = monitor.snapshot().last_identity_observation.unwrap();
        assert_eq!(retained.raw, raw);
        assert_eq!(retained.observed_unix_ms, info.observed_unix_ms);
    }
    #[test]
    fn status_waits_wake_on_observations_and_close_without_claiming_readiness() {
        let monitor = ReaderMonitor::default();
        let after = monitor.snapshot().sequence;
        let other = monitor.clone();
        let waiter =
            std::thread::spawn(move || other.wait_for_change(after, Duration::from_secs(1)));
        monitor.hotplug_disconnected();
        let result = waiter.join().unwrap().unwrap();
        assert_eq!(result.connected, Some(false));
        assert_eq!(result.hardware_ready, None);
        let after = result.sequence;
        let other = monitor.clone();
        let waiter =
            std::thread::spawn(move || other.wait_for_change(after, Duration::from_secs(1)));
        monitor.close();
        assert_eq!(
            waiter.join().unwrap().unwrap().operation,
            ReaderOperation::Closed
        );
        assert!(
            monitor
                .wait_for_change(u64::MAX, Duration::from_secs(1))
                .is_none()
        );
    }
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
