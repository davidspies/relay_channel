//! To run these tests, switch to the commented-out tokio dependency in Cargo.toml.
//! Then run:
//!
//! `LOOM_MAX_PREEMPTIONS=2 RUSTFLAGS="--cfg loom" cargo test --tests --release -- --nocapture`

#[test]
fn parallel_send_recv() {
    loom::model(|| {
        let (mut tx, mut rx) = crate::channel();

        let sender = loom::thread::spawn(move || loom::future::block_on(tx.send(Box::new(42))));
        let receiver = loom::thread::spawn(move || loom::future::block_on(rx.recv()));

        let sent = sender.join().unwrap();
        let received = receiver.join().unwrap();

        assert_eq!(sent, Ok(()));
        assert_eq!(received, Some(Box::new(42)));
    })
}

#[test]
fn concurrent_send_recv() {
    loom::model(|| {
        loom::future::block_on(async {
            let (mut tx, mut rx) = crate::channel();

            let send_fut = tx.send(Box::new(42));
            let recv_fut = rx.recv();

            let (sent, received) = tokio::join!(send_fut, recv_fut);

            assert_eq!(sent, Ok(()));
            assert_eq!(received, Some(Box::new(42)));
        })
    })
}

#[test]
fn parallel_send_recv_drop() {
    loom::model(|| {
        let (mut tx, mut rx) = crate::channel();

        let sender = loom::thread::spawn(move || {
            loom::future::block_on(async {
                tx.send(Box::new(42)).await.unwrap();
                tx.send(Box::new(43)).await.unwrap();
                drop(tx);
            })
        });
        let receiver = loom::thread::spawn(move || {
            loom::future::block_on(async {
                let received = rx.recv().await;
                assert_eq!(received, Some(Box::new(42)));
                let received = rx.recv().await;
                assert_eq!(received, Some(Box::new(43)));
                let received = rx.recv().await;
                assert_eq!(received, None);
            })
        });

        sender.join().unwrap();
        receiver.join().unwrap();
    })
}

#[test]
fn send_cancellation() {
    loom::model(|| {
        let (mut tx, mut rx) = crate::channel();
        let (cancel_handle, cancelled) = tokio::sync::oneshot::channel();

        let sender = loom::thread::spawn(move || {
            loom::future::block_on(async {
                tokio::select! {
                    res = tx.send(Box::new(42)) => {
                        assert_eq!(res, Ok(()));
                        42
                    }
                    res = cancelled => {
                        assert_eq!(res, Ok(()));
                        if tx.send(Box::new(43)).await.is_err() {
                            42
                        } else {
                            43
                        }
                    }
                }
            })
        });
        let receiver = loom::thread::spawn(move || loom::future::block_on(rx.recv()));
        let canceller = loom::thread::spawn(move || cancel_handle.send(()));

        let expected_received = sender.join().unwrap();
        let actual_received = receiver.join().unwrap();
        let did_cancel = canceller.join().unwrap();

        assert_eq!(actual_received, Some(Box::new(expected_received)));
        if did_cancel.is_err() {
            assert_eq!(actual_received, Some(Box::new(42)));
        }
    })
}

#[test]
fn recv_cancellation() {
    loom::model(|| {
        let (mut tx, mut rx) = crate::channel();
        let (cancel_handle, cancelled) = tokio::sync::oneshot::channel();

        let sender = loom::thread::spawn(move || loom::future::block_on(tx.send(Box::new(42))));
        let receiver = loom::thread::spawn(move || {
            loom::future::block_on(async {
                tokio::select! {
                    res = rx.recv() => res,
                    res = cancelled => {
                        assert_eq!(res, Ok(()));
                        rx.recv().await
                    }
                }
            })
        });
        let canceller = loom::thread::spawn(move || cancel_handle.send(()));

        let sent = sender.join().unwrap();
        let received = receiver.join().unwrap();
        let did_cancel = canceller.join().unwrap();

        assert_eq!(sent, Ok(()));
        if did_cancel.is_err() {
            assert_eq!(received, Some(Box::new(42)));
        }
    })
}
