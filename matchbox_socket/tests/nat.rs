//! Two native peers behind different NATs, connecting through STUN.
//!
//! Linux only. Everything runs in an unprivileged user namespace, so no root is needed:
//!
//! ```text
//! lan1 (peer, 192.168.1.2) - router1 (NAT, 10.0.0.11) -+
//!                                                       +- br0: signaling and STUN at 10.0.0.1
//! lan2 (peer, 192.168.2.2) - router2 (NAT, 10.0.0.12) -+
//! ```
//!
//! The peers' host candidates are unreachable from each other, so connecting at all means the
//! server reflexive candidates worked. A variant puts both peers on lan1, where they must connect
//! directly although STUN gives them the same public address. Ignored by default:
//! `cargo test -p matchbox_socket --test nat -- --ignored`
//!
//! Set `MATCHBOX_NAT_LOG` to a directory to get a debug log per process and the routers'
//! conntrack tables.

#![cfg(all(not(target_arch = "wasm32"), target_os = "linux"))]

use log::{Level, LevelFilter, Log, Metadata, Record};
use matchbox_signaling::SignalingServer;
use matchbox_socket::{ChannelConfig, Packet, PeerId, PeerState, RtcIceServerConfig, WebRtcSocket};
use rtc_stun::{
    fingerprint::FINGERPRINT,
    message::{BINDING_REQUEST, BINDING_SUCCESS, Message},
    xoraddr::XorMappedAddress,
};
use std::{
    env,
    ffi::OsString,
    fs::{self, File},
    net::{Ipv4Addr, UdpSocket},
    path::Path,
    process::{self, Child, Command, Stdio},
    thread,
    time::Duration,
};
use tokio::time::{sleep, timeout};

/// Set for the processes the test spawns, so that the modes below do nothing when run directly
const ROLE_ENV: &str = "MATCHBOX_NAT_ROLE";
const LOG_ENV: &str = "MATCHBOX_NAT_LOG";

const INTERNET: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);
const STUN_PORT: u16 = 3478;
const SIGNALING_PORT: u16 = 3536;
const TIMEOUT: Duration = Duration::from_secs(20);
/// Long enough for ICE to have connected if it was going to
const NO_STUN_WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(10);

/// Set to run the peers without STUN
const NO_STUN_ENV: &str = "MATCHBOX_NAT_NO_STUN";
/// Which network to build, see [`Topology`]
const TOPOLOGY_ENV: &str = "MATCHBOX_NAT_TOPOLOGY";

#[derive(Clone, Copy, PartialEq)]
enum Topology {
    /// Each peer on its own LAN behind its own NAT
    SeparateNats,
    /// Both peers on one LAN behind one NAT
    SharedLan,
}

#[test]
#[ignore]
fn peers_behind_different_nats_connect() {
    run_orchestrator(Topology::SeparateNats, true);
}

/// Checks that the NATs isolate the peers: without STUN they must not find each other
#[test]
#[ignore]
fn peers_behind_different_nats_need_stun() {
    run_orchestrator(Topology::SeparateNats, false);
}

/// STUN tells both peers the same public address, they must connect over the LAN anyway
#[test]
#[ignore]
fn peers_on_the_same_lan_connect() {
    run_orchestrator(Topology::SharedLan, true);
}

fn run_orchestrator(topology: Topology, stun: bool) {
    // A new user and network namespace, where we are root and own the network
    let mut command = Command::new("unshare");
    command
        .args(["--user", "--map-root-user", "--net"])
        .arg(env::current_exe().unwrap())
        .args(["orchestrator", "--exact", "--ignored", "--nocapture"])
        .env(ROLE_ENV, "orchestrator")
        .env(
            TOPOLOGY_ENV,
            match topology {
                Topology::SeparateNats => "separate_nats",
                Topology::SharedLan => "shared_lan",
            },
        );
    if !stun {
        command.env(NO_STUN_ENV, "1");
    }
    let status = command.status().expect("failed to run unshare");
    assert!(status.success(), "{status}");
}

fn role() -> Option<String> {
    env::var(ROLE_ENV).ok()
}

fn log_dir() -> Option<OsString> {
    env::var_os(LOG_ENV)
}

