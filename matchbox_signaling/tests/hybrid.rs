#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use matchbox_protocol::{JsonPeerEvent, PeerEvent, PeerId}; 
    use matchbox_signaling::SignalingServer;
    use std::{net::Ipv4Addr, str::FromStr};
    use tokio::net::TcpStream;
    use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
    use tracing::info;

    // Helper to take the next PeerEvent from a stream
    async fn recv_peer_event(
        client: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    ) -> JsonPeerEvent {
        let message: Message = client.next().await.unwrap().unwrap();
        JsonPeerEvent::from_str(&message.to_string()).expect("json peer event")
    }

    // Helper to extract PeerId when expecting an Id assignment
    fn get_peer_id(peer_event: JsonPeerEvent) -> PeerId {
        if let JsonPeerEvent::IdAssigned(id) = peer_event {
            id
        } else {
            panic!("Peer_event was not IdAssigned: {peer_event:?}");
        }
    }

    #[tokio::test]
    async fn ws_connect() {
        let mut server = SignalingServer::hybrid_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .expect("handshake");
    }



    #[tokio::test]
    async fn uuid_assigned() {
        let mut server = SignalingServer::hybrid_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut super_peer, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();

        let id_assigned_event = recv_peer_event(&mut super_peer).await;

        assert!(matches!(id_assigned_event, JsonPeerEvent::IdAssigned(..)));
    }

    #[tokio::test]
    async fn multiple_super_peers() {
        let mut server = SignalingServer::hybrid_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut super_peer_a, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let _a_uuid = get_peer_id(recv_peer_event(&mut super_peer_a).await);

        let (mut super_peer_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();
        let b_uuid = get_peer_id(recv_peer_event(&mut super_peer_b).await);

        let new_peer_event = recv_peer_event(&mut super_peer_a).await;

        assert_eq!(new_peer_event, JsonPeerEvent::NewPeer(b_uuid));
    }

    #[tokio::test]
    async fn connect_child_peer() {
        let mut server = SignalingServer::hybrid_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut super_peer_a, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let super_peer_a_uuid = get_peer_id(recv_peer_event(&mut super_peer_a).await);

        let (mut super_peer_b, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let super_peer_b_uuid = get_peer_id(recv_peer_event(&mut super_peer_b).await);

        let new_peer_event = recv_peer_event(&mut super_peer_a).await;

        info!("Super peer a got: {new_peer_event}");

        let (mut child_a, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let child_a_uuid = get_peer_id(recv_peer_event(&mut child_a).await);

        let new_peer_event: PeerEvent<serde_json::Value>;

        if super_peer_a_uuid < super_peer_b_uuid {
            new_peer_event = recv_peer_event(&mut super_peer_a).await;
        }
        else {
            new_peer_event = recv_peer_event(&mut super_peer_b).await;
        }

        assert_eq!(new_peer_event, JsonPeerEvent::NewPeer(child_a_uuid));

    
        let (mut child_b, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let child_b_uuid = get_peer_id(recv_peer_event(&mut child_b).await);

        let new_peer_event:PeerEvent<serde_json::Value>;

        if super_peer_a_uuid < super_peer_b_uuid {
            new_peer_event = recv_peer_event(&mut super_peer_b).await;
        }
        else {
            new_peer_event = recv_peer_event(&mut super_peer_a).await;
        }

        assert_eq!(new_peer_event, JsonPeerEvent::NewPeer(child_b_uuid));

    }

    #[tokio::test]
    async fn disconnect_child_peer() {
        let mut server = SignalingServer::hybrid_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut super_peer_a, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let super_peer_a_uuid = get_peer_id(recv_peer_event(&mut super_peer_a).await);

        let (mut super_peer_b, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let super_peer_b_uuid = get_peer_id(recv_peer_event(&mut super_peer_b).await);

        recv_peer_event(&mut super_peer_a).await;

        let (mut child_a, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
        .await
        .unwrap();
        let child_a_uuid = get_peer_id(recv_peer_event(&mut child_a).await);

        let new_peer_event: PeerEvent<serde_json::Value>;

        if super_peer_a_uuid < super_peer_b_uuid {
            new_peer_event = recv_peer_event(&mut super_peer_a).await;
        }
        else {
            new_peer_event = recv_peer_event(&mut super_peer_b).await;
        }

        assert_eq!(new_peer_event, JsonPeerEvent::NewPeer(child_a_uuid));

        _ = child_a.close(None).await;
        let peer_left_event: PeerEvent<serde_json::Value>;

        if super_peer_a_uuid < super_peer_b_uuid {
            peer_left_event = recv_peer_event(&mut super_peer_a).await;
        }
        else {
            peer_left_event = recv_peer_event(&mut super_peer_b).await;
        }

        assert_eq!(peer_left_event, JsonPeerEvent::PeerLeft(child_a_uuid));

    }

   #[tokio::test]
    async fn disconnect_super_peer_no_children() {
        let mut server = SignalingServer::hybrid_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut super_peer_a, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let super_peer_a_uuid = get_peer_id(recv_peer_event(&mut super_peer_a).await);

        let (mut super_peer_b, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let _super_peer_b_uuid = get_peer_id(recv_peer_event(&mut super_peer_b).await);

        recv_peer_event(&mut super_peer_a).await;

        _ = super_peer_a.close(None).await;
        let peer_left_event = recv_peer_event(&mut super_peer_b).await;

        assert_eq!(peer_left_event, JsonPeerEvent::PeerLeft(super_peer_a_uuid));

    }

    #[tokio::test]
    async fn disconnect_super_peer_with_children() {
        let mut server = SignalingServer::hybrid_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut super_peer_a, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let super_peer_a_uuid = get_peer_id(recv_peer_event(&mut super_peer_a).await);

        let (mut super_peer_b, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let super_peer_b_uuid = get_peer_id(recv_peer_event(&mut super_peer_b).await);

        recv_peer_event(&mut super_peer_a).await;

        let (mut child_a, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
        .await
        .unwrap();
        let child_a_uuid = get_peer_id(recv_peer_event(&mut child_a).await);

        if super_peer_a_uuid < super_peer_b_uuid {
            recv_peer_event(&mut super_peer_a).await;
            _ = super_peer_a.close(None).await;
            assert_eq!(recv_peer_event(&mut super_peer_b).await, JsonPeerEvent::PeerLeft(super_peer_a_uuid));
            assert_eq!(recv_peer_event(&mut child_a).await, JsonPeerEvent::PeerLeft(super_peer_a_uuid));
            assert_eq!(recv_peer_event(&mut super_peer_b).await, JsonPeerEvent::NewPeer(child_a_uuid));

        }
        else {
            recv_peer_event(&mut super_peer_b).await;
            _ = super_peer_b.close(None).await;
            assert_eq!(recv_peer_event(&mut super_peer_a).await, JsonPeerEvent::PeerLeft(super_peer_b_uuid));
            assert_eq!(recv_peer_event(&mut child_a).await, JsonPeerEvent::PeerLeft(super_peer_b_uuid));
            assert_eq!(recv_peer_event(&mut super_peer_a).await, JsonPeerEvent::NewPeer(child_a_uuid));
        }
    }

    #[tokio::test] 
    async fn reassigning_children() {
        let mut server = SignalingServer::hybrid_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut super_peer_a, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let super_peer_a_uuid = get_peer_id(recv_peer_event(&mut super_peer_a).await);

        let (mut super_peer_b, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let super_peer_b_uuid = get_peer_id(recv_peer_event(&mut super_peer_b).await);

        recv_peer_event(&mut super_peer_a).await;

        let (mut child_a, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
        .await
        .unwrap();
        let child_a_uuid = get_peer_id(recv_peer_event(&mut child_a).await);

        if super_peer_a_uuid < super_peer_b_uuid {
            assert_eq!(recv_peer_event(&mut super_peer_a).await, JsonPeerEvent::NewPeer(child_a_uuid));

            let (mut child_b, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
            let child_b_uuid = get_peer_id(recv_peer_event(&mut child_b).await);
            assert_eq!(recv_peer_event(&mut super_peer_b).await, JsonPeerEvent::NewPeer(child_b_uuid));

            let (mut child_c, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
            let child_c_uuid = get_peer_id(recv_peer_event(&mut child_c).await);
            assert_eq!(recv_peer_event(&mut super_peer_a).await, JsonPeerEvent::NewPeer(child_c_uuid));

            _ = super_peer_a.close(None).await;

            if child_a_uuid < child_c_uuid {
                assert_eq!(recv_peer_event(&mut child_a).await, JsonPeerEvent::PeerLeft(super_peer_a_uuid));
                assert_eq!(recv_peer_event(&mut child_c).await, JsonPeerEvent::PeerLeft(super_peer_a_uuid));
                assert_eq!(recv_peer_event(&mut super_peer_b).await, JsonPeerEvent::PeerLeft(super_peer_a_uuid));
                assert_eq!(recv_peer_event(&mut super_peer_b).await, JsonPeerEvent::NewPeer(child_a_uuid));
                assert_eq!(recv_peer_event(&mut child_a).await, JsonPeerEvent::NewPeer(child_c_uuid));
            }
            else{
                assert_eq!(recv_peer_event(&mut child_a).await, JsonPeerEvent::PeerLeft(super_peer_a_uuid));
                assert_eq!(recv_peer_event(&mut child_c).await, JsonPeerEvent::PeerLeft(super_peer_a_uuid));
                assert_eq!(recv_peer_event(&mut super_peer_b).await, JsonPeerEvent::PeerLeft(super_peer_a_uuid));
                assert_eq!(recv_peer_event(&mut super_peer_b).await, JsonPeerEvent::NewPeer(child_c_uuid));
                assert_eq!(recv_peer_event(&mut child_c).await, JsonPeerEvent::NewPeer(child_a_uuid));
            }
        }
        else {
            assert_eq!(recv_peer_event(&mut super_peer_b).await, JsonPeerEvent::NewPeer(child_a_uuid));

            let (mut child_b, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
            let child_b_uuid = get_peer_id(recv_peer_event(&mut child_b).await);
            assert_eq!(recv_peer_event(&mut super_peer_a).await, JsonPeerEvent::NewPeer(child_b_uuid));

            let (mut child_c, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
            let child_c_uuid = get_peer_id(recv_peer_event(&mut child_c).await);
            assert_eq!(recv_peer_event(&mut super_peer_b).await, JsonPeerEvent::NewPeer(child_c_uuid));

            _ = super_peer_b.close(None).await;

            if child_a_uuid < child_c_uuid {
                assert_eq!(recv_peer_event(&mut child_a).await, JsonPeerEvent::PeerLeft(super_peer_b_uuid));
                assert_eq!(recv_peer_event(&mut child_c).await, JsonPeerEvent::PeerLeft(super_peer_b_uuid));
                assert_eq!(recv_peer_event(&mut super_peer_a).await, JsonPeerEvent::PeerLeft(super_peer_b_uuid));
                assert_eq!(recv_peer_event(&mut super_peer_a).await, JsonPeerEvent::NewPeer(child_a_uuid));
                assert_eq!(recv_peer_event(&mut child_a).await, JsonPeerEvent::NewPeer(child_c_uuid));
            }
            else {
                assert_eq!(recv_peer_event(&mut child_a).await, JsonPeerEvent::PeerLeft(super_peer_b_uuid));
                assert_eq!(recv_peer_event(&mut child_c).await, JsonPeerEvent::PeerLeft(super_peer_b_uuid));
                assert_eq!(recv_peer_event(&mut super_peer_a).await, JsonPeerEvent::PeerLeft(super_peer_b_uuid));
                assert_eq!(recv_peer_event(&mut super_peer_a).await, JsonPeerEvent::NewPeer(child_c_uuid));
                assert_eq!(recv_peer_event(&mut child_c).await, JsonPeerEvent::NewPeer(child_a_uuid));
            }  
        }
    }
}
