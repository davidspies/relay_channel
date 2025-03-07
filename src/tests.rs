use std::task::Poll;

use futures::poll;
use tokio::{join, pin, time::Duration};

use crate::{channel, error};

#[tokio::test]
async fn send_recv() {
    let (mut tx, mut rx) = channel();
    let (sent, received) = join! {
        tx.send(42),
        rx.recv(),
    };
    assert_eq!(sent, Ok(()));
    assert_eq!(received, Some(42));
}

#[tokio::test]
async fn failed_try_recv() {
    let (_tx, mut rx) = channel::<i32>();
    let received = rx.try_recv();
    assert_eq!(received, Err(error::TryRecvError::Empty));
}

#[tokio::test]
async fn successful_try_recv() {
    let (mut tx, mut rx) = channel();
    let (sent, received) = join! {
        tx.send(42),
        async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            rx.try_recv()
        }
    };
    assert_eq!(sent, Ok(()));
    assert_eq!(received, Ok(42));
}

#[tokio::test]
async fn dropped_sender() {
    let (tx, mut rx) = channel::<i32>();
    let recv_fut = rx.recv();
    pin!(recv_fut);
    drop(tx);
    let received = poll!(recv_fut);
    assert_eq!(received, Poll::Ready(None));
}

#[tokio::test]
async fn dropped_receiver() {
    let (mut tx, rx) = channel();
    drop(rx);
    let sent = tx.send(42).await;
    assert_eq!(sent, Err(error::SendError(42)));
}

#[tokio::test]
async fn dropped_channel_try_recv() {
    let (tx, mut rx) = channel::<i32>();
    drop(tx);
    let received = rx.try_recv();
    assert_eq!(received, Err(error::TryRecvError::Disconnected));
}

#[tokio::test]
async fn aborted_send() {
    let (mut tx, mut rx) = channel();
    let sent = tokio::time::timeout(Duration::from_millis(100), tx.send(42)).await;
    assert!(sent.is_err());
    let received = rx.try_recv();
    assert_eq!(received, Err(error::TryRecvError::Empty));
}

#[tokio::test]
async fn send_non_sync() {
    let (mut tx, mut rx) = channel();
    let sent = tokio::spawn(async move { tx.send(std::cell::Cell::new(42)).await });
    let received = rx.recv().await;
    assert_eq!(sent.await.unwrap(), Ok(()));
    assert_eq!(received.unwrap().get(), 42);
}
