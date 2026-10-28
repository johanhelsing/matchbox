use futures::StreamExt;
use matchbox_signaling::SignalingServer;
use std::{net::Ipv4Addr, time::Duration};
use tokio::{sync::oneshot, time::timeout};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn shutdown_closes_connected_peers_and_stops_accepting() {
    let (shutdown, rx) = oneshot::channel();
    let mut server = SignalingServer::full_mesh_builder((Ipv4Addr::LOCALHOST, 0))
        .with_shutdown(rx)
        .build();
    let addr = server.bind().unwrap();
    let serve = tokio::spawn(server.serve());

    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/room"))
        .await
        .unwrap();
    // IdAssigned
    client.next().await.unwrap().unwrap();

    shutdown.send(()).unwrap();

    let close = timeout(Duration::from_secs(2), client.next())
        .await
        .expect("peer was not closed");
    assert!(matches!(close, Some(Ok(Message::Close(_)))), "{close:?}");

    timeout(Duration::from_secs(2), serve)
        .await
        .expect("serve did not return")
        .unwrap()
        .unwrap();

    assert!(
        tokio_tungstenite::connect_async(format!("ws://{addr}/room"))
            .await
            .is_err(),
        "still accepting connections"
    );
}
