#[cfg(test)]
mod tests {
    use futures::{pin_mut, FutureExt, StreamExt};
    use futures_timer::Delay;
    use matchbox_protocol::JsonPeerEvent;
    use matchbox_signaling::SignalingServer;
    use std::{
        net::Ipv4Addr,
        str::FromStr,
        sync::{atomic::AtomicBool, Arc},
    };
    use tokio::{net::TcpStream, select, time::Duration};
    use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

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

        let (mut host, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();

        let id_assigned_event = recv_peer_event(&mut host).await;

        assert!(matches!(id_assigned_event, JsonPeerEvent::IdAssigned(..)));
    }

    #[tokio::test]
    async fn on_upgrade_callback() {
        let success = Arc::new(AtomicBool::new(false));

        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0))
            .on_upgrade({
                let success = success.clone();
                move |_| {
                    success.store(true, std::sync::atomic::Ordering::Release);
                    true
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
    async fn deny_on_upgrade_callback() {
        let upgrade_called = Arc::new(AtomicBool::new(false));
        let peer_connected = Arc::new(AtomicBool::new(false));

        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0))
            .on_upgrade({
                let upgrade_called = upgrade_called.clone();
                move |_| {
                    upgrade_called.store(true, std::sync::atomic::Ordering::Release);
                    false // <-- Deny access!
                }
            })
            .on_host_connected({
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
    async fn on_host_connect_callback() {
        let success = Arc::new(AtomicBool::new(false));

        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0))
            .on_host_connected({
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
    async fn on_host_disconnect_callback() {
        let success = Arc::new(AtomicBool::new(false));

        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0))
            .on_host_disconnected({
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

    #[tokio::test]
    async fn on_client_connect_callback() {
        let success = Arc::new(AtomicBool::new(false));

        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0))
            .on_client_connected({
                let success = success.clone();
                move |_| success.store(true, std::sync::atomic::Ordering::Release)
            })
            .build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        // First Connect = Host
        let (mut _host, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        // Additional Connects = Client
        let (mut _client, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        wait_for_success(success).await
    }

    #[tokio::test]
    async fn on_client_disconnect_callback() {
        let success = Arc::new(AtomicBool::new(false));

        let server = SignalingServer::client_server_builder((Ipv4Addr::UNSPECIFIED, 0))
            .on_client_disconnected({
                let success = success.clone();
                move |_| success.store(true, std::sync::atomic::Ordering::Release)
            })
            .build();
        let addr = server.local_addr();
        tokio::spawn(server.serve());

        // Connect Host
        let (mut _host, _response) =
            tokio_tungstenite::connect_async(format!("ws://{}/room_a", addr))
                .await
                .expect("handshake");

        // Connect Client
        {
            tokio_tungstenite::connect_async(format!("ws://{}/room_a", addr))
                .await
                .expect("handshake");
        }
        // Client disconnects due to scope drop, host remains active

        wait_for_success(success).await
    }
}
