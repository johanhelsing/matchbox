#[cfg(test)]
mod tests {
    use futures::{SinkExt, StreamExt};
    use matchbox_protocol::{JsonPeerEvent, PeerId};
    use matchbox_signaling::SignalingServer;
    use std::{net::Ipv4Addr, str::FromStr};
    use tokio::{
        net::TcpStream,
        sync::mpsc::{error::TryRecvError, unbounded_channel},
    };
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

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
        let mut server = SignalingServer::client_server_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn uuid_assigned() {
        let mut server = SignalingServer::client_server_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut host, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();

        let id_assigned_event = recv_peer_event(&mut host).await;

        assert!(matches!(id_assigned_event, JsonPeerEvent::IdAssigned(..)));
    }

    #[tokio::test]
    async fn new_client() {
        let mut server = SignalingServer::client_server_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut host, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let _host_uuid = get_peer_id(recv_peer_event(&mut host).await);

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();
        let a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let new_peer_event = recv_peer_event(&mut host).await;

        assert_eq!(new_peer_event, JsonPeerEvent::NewPeer(a_uuid));
    }

    #[tokio::test]
    async fn disconnect_client() {
        let mut server = SignalingServer::client_server_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut host, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let _host_uuid = get_peer_id(recv_peer_event(&mut host).await);

        // Connect
        let a_uuid = {
            let (mut client_a, _response) =
                tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                    .await
                    .unwrap();
            let a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);
            let _new_peer_event = recv_peer_event(&mut host).await;
            a_uuid
        };
        // Disconnects due to scope drop

        let disconnect_event = recv_peer_event(&mut host).await;

        assert_eq!(disconnect_event, JsonPeerEvent::PeerLeft(a_uuid));
    }

    #[tokio::test]
    async fn signal() {
        let mut server = SignalingServer::client_server_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut host, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .unwrap();
        let _host_uuid = get_peer_id(recv_peer_event(&mut host).await);

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();
        let a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let new_peer_event = recv_peer_event(&mut host).await;
        let peer_uuid = match new_peer_event {
            JsonPeerEvent::NewPeer(PeerId(peer_uuid)) => peer_uuid.to_string(),
            _ => panic!("unexpected event"),
        };

        _ = client_a
            .send(Message::text(format!(
                "{{\"Signal\": {{\"receiver\": \"{peer_uuid}\", \"data\": \"123\" }}}}"
            )))
            .await;

        let signal_event = recv_peer_event(&mut host).await;
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
        let (connection_requested_tx, mut connection_requested_rx) = unbounded_channel();

        let mut server = SignalingServer::client_server_builder((Ipv4Addr::LOCALHOST, 0))
            .on_connection_request({
                let connection_requested_tx = connection_requested_tx.clone();
                move |_| {
                    connection_requested_tx
                        .send(())
                        .expect("send connection requested");
                    Ok(true)
                }
            })
            .build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        // Connect
        _ = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a")).await;

        connection_requested_rx
            .recv()
            .await
            .expect("connection requested");
    }

    #[tokio::test]
    async fn deny_on_connection_req_callback() {
        let (upgrade_called_tx, mut upgrade_called_rx) = unbounded_channel();
        let (peer_connected_tx, mut peer_connected_rx) = unbounded_channel();

        let mut server = SignalingServer::client_server_builder((Ipv4Addr::LOCALHOST, 0))
            .on_connection_request({
                let upgrade_called_tx = upgrade_called_tx.clone();
                move |_| {
                    upgrade_called_tx.send(()).expect("send upgrade called");
                    Ok(false) // <-- Deny access!
                }
            })
            .on_host_connected({
                // This should not get called because we deny all connections on upgrade
                let peer_connected_tx = peer_connected_tx.clone();
                move |_| peer_connected_tx.send(()).expect("send peer connected")
            })
            .build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        // Connect
        _ = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a")).await;

        upgrade_called_rx.recv().await.expect("upgrade called");
        assert_eq!(peer_connected_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[tokio::test]
    async fn on_id_assignment_callback() {
        let (id_assigned_tx, mut id_assigned_rx) = unbounded_channel();

        let mut server = SignalingServer::client_server_builder((Ipv4Addr::LOCALHOST, 0))
            .on_id_assignment({
                let id_assigned_tx = id_assigned_tx.clone();
                move |_| id_assigned_tx.send(()).expect("send id assigned")
            })
            .build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        // Connect
        tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .expect("handshake");

        id_assigned_rx.recv().await.expect("id assigned");
    }

    #[tokio::test]
    async fn on_host_connect_callback() {
        let (host_connected_tx, mut host_connected_rx) = unbounded_channel();

        let mut server = SignalingServer::client_server_builder((Ipv4Addr::LOCALHOST, 0))
            .on_host_connected({
                let host_connected_tx = host_connected_tx.clone();
                move |_| host_connected_tx.send(()).unwrap()
            })
            .build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        // Connect
        tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .expect("handshake");

        host_connected_rx.recv().await.expect("host connected");
    }

    #[tokio::test]
    async fn on_host_disconnect_callback() {
        let (disconnected_tx, mut disconnected_rx) = unbounded_channel::<()>();

        let mut server = SignalingServer::client_server_builder((Ipv4Addr::LOCALHOST, 0))
            .on_host_disconnected({
                let disconnected_tx = disconnected_tx.clone();
                move |_| {
                    disconnected_tx.send(()).expect("send disconnected");
                }
            })
            .build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        // Connect
        {
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .expect("handshake");
        }
        // Disconnects due to scope drop

        disconnected_rx.recv().await.expect("disconnected");
    }

    #[tokio::test]
    async fn on_client_connect_callback() {
        let (client_connected_tx, mut client_connected_rx) = unbounded_channel();

        let mut server = SignalingServer::client_server_builder((Ipv4Addr::LOCALHOST, 0))
            .on_client_connected({
                let client_connected_tx = client_connected_tx.clone();
                move |_| client_connected_tx.send(()).expect("send client connected")
            })
            .build();
        let addr = server.bind().unwrap();
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

        client_connected_rx.recv().await.expect("client_connected");
    }

    #[tokio::test]
    async fn on_client_disconnect_callback() {
        let (client_disconnected_tx, mut client_disconnected_rx) = unbounded_channel();

        let mut server = SignalingServer::client_server_builder((Ipv4Addr::LOCALHOST, 0))
            .on_client_disconnected({
                let client_disconnected_tx = client_disconnected_tx.clone();
                move |_| {
                    client_disconnected_tx
                        .send(())
                        .expect("send client disconnected")
                }
            })
            .build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        // Connect Host
        let (mut _host, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .expect("handshake");

        // Connect Client
        {
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .expect("handshake");
        }
        // Client disconnects due to scope drop, host remains active

        client_disconnected_rx
            .recv()
            .await
            .expect("client disconnected");
    }
}
