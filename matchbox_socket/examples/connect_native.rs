use log::info;
use matchbox_socket::WebRtcSocket;
use std::{env, time::Duration};
use tokio::{select, time::sleep};

#[tokio::main]
async fn main() {
    if env::var_os("RUST_LOG").is_none() {
        env::set_var("RUST_LOG", "connect_native=info,matchbox_socket=info");
    }
    pretty_env_logger::init();

    info!("Connecting to matchbox");
    let (mut socket, loop_fut) = WebRtcSocket::new("ws://localhost:3536/native_example_room");

    info!("my id is {:?}", socket.id());

    tokio::pin!(loop_fut);

    loop {
        for peer in socket.accept_new_connections() {
            info!("Found a peer {:?}", peer);
            let packet = "hello friend!".as_bytes().to_vec().into_boxed_slice();
            socket.send(packet, peer);
        }

        for (peer, packet) in socket.receive() {
            info!("Received from {:?}: {:?}", peer, packet);
        }

        let timeout = sleep(Duration::from_millis(100));
        tokio::pin!(timeout);
        select! {
            _ = timeout => {}
            _ = &mut loop_fut => {
                break;
            }
        }
    }

    info!("Done");
}
