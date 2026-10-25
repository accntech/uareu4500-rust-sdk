// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed capture causes carried through the existing anyhow::Result APIs.
use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CaptureErrorKind {
    Cancelled,
    Timeout,
    Disconnected,
    Busy,
    PermissionDenied,
    InvalidArgument,
    Unsupported,
    InvalidFrame,
    InvalidResponse,
    DeviceFailure,
    Transport,
    Other,
    Panic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStage {
    Open,
    Initialization,
    FingerWait,
    Acquisition,
    Decode,
    Cleanup,
}

#[derive(Debug)]
pub struct CaptureError {
    pub kind: CaptureErrorKind,
    pub stage: CaptureStage,
    message: String,
}
impl CaptureError {
    pub fn new(kind: CaptureErrorKind, stage: CaptureStage, message: impl Into<String>) -> Self {
        Self {
            kind,
            stage,
            message: message.into(),
        }
    }
    /// Attach typed context while preserving downcasts to the original cause.
    pub fn from_source(
        kind: CaptureErrorKind,
        stage: CaptureStage,
        cause: anyhow::Error,
    ) -> anyhow::Error {
        cause.context(Self::new(
            kind,
            stage,
            format!("capture failed during {stage:?}"),
        ))
    }
}
impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for CaptureError {}

/// Serializable diagnostic snapshot; no image or fingerprint data is included.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CaptureFailure {
    pub kind: CaptureErrorKind,
    pub stage: Option<CaptureStage>,
    pub message: String,
    pub cleanup: Option<Box<CaptureFailure>>,
}
impl CaptureFailure {
    pub fn from_error(error: &anyhow::Error) -> Self {
        let typed = error.downcast_ref::<CaptureError>();
        Self {
            kind: typed.map_or_else(
                || {
                    if error.downcast_ref::<crate::task::TaskError>()
                        == Some(&crate::task::TaskError::WorkerPanicked)
                    {
                        CaptureErrorKind::Panic
                    } else {
                        CaptureErrorKind::Other
                    }
                },
                |error| error.kind,
            ),
            stage: typed.map(|error| error.stage),
            message: format!("{error:#}"),
            cleanup: error
                .downcast_ref::<CleanupFailure>()
                .map(|e| Box::new(e.failure.clone())),
        }
    }
}
#[derive(Debug)]
pub(crate) struct CleanupFailure {
    pub failure: CaptureFailure,
}
impl fmt::Display for CleanupFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cleanup also failed: {}", self.failure.message)
    }
}
pub(crate) fn attach_cleanup(primary: anyhow::Error, cleanup: anyhow::Result<()>) -> anyhow::Error {
    match cleanup {
        Ok(()) => primary,
        Err(error) => {
            let mut failure = CaptureFailure::from_error(&error);
            if let Some(previous) = primary.downcast_ref::<CleanupFailure>() {
                let mut chain = previous.failure.clone();
                let mut tail = &mut chain;
                while let Some(ref mut next) = tail.cleanup {
                    tail = next;
                }
                tail.cleanup = Some(Box::new(failure));
                failure = chain;
            }
            primary.context(CleanupFailure { failure })
        }
    }
}

/// Classify backend types, never their text. A cancelled USB transfer can also
/// mean a backend timeout, so it remains Transport unless the protocol itself
/// observed cancellation or deadline expiry.
pub(crate) fn transport_error(stage: CaptureStage, cause: anyhow::Error) -> anyhow::Error {
    if cause.is::<CaptureError>() {
        return cause;
    }
    let kind = if let Some(error) = cause.downcast_ref::<nusb::Error>() {
        match error.kind() {
            nusb::ErrorKind::Disconnected => CaptureErrorKind::Disconnected,
            nusb::ErrorKind::Busy => CaptureErrorKind::Busy,
            nusb::ErrorKind::PermissionDenied => CaptureErrorKind::PermissionDenied,
            nusb::ErrorKind::Unsupported => CaptureErrorKind::Unsupported,
            _ => CaptureErrorKind::Transport,
        }
    } else if let Some(error) = cause.downcast_ref::<nusb::transfer::TransferError>() {
        match error {
            nusb::transfer::TransferError::Disconnected => CaptureErrorKind::Disconnected,
            _ => CaptureErrorKind::Transport,
        }
    } else if let Some(error) = cause.downcast_ref::<std::io::Error>() {
        match error.kind() {
            std::io::ErrorKind::NotConnected | std::io::ErrorKind::ConnectionAborted => {
                CaptureErrorKind::Disconnected
            }
            std::io::ErrorKind::PermissionDenied => CaptureErrorKind::PermissionDenied,
            std::io::ErrorKind::TimedOut => CaptureErrorKind::Timeout,
            _ => CaptureErrorKind::Transport,
        }
    } else {
        CaptureErrorKind::Transport
    };
    CaptureError::from_source(kind, stage, cause)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backend_categories_preserve_original_types_without_matching_text() {
        for (original, expected) in [
            (std::io::ErrorKind::TimedOut, CaptureErrorKind::Timeout),
            (
                std::io::ErrorKind::NotConnected,
                CaptureErrorKind::Disconnected,
            ),
            (
                std::io::ErrorKind::PermissionDenied,
                CaptureErrorKind::PermissionDenied,
            ),
            (std::io::ErrorKind::Other, CaptureErrorKind::Transport),
        ] {
            let error = transport_error(
                CaptureStage::Open,
                std::io::Error::new(original, "timeout cancelled disconnected").into(),
            );
            assert_eq!(error.downcast_ref::<CaptureError>().unwrap().kind, expected);
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().kind(),
                original
            );
            assert!(format!("{error:#}").contains("timeout cancelled disconnected"));
        }
        let ambiguous = transport_error(
            CaptureStage::Acquisition,
            nusb::transfer::TransferError::Cancelled.into(),
        );
        assert_eq!(
            ambiguous.downcast_ref::<CaptureError>().unwrap().kind,
            CaptureErrorKind::Transport
        );
        let again = transport_error(CaptureStage::Cleanup, ambiguous);
        assert_eq!(
            again.downcast_ref::<CaptureError>().unwrap().stage,
            CaptureStage::Acquisition
        );
    }
}
