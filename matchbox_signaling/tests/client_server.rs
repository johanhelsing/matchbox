#[cfg(test)]
mod tests {
    use futures::{pin_mut, FutureExt, SinkExt, StreamExt};
    use futures_timer::Delay;
    use matchbox_protocol::{JsonPeerEvent, PeerRequest};
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
    async fn ws_on_host_connect_callback() {
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

        // Give adequate time for the callback to trigger
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
    async fn ws_on_host_disconnect_callback() {
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

        let server = SignalingServer::full_mesh_builder((Ipv4Addr::UNSPECIFIED, 0))
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
