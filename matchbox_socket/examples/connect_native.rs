use matchbox_socket::WebRtcSocket;

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    println!("Connecting to matchbox");
    let (_socket, loop_fut) = WebRtcSocket::new("ws://localhost:3536/native_example_room");

    loop_fut.await;
    println!("Done");
}
