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
    let recv_fut = tokio::task::unconstrained(rx.recv());
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
async fn aborted_recv() {
    let (mut tx, mut rx) = channel::<i32>();
    {
        let recv_future = tokio::task::unconstrained(rx.recv());
        pin!(recv_future);

        // Poll the future once to ensure the waker is registered
        assert_eq!(poll!(&mut recv_future), Poll::Pending);

        // Abort the receive operation
        let received_result = tokio::time::timeout(Duration::from_millis(10), recv_future).await;
        assert!(received_result.is_err()); // Indicates timeout, i.e., aborted recv
    }

    // After the receiver is aborted (dropped), sending should fail
    let sent = tx.try_send(42);
    assert_eq!(sent, Err(error::TrySendError::NotWaiting(42)));
}

#[tokio::test]
async fn send_non_sync() {
    let (mut tx, mut rx) = channel();
    // Spawn the send operation in a new task
    let send_task = tokio::spawn(async move { tx.send(std::cell::Cell::new(42)).await });
    // Receive the value in the current task
    let received_value = rx.recv().await;
    // Wait for the send task to complete and check its result
    assert_eq!(send_task.await.unwrap(), Ok(()));
    // Check the received value
    assert_eq!(received_value.unwrap().get(), 42);
}

#[tokio::test]
async fn try_send_receiver_waiting_polled() {
    let (mut tx, mut rx) = channel::<i32>();
    let recv_fut = tokio::task::unconstrained(rx.recv());
    pin!(recv_fut);

    // Poll recv_fut so its waker is registered with the sender side
    assert_eq!(poll!(&mut recv_fut), Poll::Pending);

    // Now that receiver is confirmed to be waiting, try_send should succeed
    let result = tx.try_send(100);
    assert_eq!(result, Ok(()));

    // The receiver should now get the value
    assert_eq!(poll!(&mut recv_fut), Poll::Ready(Some(100)));
}

#[tokio::test]
async fn try_send_receiver_not_polled_yet() {
    let (mut tx, _rx) = channel::<i32>(); // _rx to keep the channel open

    // Receiver exists but recv() hasn't been called or polled
    // so it's not "waiting" in a way try_send can immediately satisfy.
    let result = tx.try_send(100);
    assert_eq!(result, Err(error::TrySendError::NotWaiting(100)));
}

#[tokio::test]
async fn try_send_receiver_exists_but_recv_not_called() {
    let (mut tx, _rx) = channel::<i32>(); // Receiver exists but is not actively receiving

    // try_send should fail with NotWaiting because no recv() call is active
    let result = tx.try_send(100);
    assert_eq!(result, Err(error::TrySendError::NotWaiting(100)));
}

#[tokio::test]
async fn try_send_receiver_dropped() {
    let (mut tx, rx) = channel::<i32>();
    drop(rx); // Drop the receiver

    // try_send should fail with Closed because the receiver is gone
    let result = tx.try_send(100);
    assert_eq!(result, Err(error::TrySendError::Closed(100)));
}

#[tokio::test]
async fn try_send_successful_then_recv_when_receiver_is_waiting() {
    // This test ensures that if a receiver is actively waiting (polled),
    // try_send can successfully send a value to it.
    let (mut tx, mut rx) = channel::<i32>();
    let recv_fut = tokio::task::unconstrained(rx.recv()); // Receiver initiates recv
    pin!(recv_fut);

    // Poll the receiver's future once. This signifies the receiver is now actively
    // waiting and has registered its waker with the channel.
    assert_eq!(poll!(&mut recv_fut), Poll::Pending);

    // Now that the receiver is confirmed to be waiting, try_send should succeed.
    let send_result = tx.try_send(200);
    assert_eq!(send_result, Ok(()));

    // The receiver should now be able to poll the value.
    // Since try_send is synchronous and the value is now in the channel,
    // the next poll on recv_fut should resolve to Ready.
    assert_eq!(poll!(&mut recv_fut), Poll::Ready(Some(200)));

    // Or, alternatively, await the future to confirm it resolves correctly.
    // let received_val = recv_fut.await;
    // assert_eq!(received_val, Some(200));
}

#[tokio::test]
async fn try_send_value_propagates_in_error() {
    let (mut tx, _rx) = channel::<i32>();
    let value_to_send = 123;
    // Receiver not waiting
    match tx.try_send(value_to_send) {
        Err(error::TrySendError::NotWaiting(val)) => assert_eq!(val, value_to_send),
        other => panic!("Expected NotWaiting, got {:?}", other),
    }

    drop(_rx); // Drop the receiver
    match tx.try_send(value_to_send) {
        Err(error::TrySendError::Closed(val)) => assert_eq!(val, value_to_send),
        other => panic!("Expected Closed, got {:?}", other),
    }
}
