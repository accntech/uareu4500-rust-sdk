// SPDX-License-Identifier: GPL-3.0-or-later
//! Owned asynchronous capture. Close/drop cancels and joins the worker.
use crate::{
    error::CaptureFailure, metadata::CapturedImage, protocol::CaptureOptions, usb::Reader,
};
use anyhow::{Result, anyhow};
use serde::Serialize;
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default, Serialize)]
pub struct TaskStatus {
    pub complete: bool,
    pub consumed: bool,
    pub failure: Option<CaptureFailure>,
}
#[derive(Default)]
struct State {
    status: TaskStatus,
    result: Option<Result<CapturedImage>>,
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}
pub struct CaptureTask {
    cancel: Arc<AtomicBool>,
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum TaskError {
    CompletionConsumed,
    WorkerPanicked,
    InvalidWait,
}
impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::CompletionConsumed => "capture completion was already consumed",
            Self::WorkerPanicked => "capture worker panicked",
            Self::InvalidWait => "wait timeout out of range",
        })
    }
}
impl std::error::Error for TaskError {}
impl CaptureTask {
    pub fn start(reader: Reader, options: CaptureOptions) -> Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        Self::spawn(cancel, move |cancel| {
            reader.capture_with_metadata(options, &cancel, |_| {})
        })
    }
    pub(crate) fn spawn(
        cancel: Arc<AtomicBool>,
        run: impl FnOnce(Arc<AtomicBool>) -> Result<CapturedImage> + Send + 'static,
    ) -> Result<Self> {
        let shared = Arc::new(Shared::default());
        let output = shared.clone();
        let signal = cancel.clone();
        let worker = std::thread::Builder::new()
            .name("uareu-capture".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(signal)))
                    .unwrap_or_else(|_| Err(TaskError::WorkerPanicked.into()));
                let mut state = output.state.lock().unwrap_or_else(|p| p.into_inner());
                state.status.complete = true;
                state.status.failure = result.as_ref().err().map(CaptureFailure::from_error);
                state.result = Some(result);
                output.ready.notify_all();
            })?;
        Ok(Self {
            cancel,
            shared,
            worker: Mutex::new(Some(worker)),
        })
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn status(&self) -> TaskStatus {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .status
            .clone()
    }
    /// None means still pending. The completion (including errors) is consumed once.
    pub fn wait(&self, timeout: Duration) -> Result<Option<CapturedImage>> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(TaskError::InvalidWait)?;
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.status.consumed {
            return Err(TaskError::CompletionConsumed.into());
        }
        while !state.status.complete {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            state = self
                .shared
                .ready
                .wait_timeout(state, left)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        if state.status.consumed {
            return Err(TaskError::CompletionConsumed.into());
        }
        state.status.consumed = true;
        state
            .result
            .take()
            .expect("complete task has a result")
            .map(Some)
    }
    pub fn close(&self) -> Result<()> {
        self.cancel();
        // Serialize join so every close waits for the worker, even concurrent closes.
        let mut worker = self.worker.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(worker) = worker.take() {
            worker
                .join()
                .map_err(|_| anyhow!("capture worker failed"))?;
        }
        Ok(())
    }
}
impl Drop for CaptureTask {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{CaptureError, CaptureErrorKind, CaptureStage};
    #[test]
    fn task_wait_cancel_consumption_and_drop_are_bounded() {
        let signal = Arc::new(AtomicBool::new(false));
        let task = CaptureTask::spawn(signal, |cancel| {
            while !cancel.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(CaptureError::new(
                CaptureErrorKind::Cancelled,
                CaptureStage::FingerWait,
                "capture cancelled",
            )
            .into())
        })
        .unwrap();
        assert!(task.wait(Duration::ZERO).unwrap().is_none());
        task.cancel();
        let error = task.wait(Duration::from_secs(2)).unwrap_err();
        assert_eq!(
            error.downcast_ref::<CaptureError>().unwrap().kind,
            CaptureErrorKind::Cancelled
        );
        assert!(task.status().consumed);
        assert!(task.wait(Duration::ZERO).is_err());
        task.close().unwrap();
        let task = CaptureTask::spawn(Arc::new(AtomicBool::new(false)), |_| {
            panic!("injected panic")
        })
        .unwrap();
        assert!(task.wait(Duration::from_secs(2)).is_err());
        assert!(task.status().complete);
        assert_eq!(task.status().failure.unwrap().kind, CaptureErrorKind::Panic);
    }
    #[test]
    fn drop_cancels_and_joins_without_waiting_for_a_result() {
        let finished = Arc::new(AtomicBool::new(false));
        let output = finished.clone();
        let task = CaptureTask::spawn(Arc::new(AtomicBool::new(false)), move |cancel| {
            while !cancel.load(Ordering::Relaxed) {
                std::thread::yield_now();
            }
            output.store(true, Ordering::Release);
            Err(TaskError::WorkerPanicked.into())
        })
        .unwrap();
        drop(task);
        assert!(finished.load(Ordering::Acquire));
    }
}
