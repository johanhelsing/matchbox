#[cfg(test)]
mod tests {
    use futures::{SinkExt, StreamExt};
    use matchbox_protocol::{JsonPeerEvent, PeerId};
    use matchbox_signaling::SignalingServer;
    use std::{net::Ipv4Addr, str::FromStr};
    use tokio::{sync::mpsc::unbounded_channel, time::sleep};
    use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
    use std::time::Duration;
    use tokio::net::TcpStream;

    async fn recv_peer_event(
        client: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    ) -> JsonPeerEvent {
        let message: Message = client.next().await.unwrap().unwrap();
        JsonPeerEvent::from_str(&message.to_string()).expect("json peer event")
    }

    async fn connect_client(addr: &str) -> WebSocketStream<MaybeTlsStream<TcpStream>> {
        let (client, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .expect("handshake");
        client
    }

    #[tokio::test]
    async fn hybrid_connect_disconnect() {
        let mut server = SignalingServer::hybrid_builder((Ipv4Addr::LOCALHOST, 0)).build();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let mut clients = vec![];

        // Connect 10 clients
        for i in 0..10 {
            let client = connect_client(&addr.to_string()).await;
            println!("Client {} connected", i);
            clients.push(client);
        }

        sleep(Duration::from_secs(1)).await; // Allow time for processing

        // Disconnect all clients
        clients.clear();

        sleep(Duration::from_secs(1)).await; // Allow time for cleanup
    }
}
