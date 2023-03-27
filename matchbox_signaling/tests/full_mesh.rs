#[cfg(test)]
mod tests {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use futures::{pin_mut, FutureExt, SinkExt, StreamExt};
    use futures_timer::Delay;
    use hyper::Uri;
    use matchbox_protocol::{JsonPeerEvent, PeerId};
    use matchbox_signaling::SignalingServer;
    use std::{
        net::Ipv4Addr,
        str::FromStr,
        sync::{atomic::AtomicBool, Arc},
    };
    use tokio::{net::TcpStream, select, time::Duration};
    use tokio_tungstenite::{
        tungstenite::{
            handshake::client::{generate_key, Request},
            Message,
        },
        MaybeTlsStream, WebSocketStream,
    };

    async fn wait_for_success(success: Arc<AtomicBool>) {
        // Give adaquete time for the callback to trigger
        let fetch = Delay::new(Duration::from_millis(1)).fuse();
        let timeout = Delay::new(Duration::from_millis(100)).fuse();
        pin_mut!(timeout, fetch);
        select! {
            _ = &mut fetch => {
                if !success.load(std::sync::atomic::Ordering::Acquire) {
                    // Reset the clock
                    fetch.set(Delay::new(Duration::from_millis(1)).fuse());
                }
            }
            _ = timeout => panic!("timeout")
        };
        assert!(success.load(std::sync::atomic::Ordering::Acquire))
    }

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
        let server = SignalingServer::full_mesh_builder((Ipv4Addr::UNSPECIFIED, 0)).build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        tokio_tungstenite::connect_async(format!("ws://{}/room_a", addr))
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn ws_connect_auth() {
        let username = "test";
        let password = "123";

        let server = SignalingServer::full_mesh_builder((Ipv4Addr::UNSPECIFIED, 0))
            .basic_auth(username, password)
            .build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        let auth = STANDARD.encode(format!("{username}:{password}"));
        let uri = format!("ws://{addr}/room_a").parse::<Uri>().unwrap();
        let authority = uri.authority().unwrap().as_str();
        let host = authority
            .find('@')
            .map(|idx| authority.split_at(idx + 1).1)
            .unwrap_or_else(|| authority);
        let request = Request::builder()
            .uri(format!("ws://{addr}/room_a"))
            .method("GET")
            .header("Host", host)
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", generate_key())
            .header("Authorization", format!("Basic {auth}"))
            .body(())
            .unwrap();

        tokio_tungstenite::connect_async(request)
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn ws_connect_invalid_auth() {
        let username = "test";
        let password = "123";
        let invalid_password = "321";

        let server = SignalingServer::full_mesh_builder((Ipv4Addr::UNSPECIFIED, 0))
            .basic_auth(username, password)
            .build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        let auth = STANDARD.encode(format!("{username}:{invalid_password}"));
        let uri = format!("ws://{addr}/room_a").parse::<Uri>().unwrap();
        let authority = uri.authority().unwrap().as_str();
        let host = authority
            .find('@')
            .map(|idx| authority.split_at(idx + 1).1)
            .unwrap_or_else(|| authority);
        let request = Request::builder()
            .uri(format!("ws://{addr}/room_a"))
            .method("GET")
            .header("Host", host)
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", generate_key())
            .header("Authorization", format!("Basic {auth}"))
            .body(())
            .unwrap();
        let resp = tokio_tungstenite::connect_async(request).await;

        assert!(resp.is_err());
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

    #[tokio::test]
    async fn on_connection_req_callback() {
        let success = Arc::new(AtomicBool::new(false));

        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0))
            .on_connection_request({
                let success = success.clone();
                move |_| {
                    success.store(true, std::sync::atomic::Ordering::Release);
                    Ok(true)
                }
            })
            .build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        // Connect
        _ = tokio_tungstenite::connect_async(format!("ws://{}/room_a", addr)).await;

        wait_for_success(success).await
    }

    #[tokio::test]
    async fn deny_on_connection_req_callback() {
        let upgrade_called = Arc::new(AtomicBool::new(false));
        let peer_connected = Arc::new(AtomicBool::new(false));

        let server = SignalingServer::full_mesh_builder((Ipv4Addr::UNSPECIFIED, 0))
            .on_connection_request({
                let upgrade_called = upgrade_called.clone();
                move |_| {
                    upgrade_called.store(true, std::sync::atomic::Ordering::Release);
                    Ok(false) // <-- Deny access!
                }
            })
            .on_peer_connected({
                // This should not get called because we deny all connections on upgrade
                let peer_connected = peer_connected.clone();
                move |_| peer_connected.store(true, std::sync::atomic::Ordering::Release)
            })
            .build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        // Connect
        _ = tokio_tungstenite::connect_async(format!("ws://{}/room_a", addr)).await;

        wait_for_success(upgrade_called).await;
        assert!(!peer_connected.load(std::sync::atomic::Ordering::Acquire))
    }

    #[tokio::test]
    async fn on_id_assignment_callback() {
        let success = Arc::new(AtomicBool::new(false));

        let server = SignalingServer::full_mesh_builder((Ipv4Addr::UNSPECIFIED, 0))
            .on_id_assignment({
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

        wait_for_success(success).await
    }

    #[tokio::test]
    async fn on_connect_callback() {
        let success = Arc::new(AtomicBool::new(false));

        let server = SignalingServer::full_mesh_builder((Ipv4Addr::UNSPECIFIED, 0))
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

        wait_for_success(success).await
    }

    #[tokio::test]
    async fn on_disconnect_callback() {
        let success = Arc::new(AtomicBool::new(false));

        let server = SignalingServer::full_mesh_builder((Ipv4Addr::UNSPECIFIED, 0))
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

        wait_for_success(success).await
    }
}
