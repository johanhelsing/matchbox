use std::time::Duration;

use log::info;
use matchbox_socket::WebRtcSocket;
use tokio::{select, time::sleep};

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    info!("Connecting to matchbox");
    let (mut socket, loop_fut) = WebRtcSocket::new("ws://localhost:3536/native_example_room");

    info!("my id is {:?}", socket.id());

    tokio::pin!(loop_fut);

    loop {
        let packets = socket.receive();

        for (peer, packet) in packets {
            info!("Received from {:?}: {:?}", peer, packet);
        }

        let timeout = sleep(Duration::from_millis(100));
        tokio::pin!(timeout);
        select! {
            _ = timeout => {}
            // TODO: restarting this future inside `select!` probably isn't a
            // good idea
            peers = socket.wait_for_peers(1) => {
                info!("Found a peer {:?}", peers);
                let peer = &peers[0];
                let packet = "hello friend!".as_bytes().to_vec().into_boxed_slice();
                socket.send(packet, peer);
            },
            _ = &mut loop_fut => {
                break;
            }
        }
    }

    info!("Done");
}