/// Runs a command line, which must succeed
fn run(command: &str) {
    let args: Vec<&str> = command.split_whitespace().collect();
    let output = Command::new(args[0])
        .args(&args[1..])
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", args[0]));
    assert!(
        output.status.success(),
        "{command} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs a command line inside the network namespace of `netns`
fn in_netns(netns: &Child, command: &str) {
    run(&format!("nsenter -t {} -n {command}", netns.id()));
}

/// A process holding a new network namespace open
fn new_netns() -> Child {
    Command::new("unshare")
        .args(["--net", "sleep", "infinity"])
        .spawn()
        .expect("failed to create a network namespace")
}

/// This test binary in the given role, in the network namespace of `netns` if given
fn spawn_role(role: &str, netns: Option<&Child>) -> Child {
    let exe = env::current_exe().unwrap();
    let mut command = match netns {
        Some(netns) => {
            let mut c = Command::new("nsenter");
            c.args(["-t", &netns.id().to_string(), "-n"]).arg(exe);
            c
        }
        None => Command::new(exe),
    };
    command
        .args([role, "--exact", "--ignored", "--nocapture"])
        .env(ROLE_ENV, role)
        .stdout(Stdio::inherit());
    // One log file per process, their stderrs interleave otherwise
    if let Some(dir) = log_dir() {
        let name = format!("{role}-{}.log", netns.map_or(0, Child::id));
        command.stderr(File::create(Path::new(&dir).join(name)).unwrap());
    }
    command
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {role}: {e}"))
}

/// A NAT between the internet and LAN `i`, with the public address 10.0.0.1i and a bridge for
/// the LAN 192.168.i.0/24
fn setup_router(i: u8) -> Child {
    let router = new_netns();
    let (pub_end, bridge_end) = (format!("r{i}-pub"), format!("r{i}-br"));

    run(&format!(
        "ip link add {pub_end} type veth peer name {bridge_end}"
    ));
    run(&format!("ip link set {bridge_end} master br0"));
    run(&format!("ip link set {bridge_end} up"));
    run(&format!("ip link set {pub_end} netns {}", router.id()));

    in_netns(&router, "ip link set lo up");
    in_netns(
        &router,
        &format!("ip addr add 10.0.0.1{i}/24 dev {pub_end}"),
    );
    in_netns(&router, &format!("ip link set {pub_end} up"));
    in_netns(&router, "ip link add lan type bridge");
    in_netns(&router, &format!("ip addr add 192.168.{i}.1/24 dev lan"));
    in_netns(&router, "ip link set lan up");
    in_netns(&router, "sysctl -q net.ipv4.ip_forward=1");
    in_netns(
        &router,
        &format!("iptables -t nat -A POSTROUTING -o {pub_end} -j MASQUERADE"),
    );
    // Drop unsolicited packets from the internet like a real NAT does. Accepted, they would
    // create conntrack entries that make MASQUERADE pick new ports for the peer's own flows,
    // which breaks hole punching.
    in_netns(
        &router,
        &format!("iptables -A INPUT -i {pub_end} -m conntrack --ctstate NEW,INVALID -j DROP"),
    );
    router
}

/// A host on the LAN of `router`, at 192.168.i.h
fn add_host(router: &Child, i: u8, h: u8) -> Child {
    let host = new_netns();
    let (lan_end, host_end) = (format!("h{i}{h}-lan"), format!("h{i}{h}"));

    run(&format!(
        "ip link add {lan_end} type veth peer name {host_end}"
    ));
    run(&format!("ip link set {lan_end} netns {}", router.id()));
    run(&format!("ip link set {host_end} netns {}", host.id()));
    in_netns(router, &format!("ip link set {lan_end} master lan"));
    in_netns(router, &format!("ip link set {lan_end} up"));

    in_netns(&host, "ip link set lo up");
    in_netns(
        &host,
        &format!("ip addr add 192.168.{i}.{h}/24 dev {host_end}"),
    );
    in_netns(&host, &format!("ip link set {host_end} up"));
    in_netns(&host, &format!("ip route add default via 192.168.{i}.1"));
    host
}

/// Sets up the network, runs the servers and the peers, and exits with the peers' result
#[test]
#[ignore]
fn orchestrator() {
    if role().as_deref() != Some("orchestrator") {
        return;
    }
    let topology = match env::var(TOPOLOGY_ENV).as_deref() {
        Ok("separate_nats") => Topology::SeparateNats,
        Ok("shared_lan") => Topology::SharedLan,
        other => panic!("unknown topology {other:?}"),
    };

    run("ip link set lo up");
    run("ip link add br0 type bridge");
    run(&format!("ip addr add {INTERNET}/24 dev br0"));
    run("ip link set br0 up");

    let mut servers = [
        spawn_role("stun_server", None),
        spawn_role("signaling_server", None),
    ];

    let (mut routers, mut hosts) = (vec![], vec![]);
    match topology {
        Topology::SeparateNats => {
            for i in 1..=2 {
                let router = setup_router(i);
                hosts.push(add_host(&router, i, 2));
                routers.push(router);
            }
        }
        Topology::SharedLan => {
            let router = setup_router(1);
            hosts.push(add_host(&router, 1, 2));
            hosts.push(add_host(&router, 1, 3));
            routers.push(router);
        }
    }

    // Give the servers a moment to bind
    thread::sleep(Duration::from_millis(500));

    let mut peers: Vec<Child> = hosts
        .iter()
        .map(|host| spawn_role("nat_peer", Some(host)))
        .collect();
    let ok = peers.iter_mut().all(|peer| peer.wait().unwrap().success());

    if let Some(dir) = log_dir() {
        for (i, router) in routers.iter().enumerate() {
            let conntrack = Command::new("nsenter")
                .args(["-t", &router.id().to_string(), "-n"])
                .args(["cat", "/proc/net/nf_conntrack"])
                .output()
                .unwrap()
                .stdout;
            fs::write(
                Path::new(&dir).join(format!("router{}.conntrack", i + 1)),
                conntrack,
            )
            .unwrap();
        }
    }

    for child in servers.iter_mut().chain(&mut routers).chain(&mut hosts) {
        let _ = child.kill();
    }
    process::exit(if ok { 0 } else { 1 });
}

/// Answers STUN binding requests with the sender's address, which is the NAT's public one
#[test]
#[ignore]
fn stun_server() {
    if role().as_deref() != Some("stun_server") {
        return;
    }
    let socket = UdpSocket::bind((INTERNET, STUN_PORT)).unwrap();
    let mut buf = [0u8; 1500];
    loop {
        let (len, from) = socket.recv_from(&mut buf).unwrap();
        let mut request = Message::new();
        if request.unmarshal_binary(&buf[..len]).is_err() || request.typ != BINDING_REQUEST {
            continue;
        }
        let mut response = Message::new();
        response
            .build(&[
                Box::new(BINDING_SUCCESS),
                Box::new(request.transaction_id),
                Box::new(XorMappedAddress {
                    ip: from.ip(),
                    port: from.port(),
                }),
                Box::new(FINGERPRINT),
            ])
            .unwrap();
        eprintln!("binding request from {from}");
        socket.send_to(&response.raw, from).unwrap();
    }
}

#[tokio::test]
#[ignore]
async fn signaling_server() {
    if role().as_deref() != Some("signaling_server") {
        return;
    }
    let mut server = SignalingServer::full_mesh_builder((INTERNET, SIGNALING_PORT)).build();
    server.bind().unwrap();
    server.serve().await.unwrap();
}

/// Connects through the NAT and exchanges a packet with the other peer. The peer with the lower
/// id pings and leaves once it has the pong, the other one answers and leaves once the first is
/// gone, so that neither exits with a packet still queued.
#[tokio::test]
#[ignore]
async fn nat_peer() {
    if role().as_deref() != Some("nat_peer") {
        return;
    }
    init_logging();
    let stun = env::var_os(NO_STUN_ENV).is_none();
    let (mut socket, message_loop) =
        WebRtcSocket::builder(format!("ws://{INTERNET}:{SIGNALING_PORT}/nat"))
            .ice_server(RtcIceServerConfig {
                urls: stun
                    .then(|| format!("stun:{INTERNET}:{STUN_PORT}"))
                    .into_iter()
                    .collect(),
                ..Default::default()
            })
            .add_channel(ChannelConfig::reliable())
            .build();
    tokio::spawn(message_loop);

    if !stun {
        let connected = timeout(
            NO_STUN_WAIT,
            wait_for_peer(&mut socket, PeerState::Connected),
        );
        assert!(connected.await.is_err(), "connected without STUN");
        return;
    }

    let peer = wait_for_peer(&mut socket, PeerState::Connected).await;
    if socket.id().unwrap() < peer {
        socket.channel_mut(0).send(b"ping".to_vec().into(), peer);
        assert_eq!(
            wait_for_packets(&mut socket).await,
            vec![(peer, b"pong".to_vec().into())]
        );
    } else {
        assert_eq!(
            wait_for_packets(&mut socket).await,
            vec![(peer, b"ping".to_vec().into())]
        );
        socket.channel_mut(0).send(b"pong".to_vec().into(), peer);
        wait_for_peer(&mut socket, PeerState::Disconnected).await;
    }
}

async fn wait_for_peer(socket: &mut WebRtcSocket, state: PeerState) -> PeerId {
    timeout(TIMEOUT, async {
        loop {
            if let Some((id, _)) = socket.update_peers().into_iter().find(|(_, s)| *s == state) {
                return id;
            }
            sleep(POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no peer became {state:?}"))
}

async fn wait_for_packets(socket: &mut WebRtcSocket) -> Vec<(PeerId, Packet)> {
    timeout(TIMEOUT, async {
        loop {
            let packets = socket.channel_mut(0).receive();
            if !packets.is_empty() {
                return packets;
            }
            sleep(POLL).await;
        }
    })
    .await
    .expect("no packet received")
}

/// Logs matchbox and webrtc to stderr when `MATCHBOX_NAT_LOG` is set
struct StderrLogger;

impl Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{} {}] {}", record.level(), record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

fn init_logging() {
    if log_dir().is_some() {
        let _ = log::set_logger(&StderrLogger);
        log::set_max_level(LevelFilter::Debug);
    }
}
