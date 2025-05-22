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

use std::mem;

use consume_on_drop::ConsumeOnDrop;
use derive_where::derive_where;
use sync_wrapper::SyncWrapper;
use tokio::sync::watch;

pub mod error;

#[derive_where(Clone)]
struct Inner<T>(watch::Sender<ValueHolder<T>>);

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        self.0.send_modify(|holder| holder.dropped = true);
    }
}

#[derive_where(Default)]
struct ValueHolder<T> {
    sender_slot: Option<SyncWrapper<T>>,
    receiver_slot: ReceiverSlot<T>,
    dropped: bool,
}

#[derive_where(Default)]
enum ReceiverSlot<T> {
    #[derive_where(default)]
    NoReceiver,
    ReceiverWaiting,
    Received(SyncWrapper<T>),
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
    /// [send](Self::send) method require an unshared (`&mut self`) reference.
    pub async fn send(&mut self, value: T) -> Result<(), error::SendError<T>> {
        self.0.send(value).await
    }

    pub fn try_send(&mut self, value: T) -> Result<(), error::TrySendError<T>> {
        self.0.try_send(value)
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
        Self(watch::Sender::new(ValueHolder::default()))
    }

    async fn send(&mut self, value: T) -> Result<(), self::error::SendError<T>> {
        let mut received = false;
        self.0.send_modify(|holder| match holder.receiver_slot {
            ReceiverSlot::ReceiverWaiting => {
                holder.receiver_slot = ReceiverSlot::Received(SyncWrapper::new(value));
                received = true;
            }
            _ => {
                let old_value = holder.sender_slot.replace(SyncWrapper::new(value));
                assert!(old_value.is_none());
            }
        });
        if received {
            return Ok(());
        }
        let mut rx = self.0.subscribe();
        let mut rejected = None;
        let on_drop = ConsumeOnDrop::new(|| self.0.send_modify(|holder| holder.sender_slot = None));
        let result = rx
            .wait_for(|holder| holder.dropped || holder.sender_slot.is_none())
            .await;
        drop(result.unwrap());
        let _disarmed = ConsumeOnDrop::into_inner(on_drop);
        self.0.send_if_modified(|holder| {
            rejected = holder.sender_slot.take().map(SyncWrapper::into_inner);
            assert!(holder.dropped || rejected.is_none());
            rejected.is_some()
        });
        match rejected {
            Some(value) => Err(self::error::SendError(value)),
            None => Ok(()),
        }
    }

    fn try_send(&mut self, value: T) -> Result<(), self::error::TrySendError<T>> {
        let mut rejected = None;
        let mut dropped = false;
        self.0.send_if_modified(|holder| {
            assert!(holder.sender_slot.is_none());
            dropped = holder.dropped;
            let receiver_waiting = matches!(holder.receiver_slot, ReceiverSlot::ReceiverWaiting);
            if receiver_waiting {
                holder.receiver_slot = ReceiverSlot::Received(SyncWrapper::new(value));
            } else {
                rejected = Some(value);
            }
            receiver_waiting
        });
        match rejected {
            Some(value) => {
                if dropped {
                    Err(self::error::TrySendError::Closed(value))
                } else {
                    Err(self::error::TrySendError::NotWaiting(value))
                }
            }
            None => Ok(()),
        }
    }

    async fn recv(&mut self) -> Option<T> {
        let mut value = None;
        self.0.send_modify(|holder| {
            assert!(matches!(holder.receiver_slot, ReceiverSlot::NoReceiver));
            value = holder.sender_slot.take();
            if value.is_none() {
                holder.receiver_slot = ReceiverSlot::ReceiverWaiting;
            }
        });
        if let Some(value) = value {
            return Some(value.into_inner());
        }
        let mut rx = self.0.subscribe();
        let on_drop = ConsumeOnDrop::new(|| {
            self.0
                .send_modify(|holder| holder.receiver_slot = ReceiverSlot::NoReceiver);
        });
        let result = rx
            .wait_for(|holder| {
                let received = matches!(holder.receiver_slot, ReceiverSlot::Received(_));
                assert!(received || holder.sender_slot.is_none());
                holder.dropped || received
            })
            .await;
        drop(result.unwrap());
        let _disarmed = ConsumeOnDrop::into_inner(on_drop);
        let mut value = ReceiverSlot::NoReceiver;
        self.0
            .send_modify(|holder| mem::swap(&mut holder.receiver_slot, &mut value));
        match value {
            ReceiverSlot::Received(value) => Some(value.into_inner()),
            ReceiverSlot::ReceiverWaiting => None,
            ReceiverSlot::NoReceiver => {
                unreachable!("Received variant cannot be changed by sender")
            }
        }
    }

    fn try_recv(&mut self) -> Result<T, self::error::TryRecvError> {
        let mut value = None;
        let mut dropped = false;
        self.0.send_if_modified(|holder| {
            assert!(matches!(holder.receiver_slot, ReceiverSlot::NoReceiver));
            dropped = holder.dropped;
            value = holder.sender_slot.take();
            value.is_some()
        });
        match value {
            Some(value) => Ok(value.into_inner()),
            None => {
                if dropped {
                    Err(self::error::TryRecvError::Disconnected)
                } else {
                    Err(self::error::TryRecvError::Empty)
                }
            }
        }
    }
}

#[cfg(all(test, not(loom)))]
mod norm_tests;

#[cfg(all(test, loom))]
mod loom_tests;
