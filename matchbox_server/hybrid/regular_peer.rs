use matchbox_socket::WebRtcSocket;
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() {
    // Connect to the super peer's signaling server
    let (mut socket, message_loop) = WebRtcSocket::new_reliable("ws://127.0.0.1:3536");

    // Run the message loop in the background
    tokio::spawn(message_loop);

    println!("Waiting to be matched with another peer...");

    // Wait for the super peer to assign us to a room and notify us of other peers
    while socket.connected_peers().len() == 0 {
        sleep(Duration::from_millis(10)).await;
    }

    let peer_id = socket.connected_peers()[0].clone();
    println!("Connected to peer: {}", peer_id);

    // Send a message to the matched peer
    let message = b"Hello from a regular peer!";
    socket.send(message.to_vec().into(), peer_id.clone());

    // Receive messages
    loop {
        if let Some((peer, message)) = socket.receive() {
            println!("Received message from {}: {:?}", peer, message);
        }
        sleep(Duration::from_millis(10)).await;
    }
}