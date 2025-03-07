//! A single-producer, single-consumer "relay" channel wherein the
//! [send](Sender::send) future doesn't return until the receiver task has
//! received the sent value. This can be thought of as being like a
//! [mpsc::channel](tokio::sync::mpsc::channel) with capacity 0.
//!
//! Note that to use this channel at all, some form of task parallelism (eg via
//! join! or spawn) is required.
//!
//! All provided async methods are cancel-safe.
//!
//! # Examples
//! ```
//! use tokio::{join, time::Duration};
//!
//! // This future will never return
//! async fn send_then_recv() {
//!     let (mut tx, mut rx) = relay_channel::channel();
//!     tx.send(42).await.unwrap();
//!     rx.recv().await.unwrap();
//! }
//!
//! // This future will return
//! async fn join_send_and_recv() {
//!     let (mut tx, mut rx) = relay_channel::channel();
//!     let (sent, received) = join! {
//!         tx.send(42),
//!         rx.recv(),
//!     };
//!     assert_eq!(sent, Ok(()));
//!     assert_eq!(received, Some(42));
//! }
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() {
//!     let result = tokio::time::timeout(
//!         Duration::from_millis(100),
//!         send_then_recv()
//!     ).await;
//!     assert!(result.is_err());
//!     join_send_and_recv().await;
//! }
//! ```

use std::sync::Mutex;

use consume_on_drop::ConsumeOnDrop;
use tokio::{select, sync::watch};
use tokio_util::sync::{CancellationToken, DropGuard};

pub use tokio::sync::mpsc::error;

struct Inner<T> {
    // We wrap the value in a Mutex so that it is not required to be `Sync` in
    // order to be sent between threads (the value is only ever accessed with
    // Mutex::into_inner).
    tx: watch::Sender<Option<Mutex<T>>>,
    dropped: CancellationToken,
    _drop_guard: DropGuard,
}

pub struct Sender<T>(Inner<T>);

pub struct Receiver<T>(Inner<T>);

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Inner::new();
    (Sender(inner.clone()), Receiver(inner))
}

impl<T> Sender<T> {
    /// Only one thread can send a value at a time on a [relay_channel](crate).
    /// This is enforced by making [Sender] not implement [Clone] and the
    /// [send](Self::send) method require an unshared reference.
    pub async fn send(&mut self, value: T) -> Result<(), error::SendError<T>> {
        self.0.send(value).await
    }
}

impl<T> Receiver<T> {
    /// Returns [None] if and when the sender has been dropped.
    pub async fn recv(&mut self) -> Option<T> {
        self.0.recv().await
    }

    pub fn try_recv(&mut self) -> Result<T, error::TryRecvError> {
        self.0.try_recv()
    }
}

impl<T> Inner<T> {
    fn new() -> Self {
        let tx = watch::Sender::new(None);
        let dropped = CancellationToken::new();
        let _drop_guard = dropped.clone().drop_guard();
        Self {
            tx,
            dropped,
            _drop_guard,
        }
    }

    async fn send(&mut self, value: T) -> Result<(), error::SendError<T>> {
        let mut rx = self.tx.subscribe();
        let old_value = self.tx.send_replace(Some(Mutex::new(value)));
        assert!(old_value.is_none());
        let on_drop = ConsumeOnDrop::new(|| {
            let _: Option<Mutex<T>> = self.tx.send_replace(None);
        });
        select! {
            result = rx.wait_for(|value| value.is_none()) => {
                let none = result.unwrap();
                assert!(none.is_none());
            },
            () = self.dropped.cancelled() => {}
        }
        let _disarmed = ConsumeOnDrop::into_inner(on_drop);
        match self.tx.send_replace(None) {
            Some(value) => Err(error::SendError(value.into_inner().unwrap())),
            None => Ok(()),
        }
    }

    async fn recv(&mut self) -> Option<T> {
        let mut rx = self.tx.subscribe();
        loop {
            select! {
                result = rx.wait_for(|value| value.is_some()) => {
                    let some = result.unwrap();
                    assert!(some.is_some());
                }
                () = self.dropped.cancelled() => {},
            }
            // The sender could have been aborted between dropping the guard
            // returned by `wait_for` and this `send_replace` call so we need to
            // double-check that the value is still there.
            match self.tx.send_replace(None) {
                Some(value) => return Some(value.into_inner().unwrap()),
                None => {
                    if self.dropped.is_cancelled() {
                        return None;
                    }
                }
            }
        }
    }

    fn try_recv(&mut self) -> Result<T, error::TryRecvError> {
        match self.tx.send_replace(None) {
            Some(value) => Ok(value.into_inner().unwrap()),
            None => {
                if self.dropped.is_cancelled() {
                    Err(error::TryRecvError::Disconnected)
                } else {
                    Err(error::TryRecvError::Empty)
                }
            }
        }
    }
}

impl<T> Clone for Inner<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            dropped: self.dropped.clone(),
            _drop_guard: self.dropped.clone().drop_guard(),
        }
    }
}

#[cfg(test)]
mod tests;
