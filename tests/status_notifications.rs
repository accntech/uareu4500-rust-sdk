// SPDX-License-Identifier: GPL-3.0-or-later
use std::time::{Duration, Instant};
use uareu::reader::ReaderMonitor;
#[test]
fn waiting_for_status_change_is_bounded_and_returns_snapshot_sequence() {
    let monitor = ReaderMonitor::default();
    let initial = monitor.snapshot();
    let start = Instant::now();
    assert!(
        monitor
            .wait_for_change(initial.sequence, Duration::from_millis(20))
            .is_none()
    );
    assert!(start.elapsed() < Duration::from_secs(1));
}
