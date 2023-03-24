#[cfg(test)]
mod tests {
    use futures::{pin_mut, FutureExt, SinkExt, StreamExt};
    use futures_timer::Delay;
    use matchbox_protocol::{JsonPeerEvent, PeerId, PeerRequest};
    use matchbox_signaling::SignalingServer;
    use std::{
        net::Ipv4Addr,
        str::FromStr,
        sync::{atomic::AtomicBool, Arc},
    };
    use tokio::{net::TcpStream, select, time::Duration};
    use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

    // Helper to take the next PeerEvent from a stream
    async fn recv_peer_event(
        client: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    ) -> JsonPeerEvent {
        let message: Message = client
            .next()
            .await
            .expect("some message")
            .expect("socket message");
        JsonPeerEvent::from_str(&message.to_string()).expect("json peer event")
    }

    fn get_peer_id(peer_event: JsonPeerEvent) -> PeerId {
        if let JsonPeerEvent::IdAssigned(id) = peer_event {
            id
        } else {
            panic!("Peer_event was not IdAssigned: {peer_event:?}");
        }
    }

    #[tokio::test]
    async fn ws_connect() {
        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0)).build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        tokio_tungstenite::connect_async(format!("ws://{}/room_a", addr))
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn uuid_assigned() {
        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0)).build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        let (mut client, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let id_assigned_event = recv_peer_event(&mut client).await;

        assert!(matches!(id_assigned_event, JsonPeerEvent::IdAssigned(..)));
    }

    #[tokio::test]
    async fn new_peer() {
        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0)).build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let _a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        let new_peer_event = recv_peer_event(&mut client_a).await;

        assert_eq!(new_peer_event, JsonPeerEvent::NewPeer(b_uuid));
    }

    #[tokio::test]
    async fn disconnect_peer() {
        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0)).build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let _a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        // Ensure Peer B was received
        let new_peer_event = recv_peer_event(&mut client_a).await;
        assert_eq!(new_peer_event, JsonPeerEvent::NewPeer(b_uuid));

        // Disconnect Peer B
        _ = client_b.close(None).await;
        let peer_left_event = recv_peer_event(&mut client_a).await;

        assert_eq!(peer_left_event, JsonPeerEvent::PeerLeft(b_uuid));
    }

    #[tokio::test]
    async fn signal() {
        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0)).build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let _b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        let new_peer_event = recv_peer_event(&mut client_a).await;
        let peer_uuid = match new_peer_event {
            JsonPeerEvent::NewPeer(PeerId(peer_uuid)) => peer_uuid.to_string(),
            _ => panic!("unexpected event"),
        };

        _ = client_a
            .send(Message::text(format!(
                "{{\"Signal\": {{\"receiver\": \"{peer_uuid}\", \"data\": \"123\" }}}}"
            )))
            .await;

        let signal_event = recv_peer_event(&mut client_b).await;
        assert_eq!(
            signal_event,
            JsonPeerEvent::Signal {
                data: serde_json::Value::String("123".to_string()),
                sender: a_uuid,
            }
        );
    }

    #[tokio::test]
    async fn ws_on_connect_callback() {
        let success = Arc::new(AtomicBool::new(false));

        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0))
            .on_peer_connected({
                let success = success.clone();
                move |_| success.store(true, std::sync::atomic::Ordering::Release)
            })
            .build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        // Connect
        tokio_tungstenite::connect_async(format!("ws://{}/room_a", addr))
            .await
            .expect("handshake");

        // Give adaquete time for the callback to trigger
        let fetch = Delay::new(Duration::from_millis(1)).fuse();
        let timeout = Delay::new(Duration::from_millis(100)).fuse();
        pin_mut!(timeout, fetch);
        select! {
            _ = &mut fetch => {
                if !success.load(std::sync::atomic::Ordering::Acquire) {
                    fetch.set(Delay::new(Duration::from_millis(1)).fuse());
                }
            }
            _ = timeout => panic!("timeout")
        };
        assert!(success.load(std::sync::atomic::Ordering::Acquire))
    }

    #[tokio::test]
    async fn ws_on_disconnect_callback() {
        let success = Arc::new(AtomicBool::new(false));

        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0))
            .on_peer_disconnected({
                let success = success.clone();
                move |_| success.store(true, std::sync::atomic::Ordering::Release)
            })
            .build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        // Connect
        {
            tokio_tungstenite::connect_async(format!("ws://{}/room_a", addr))
                .await
                .expect("handshake");
        }
        // Disconnects due to scope drop

        // Give adaquete time for the callback to trigger
        let fetch = Delay::new(Duration::from_millis(1)).fuse();
        let timeout = Delay::new(Duration::from_millis(100)).fuse();
        pin_mut!(timeout, fetch);
        select! {
            _ = &mut fetch => {
                if !success.load(std::sync::atomic::Ordering::Acquire) {
                    fetch.set(Delay::new(Duration::from_millis(1)).fuse());
                }
            }
            _ = timeout => panic!("timeout")
        };
        assert!(success.load(std::sync::atomic::Ordering::Acquire))
    }

    #[tokio::test]
    async fn ws_on_signal_callback() {
        let success = Arc::new(AtomicBool::new(false));

        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0))
            .on_signal({
                let success = success.clone();
                move |_| success.store(true, std::sync::atomic::Ordering::Release)
            })
            .build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        // Connect
        let (mut stream, _) = tokio_tungstenite::connect_async(format!("ws://{}/room_a", addr))
            .await
            .expect("handshake");

        let request = PeerRequest::KeepAlive.to_string();
        stream.send(Message::Text(request)).await.unwrap();

        // Give adaquete time for the callback to trigger
        let fetch = Delay::new(Duration::from_millis(1)).fuse();
        let timeout = Delay::new(Duration::from_millis(100)).fuse();
        pin_mut!(timeout, fetch);
        select! {
            _ = &mut fetch => {
                if !success.load(std::sync::atomic::Ordering::Acquire) {
                    fetch.set(Delay::new(Duration::from_millis(1)).fuse());
                }
            }
            _ = timeout => panic!("timeout")
        };
        assert!(success.load(std::sync::atomic::Ordering::Acquire))
    }
}
