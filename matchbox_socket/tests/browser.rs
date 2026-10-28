//! Browser peers against a native one, in Chrome and Firefox. Needs wasm-pack and the browsers,
//! so ignored by default:
//! `cargo test -p matchbox_socket --test browser -- --ignored`

#![cfg(not(target_arch = "wasm32"))]

use matchbox_signaling::SignalingServer;
use matchbox_socket::{ChannelConfig, RtcIceServerConfig, WebRtcSocket};
use std::{net::Ipv4Addr, process::Command, time::Duration};
use tokio::{runtime::Runtime, time::sleep};

/// Also in tests/browser_peer.rs
const PORT: u16 = 3536;

#[tokio::test]
#[ignore]
async fn browser_peer_exchanges_packets() {
    let server_runtime = Runtime::new().unwrap();
    let _guard = server_runtime.enter();
    let mut server = SignalingServer::full_mesh_builder((Ipv4Addr::LOCALHOST, PORT)).build();
    server.bind().unwrap();
    server_runtime.spawn(server.serve());

    let (mut native, message_loop) =
        WebRtcSocket::builder(format!("ws://127.0.0.1:{PORT}/browser"))
            .ice_server(RtcIceServerConfig {
                urls: vec![],
                ..Default::default()
            })
            .add_channel(ChannelConfig::reliable())
            .build();
    tokio::spawn(message_loop);
    let echo = tokio::spawn(async move {
        loop {
            native.update_peers();
            for (peer, packet) in native.channel_mut(0).receive() {
                assert_eq!(&packet[..], b"ping");
                native.channel_mut(0).send(b"pong".to_vec().into(), peer);
            }
            sleep(Duration::from_millis(10)).await;
        }
    });

    let mut failed = vec![];
    for browser in ["--chrome", "--firefox"] {
        if !wasm_pack_test(browser, "browser_peer").await {
            failed.push(browser.to_string());
        }
    }
    // Both browsers at once, so that they connect to each other
    let (chrome, firefox) = tokio::join!(
        wasm_pack_test("--chrome", "browser_cross"),
        wasm_pack_test("--firefox", "browser_cross")
    );
    if !(chrome && firefox) {
        failed.push("cross".to_string());
    }
    echo.abort();
    drop(_guard);
    server_runtime.shutdown_background();
    assert!(failed.is_empty(), "browser tests failed in {failed:?}");
}

async fn wasm_pack_test(browser: &'static str, test: &'static str) -> bool {
    tokio::task::spawn_blocking(move || {
        Command::new("wasm-pack")
            .args(["test", "--headless", browser, "--test", test])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env(
                "RUSTFLAGS",
                "--cfg=web_sys_unstable_apis --cfg=getrandom_backend=\"wasm_js\"",
            )
            .status()
            .expect("failed to run wasm-pack")
            .success()
    })
    .await
    .unwrap()
}
