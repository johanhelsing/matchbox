#[cfg(test)]
mod tests {
    use futures::{SinkExt, StreamExt};
    use matchbox_protocol::{JsonPeerEvent, PeerId};
    use matchbox_signalling_socket::SignalingServer;
    use std::{net::Ipv4Addr, str::FromStr, sync::atomic::AtomicBool};
    use tokio::net::TcpStream;
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
    #[should_panic]
    async fn ws_on_connect_callback() {
        let success = std::sync::Arc::new(AtomicBool::new(false));

        let server = SignalingServer::full_mesh_builder((Ipv4Addr::UNSPECIFIED, 0))
            .on_peer_connected(|| async { panic!("This should panic") })
            .build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        tokio_tungstenite::connect_async(format!("ws://{}/room_a", addr))
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn ws_connect() {
        let server = SignalingServer::full_mesh_builder((Ipv4Addr::UNSPECIFIED, 0)).build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        tokio_tungstenite::connect_async(format!("ws://{}/room_a", addr))
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn uuid_assigned() {
        let server = SignalingServer::full_mesh_builder((Ipv4Addr::UNSPECIFIED, 0)).build();
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
        let server = SignalingServer::full_mesh_builder((Ipv4Addr::UNSPECIFIED, 0)).build();
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
        let server = SignalingServer::full_mesh_builder((Ipv4Addr::UNSPECIFIED, 0)).build();
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
        let server = SignalingServer::full_mesh_builder((Ipv4Addr::UNSPECIFIED, 0)).build();
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
}
