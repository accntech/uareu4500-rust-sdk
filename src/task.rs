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
    worker_id: std::thread::ThreadId,
}
type Completion = Box<dyn FnOnce(Result<CapturedImage>) + Send + 'static>;
#[derive(Debug, PartialEq, Eq)]
pub enum TaskError {
    CompletionConsumed,
    WorkerPanicked,
    InvalidWait,
    CloseFromWorker,
}
impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::CompletionConsumed => "capture completion was already consumed",
            Self::WorkerPanicked => "capture worker panicked",
            Self::InvalidWait => "wait timeout out of range",
            Self::CloseFromWorker => "cannot close a capture task from its own completion callback",
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
    /// Deliver the owned capture result once on the capture worker. Polling its
    /// status remains available, but `wait` cannot consume the callback's result.
    /// Close joins callback completion; a callback cannot close its own task.
    pub fn start_with_completion(
        reader: Reader,
        options: CaptureOptions,
        completion: impl FnOnce(Result<CapturedImage>) + Send + 'static,
    ) -> Result<Self> {
        Self::spawn_with_completion(
            Arc::new(AtomicBool::new(false)),
            move |cancel| reader.capture_with_metadata(options, &cancel, |_| {}),
            completion,
        )
    }
    pub(crate) fn spawn(
        cancel: Arc<AtomicBool>,
        run: impl FnOnce(Arc<AtomicBool>) -> Result<CapturedImage> + Send + 'static,
    ) -> Result<Self> {
        Self::spawn_inner(cancel, run, None)
    }
    pub(crate) fn spawn_with_completion(
        cancel: Arc<AtomicBool>,
        run: impl FnOnce(Arc<AtomicBool>) -> Result<CapturedImage> + Send + 'static,
        completion: impl FnOnce(Result<CapturedImage>) + Send + 'static,
    ) -> Result<Self> {
        Self::spawn_inner(cancel, run, Some(Box::new(completion)))
    }
    fn spawn_inner(
        cancel: Arc<AtomicBool>,
        run: impl FnOnce(Arc<AtomicBool>) -> Result<CapturedImage> + Send + 'static,
        completion: Option<Completion>,
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
                if let Some(completion) = completion {
                    state.status.consumed = true;
                    output.ready.notify_all();
                    drop(state);
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| completion(result)))
                        .is_err()
                    {
                        let mut state = output.state.lock().unwrap_or_else(|p| p.into_inner());
                        state.status.failure = Some(CaptureFailure::from_error(
                            &TaskError::WorkerPanicked.into(),
                        ));
                    }
                } else {
                    state.result = Some(result);
                    output.ready.notify_all();
                }
            })?;
        let worker_id = worker.thread().id();
        Ok(Self {
            cancel,
            shared,
            worker: Mutex::new(Some(worker)),
            worker_id,
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
        // Check before locking: another thread can hold the join mutex while
        // waiting for this callback, which must never wait for that mutex.
        if std::thread::current().id() == self.worker_id {
            return Err(TaskError::CloseFromWorker.into());
        }
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
    fn completion_callback_owns_result_and_close_joins_delivery() {
        let delivered = Arc::new(AtomicBool::new(false));
        let output = delivered.clone();
        let task = CaptureTask::spawn_with_completion(
            Arc::new(AtomicBool::new(false)),
            |_| {
                Ok(CapturedImage::new(crate::frame::Image {
                    width: 1,
                    height: 1,
                    pixels: vec![42],
                }))
            },
            move |result| {
                assert_eq!(result.unwrap().image.pixels, [42]);
                output.store(true, Ordering::Release);
            },
        )
        .unwrap();
        task.close().unwrap();
        assert!(delivered.load(Ordering::Acquire));
        assert!(task.status().complete && task.status().consumed);
        assert!(task.wait(Duration::ZERO).unwrap_err().is::<TaskError>());
    }

    #[test]
    fn completion_callback_receives_cancelled_error_once() {
        let (send, receive) = std::sync::mpsc::channel();
        let task = CaptureTask::spawn_with_completion(
            Arc::new(AtomicBool::new(false)),
            |cancel| {
                while !cancel.load(Ordering::Relaxed) {
                    std::thread::yield_now();
                }
                Err(CaptureError::new(
                    CaptureErrorKind::Cancelled,
                    CaptureStage::FingerWait,
                    "cancelled",
                )
                .into())
            },
            move |result| {
                send.send(CaptureFailure::from_error(&result.unwrap_err()).kind)
                    .unwrap();
            },
        )
        .unwrap();
        task.cancel();
        assert_eq!(
            receive.recv_timeout(Duration::from_secs(2)).unwrap(),
            CaptureErrorKind::Cancelled
        );
        task.close().unwrap();
        assert!(receive.try_recv().is_err());
    }

    #[test]
    fn completion_callback_cannot_join_its_own_worker() {
        let (send, receive) = std::sync::mpsc::channel::<std::sync::Weak<CaptureTask>>();
        let (result_send, result_receive) = std::sync::mpsc::channel();
        let task = Arc::new(
            CaptureTask::spawn_with_completion(
                Arc::new(AtomicBool::new(false)),
                |_| Err(anyhow!("injected failure")),
                move |_| {
                    let task = receive
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap()
                        .upgrade()
                        .unwrap();
                    result_send
                        .send(task.close().unwrap_err().downcast::<TaskError>().unwrap())
                        .unwrap();
                },
            )
            .unwrap(),
        );
        send.send(Arc::downgrade(&task)).unwrap();
        assert_eq!(
            result_receive.recv_timeout(Duration::from_secs(2)).unwrap(),
            TaskError::CloseFromWorker
        );
        task.close().unwrap();
    }
    #[test]
    fn callback_self_close_stays_nonblocking_while_external_close_is_joining() {
        let (send, receive) = std::sync::mpsc::channel::<std::sync::Weak<CaptureTask>>();
        let (result_send, result_receive) = std::sync::mpsc::channel();
        let task = Arc::new(
            CaptureTask::spawn_with_completion(
                Arc::new(AtomicBool::new(false)),
                |_| Err(anyhow!("injected")),
                move |_| {
                    let task = receive
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap()
                        .upgrade()
                        .unwrap();
                    result_send
                        .send(task.close().unwrap_err().downcast::<TaskError>().unwrap())
                        .unwrap();
                },
            )
            .unwrap(),
        );
        let closing = task.clone();
        let close = std::thread::spawn(move || closing.close());
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            if matches!(
                task.worker.try_lock(),
                Err(std::sync::TryLockError::WouldBlock)
            ) {
                break;
            }
            assert!(
                Instant::now() < until,
                "external close never acquired the join mutex"
            );
            std::thread::yield_now();
        }
        send.send(Arc::downgrade(&task)).unwrap();
        assert_eq!(
            result_receive.recv_timeout(Duration::from_secs(2)).unwrap(),
            TaskError::CloseFromWorker
        );
        close.join().unwrap().unwrap();
    }
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
