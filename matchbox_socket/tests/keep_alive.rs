#![cfg(not(target_arch = "wasm32"))]

use async_tungstenite::tungstenite::{Error, Message, WebSocket, accept};
use bytes::Bytes;
use matchbox_socket::{WebRtcSocket, WebRtcSocketBuilder};
use std::{
    io::ErrorKind,
    net::{Ipv4Addr, TcpStream},
    time::Duration,
};
use tokio::{net::TcpListener, task::spawn_blocking, time::timeout};

const INTERVAL: Duration = Duration::from_millis(50);
const TIMEOUT: Duration = Duration::from_secs(5);

async fn with_websocket(
    configure: impl FnOnce(WebRtcSocketBuilder) -> WebRtcSocketBuilder,
    check: impl FnOnce(&mut WebSocket<TcpStream>) + Send + 'static,
) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let builder = WebRtcSocket::builder(format!("ws://{}/room", listener.local_addr().unwrap()))
        .reconnect_attempts(Some(1))
        .add_reliable_channel();
    let (_socket, message_loop) = configure(builder).build();
    tokio::spawn(message_loop);

    let (stream, _) = timeout(TIMEOUT, listener.accept())
        .await
        .expect("signaling connection timed out")
        .unwrap();
    let stream = stream.into_std().unwrap();
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(TIMEOUT)).unwrap();

    spawn_blocking(move || {
        let mut websocket = accept(stream).expect("WebSocket handshake failed");
        check(&mut websocket);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn native_keep_alive_sends_periodic_pings() {
    with_websocket(
        |builder| builder.signaling_keep_alive_interval(Some(INTERVAL)),
        |websocket| {
            for _ in 0..2 {
                assert_eq!(websocket.read().unwrap(), Message::Ping(Bytes::new()));
            }
        },
    )
    .await;
}

#[tokio::test]
async fn disabled_keep_alive_still_replies_to_pings() {
    with_websocket(
        |builder| {
            builder
                .signaling_keep_alive_interval(Some(INTERVAL))
                .signaling_keep_alive_interval(None)
        },
        |websocket| {
            let payload = Bytes::from_static(b"keepalive");
            websocket.send(Message::Ping(payload.clone())).unwrap();
            assert_eq!(websocket.read().unwrap(), Message::Pong(payload));

            websocket
                .get_ref()
                .set_read_timeout(Some(INTERVAL * 3))
                .unwrap();
            let result = websocket.read();
            assert!(
                matches!(&result, Err(Error::Io(err)) if matches!(
                    err.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut
                )),
                "expected no frames with keepalive disabled, got {result:?}"
            );
        },
    )
    .await;
}
