use std::error::Error;
use std::fmt;

pub use tokio::sync::mpsc::error::SendError;
pub use tokio::sync::mpsc::error::TryRecvError;

// ===== TrySendError =====

/// This enumeration is the list of the possible error outcomes for the
/// [`try_send`](super::Sender::try_send) method.
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum TrySendError<T> {
    Closed(T),
    NotWaiting(T),
}

impl<T> TrySendError<T> {
    /// Consume the `TrySendError`, returning the unsent value.
    pub fn into_inner(self) -> T {
        match self {
            TrySendError::NotWaiting(val) => val,
            TrySendError::Closed(val) => val,
        }
    }
}

impl<T> fmt::Debug for TrySendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            TrySendError::NotWaiting(..) => "NotWaiting(..)".fmt(f),
            TrySendError::Closed(..) => "Closed(..)".fmt(f),
        }
    }
}

impl<T> fmt::Display for TrySendError<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            fmt,
            "{}",
            match self {
                TrySendError::NotWaiting(..) => "receiver is not waiting",
                TrySendError::Closed(..) => "channel closed",
            }
        )
    }
}

impl<T> Error for TrySendError<T> {}

impl<T> From<SendError<T>> for TrySendError<T> {
    fn from(src: SendError<T>) -> TrySendError<T> {
        TrySendError::Closed(src.0)
    }
}
