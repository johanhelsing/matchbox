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
//! directly although STUN gives them the same public address, and another makes the NATs
//! symmetric, where only the TURN server (also at 10.0.0.1) gets them through. Each peer is
//! native, Chrome or Firefox, the browsers run through wasm-pack inside the LAN (see
//! tests/nat_browser.rs and webdriver.json, Chrome's sandbox doesn't start inside a user
//! namespace). Ignored by default:
//! `cargo test -p matchbox_socket --test nat -- --ignored`
//!
//! Set `MATCHBOX_NAT_LOG` to a directory to get a debug log per process and the routers'
//! conntrack tables.

#![cfg(all(not(target_arch = "wasm32"), target_os = "linux"))]

use log::{Level, LevelFilter, Log, Metadata, Record};
use matchbox_signaling::SignalingServer;
use matchbox_socket::{
    CandidateType, ChannelConfig, Packet, PeerId, PeerState, RtcIceServerConfig, WebRtcSocket,
};
use rtc_stun::{
    fingerprint::FINGERPRINT,
    message::{BINDING_REQUEST, BINDING_SUCCESS, Message},
    xoraddr::XorMappedAddress,
};
use std::{
    env,
    ffi::OsString,
    fs::{self, File},
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    path::Path,
    process::{self, Child, Command, Stdio},
    sync::Arc,
    thread,
    time::Duration,
};
use tokio::time::{sleep, timeout};
use turn::{
    auth::{AuthHandler, generate_auth_key},
    relay::relay_static::RelayAddressGeneratorStatic,
    server::{
        Server,
        config::{ConnConfig, ServerConfig},
    },
};
use webrtc_util::vnet::net::Net;

/// Set for the processes the test spawns, so that the modes below do nothing when run directly
const ROLE_ENV: &str = "MATCHBOX_NAT_ROLE";
const LOG_ENV: &str = "MATCHBOX_NAT_LOG";

const INTERNET: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);
const STUN_PORT: u16 = 3478;
const TURN_PORT: u16 = 3479;
const TURN_USER: &str = "matchbox";
const TURN_PASSWORD: &str = "hunter2";
const TURN_REALM: &str = "matchbox";
const SIGNALING_PORT: u16 = 3536;
const TIMEOUT: Duration = Duration::from_secs(20);
/// Long enough for ICE to give up on a connection, which takes about 30 seconds
const ICE_FAILURE_TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(10);

/// Which ICE servers the peers get, see [`Ice`]
const ICE_ENV: &str = "MATCHBOX_NAT_ICE";
/// Which network to build, see [`Topology`]
const TOPOLOGY_ENV: &str = "MATCHBOX_NAT_TOPOLOGY";
/// The two peers, see [`Peer`], comma separated
const PEERS_ENV: &str = "MATCHBOX_NAT_PEERS";

#[derive(Clone, Copy, PartialEq)]
enum Topology {
    /// Each peer on its own LAN behind its own NAT
    SeparateNats,
    /// Like `SeparateNats`, but the NATs pick a new port per destination, so the address STUN
    /// reports is useless and hole punching fails
    SymmetricNats,
    /// Both peers on one LAN behind one NAT
    SharedLan,
}

impl Topology {
    fn name(self) -> &'static str {
        match self {
            Self::SeparateNats => "separate_nats",
            Self::SymmetricNats => "symmetric_nats",
            Self::SharedLan => "shared_lan",
        }
    }

    fn from_name(name: &str) -> Self {
        match name {
            "separate_nats" => Self::SeparateNats,
            "symmetric_nats" => Self::SymmetricNats,
            "shared_lan" => Self::SharedLan,
            _ => panic!("unknown topology {name}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Ice {
    /// Host candidates only
    NoServers,
    Stun,
    StunAndTurn,
}

impl Ice {
    fn name(self) -> &'static str {
        match self {
            Self::NoServers => "none",
            Self::Stun => "stun",
            Self::StunAndTurn => "turn",
        }
    }

    fn from_name(name: &str) -> Self {
        match name {
            "none" => Self::NoServers,
            "stun" => Self::Stun,
            "turn" => Self::StunAndTurn,
            _ => panic!("unknown ice setting {name}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Peer {
    /// This test binary
    Native,
    /// tests/nat_browser.rs in headless Chrome, through wasm-pack
    Chrome,
    /// tests/nat_browser.rs in headless Firefox, through wasm-pack
    Firefox,
}

impl Peer {
    fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Chrome => "chrome",
            Self::Firefox => "firefox",
        }
    }

    fn from_name(name: &str) -> Self {
        match name {
            "native" => Self::Native,
            "chrome" => Self::Chrome,
            "firefox" => Self::Firefox,
            _ => panic!("unknown peer {name}"),
        }
    }
}

use Ice::{NoServers, Stun, StunAndTurn};
use Peer::{Chrome, Firefox, Native};
use Topology::{SeparateNats, SharedLan, SymmetricNats};

#[test]
#[ignore]
fn peers_behind_different_nats_connect() {
    run_orchestrator(SeparateNats, [Native, Native], Stun);
}

/// Checks that the NATs isolate the peers: without STUN they must not find each other
#[test]
#[ignore]
fn peers_behind_different_nats_need_stun() {
    run_orchestrator(SeparateNats, [Native, Native], NoServers);
}

/// STUN tells both peers the same public address, they must connect over the LAN anyway
#[test]
#[ignore]
fn peers_on_the_same_lan_connect() {
    run_orchestrator(SharedLan, [Native, Native], Stun);
}

/// Checks that the symmetric NATs defeat hole punching: with STUN only, no connection
#[test]
#[ignore]
fn peers_behind_symmetric_nats_need_turn() {
    run_orchestrator(SymmetricNats, [Native, Native], Stun);
}

#[test]
#[ignore]
fn peers_behind_symmetric_nats_connect_through_turn() {
    run_orchestrator(SymmetricNats, [Native, Native], StunAndTurn);
}

#[test]
#[ignore]
fn native_and_chrome_behind_symmetric_nats() {
    run_orchestrator(SymmetricNats, [Native, Chrome], StunAndTurn);
}

#[test]
#[ignore]
fn native_and_firefox_behind_symmetric_nats() {
    run_orchestrator(SymmetricNats, [Native, Firefox], StunAndTurn);
}

#[test]
#[ignore]
fn chrome_and_chrome_behind_symmetric_nats() {
    run_orchestrator(SymmetricNats, [Chrome, Chrome], StunAndTurn);
}

#[test]
#[ignore]
fn firefox_and_firefox_behind_symmetric_nats() {
    run_orchestrator(SymmetricNats, [Firefox, Firefox], StunAndTurn);
}

#[test]
#[ignore]
fn chrome_and_firefox_behind_symmetric_nats() {
    run_orchestrator(SymmetricNats, [Chrome, Firefox], StunAndTurn);
}

#[test]
#[ignore]
fn native_and_chrome_behind_different_nats() {
    run_orchestrator(SeparateNats, [Native, Chrome], Stun);
}

#[test]
#[ignore]
fn native_and_firefox_behind_different_nats() {
    run_orchestrator(SeparateNats, [Native, Firefox], Stun);
}

#[test]
#[ignore]
fn chrome_and_chrome_behind_different_nats() {
    run_orchestrator(SeparateNats, [Chrome, Chrome], Stun);
}

#[test]
#[ignore]
fn firefox_and_firefox_behind_different_nats() {
    run_orchestrator(SeparateNats, [Firefox, Firefox], Stun);
}

#[test]
#[ignore]
fn chrome_and_firefox_behind_different_nats() {
    run_orchestrator(SeparateNats, [Chrome, Firefox], Stun);
}

#[test]
#[ignore]
fn native_and_chrome_on_the_same_lan() {
    run_orchestrator(SharedLan, [Native, Chrome], Stun);
}

#[test]
#[ignore]
fn native_and_firefox_on_the_same_lan() {
    run_orchestrator(SharedLan, [Native, Firefox], Stun);
}

#[test]
#[ignore]
fn chrome_and_chrome_on_the_same_lan() {
    run_orchestrator(SharedLan, [Chrome, Chrome], Stun);
}

#[test]
#[ignore]
fn firefox_and_firefox_on_the_same_lan() {
    run_orchestrator(SharedLan, [Firefox, Firefox], Stun);
}

#[test]
#[ignore]
fn chrome_and_firefox_on_the_same_lan() {
    run_orchestrator(SharedLan, [Chrome, Firefox], Stun);
}

fn run_orchestrator(topology: Topology, peers: [Peer; 2], ice: Ice) {
    // A new user and network namespace, where we are root and own the network
    let status = Command::new("unshare")
        .args(["--user", "--map-root-user", "--net"])
        .arg(env::current_exe().unwrap())
        .args(["orchestrator", "--exact", "--ignored", "--nocapture"])
        .env(ROLE_ENV, "orchestrator")
        .env(TOPOLOGY_ENV, topology.name())
        .env(PEERS_ENV, peers.map(Peer::name).join(","))
        .env(ICE_ENV, ice.name())
        .status()
        .expect("failed to run unshare");
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

/// A peer in the network namespace of `host`
fn spawn_peer(peer: Peer, host: &Child) -> Child {
    let browser = match peer {
        Native => return spawn_role("nat_peer", Some(host)),
        Chrome => "--chrome",
        Firefox => "--firefox",
    };
    // A wasm test can't read the environment, so the ICE servers are baked into two files
    let test = match Ice::from_name(&env::var(ICE_ENV).unwrap()) {
        StunAndTurn => "nat_browser_turn",
        _ => "nat_browser",
    };
    let mut command = Command::new("nsenter");
    command
        .args(["-t", &host.id().to_string(), "-n"])
        .args(["wasm-pack", "test", "--headless", browser, "--test", test])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env(
            "RUSTFLAGS",
            "--cfg=web_sys_unstable_apis --cfg=getrandom_backend=\"wasm_js\"",
        );
    if let Some(dir) = log_dir() {
        let name = format!("{}-{}.log", peer.name(), host.id());
        command.stderr(File::create(Path::new(&dir).join(name)).unwrap());
    }
    command
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn wasm-pack: {e}"))
}

/// A NAT between the internet and LAN `i`, with the public address 10.0.0.1i and a bridge for
/// the LAN 192.168.i.0/24. A symmetric one maps every destination to a new public port.
fn setup_router(i: u8, symmetric: bool) -> Child {
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
    let random_ports = if symmetric { " --random-fully" } else { "" };
    in_netns(
        &router,
        &format!("iptables -t nat -A POSTROUTING -o {pub_end} -j MASQUERADE{random_ports}"),
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
    let topology = Topology::from_name(&env::var(TOPOLOGY_ENV).unwrap());
    let peers: Vec<Peer> = env::var(PEERS_ENV)
        .unwrap()
        .split(',')
        .map(Peer::from_name)
        .collect();

    run("ip link set lo up");
    run("ip link add br0 type bridge");
    run(&format!("ip addr add {INTERNET}/24 dev br0"));
    run("ip link set br0 up");

    let mut servers = [
        spawn_role("stun_server", None),
        spawn_role("turn_server", None),
        spawn_role("signaling_server", None),
    ];

    let (mut routers, mut hosts) = (vec![], vec![]);
    match topology {
        SeparateNats | SymmetricNats => {
            for i in 1..=2 {
                let router = setup_router(i, topology == SymmetricNats);
                hosts.push(add_host(&router, i, 2));
                routers.push(router);
            }
        }
        SharedLan => {
            let router = setup_router(1, false);
            hosts.push(add_host(&router, 1, 2));
            hosts.push(add_host(&router, 1, 3));
            routers.push(router);
        }
    }

    // Give the servers a moment to bind
    thread::sleep(Duration::from_millis(500));

    let mut peers: Vec<Child> = peers
        .iter()
        .zip(&hosts)
        .map(|(peer, host)| spawn_peer(*peer, host))
        .collect();
    // Wait for both, so that no peer process is left behind
    let results: Vec<bool> = peers
        .iter_mut()
        .map(|peer| peer.wait().unwrap().success())
        .collect();
    let ok = results.iter().all(|ok| *ok);

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

/// Relays for peers that can't reach each other, with the relay addresses on the internet side
#[tokio::test]
#[ignore]
async fn turn_server() {
    if role().as_deref() != Some("turn_server") {
        return;
    }
    struct Auth;
    impl AuthHandler for Auth {
        fn auth_handle(
            &self,
            username: &str,
            realm: &str,
            _: SocketAddr,
        ) -> Result<Vec<u8>, turn::Error> {
            if username == TURN_USER && realm == TURN_REALM {
                Ok(generate_auth_key(username, realm, TURN_PASSWORD))
            } else {
                Err(turn::Error::ErrFakeErr)
            }
        }
    }

    let socket = tokio::net::UdpSocket::bind((INTERNET, TURN_PORT))
        .await
        .unwrap();
    let _server = Server::new(ServerConfig {
        conn_configs: vec![ConnConfig {
            conn: Arc::new(socket),
            relay_addr_generator: Box::new(RelayAddressGeneratorStatic {
                relay_address: IpAddr::V4(INTERNET),
                address: INTERNET.to_string(),
                net: Arc::new(Net::new(None)),
            }),
        }],
        realm: TURN_REALM.to_string(),
        auth_handler: Arc::new(Auth),
        channel_bind_timeout: Duration::from_secs(0),
        alloc_close_notify: None,
    })
    .await
    .unwrap();
    std::future::pending::<()>().await;
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
    let topology = Topology::from_name(&env::var(TOPOLOGY_ENV).unwrap());
    let ice = Ice::from_name(&env::var(ICE_ENV).unwrap());
    let (urls, username, credential) = match ice {
        NoServers => (vec![], None, None),
        Stun => (vec![format!("stun:{INTERNET}:{STUN_PORT}")], None, None),
        StunAndTurn => (
            vec![
                format!("stun:{INTERNET}:{STUN_PORT}"),
                format!("turn:{INTERNET}:{TURN_PORT}"),
            ],
            Some(TURN_USER.to_string()),
            Some(TURN_PASSWORD.to_string()),
        ),
    };
    let (mut socket, message_loop) =
        WebRtcSocket::builder(format!("ws://{INTERNET}:{SIGNALING_PORT}/nat"))
            .ice_server(RtcIceServerConfig {
                urls,
                username,
                credential,
            })
            .add_channel(ChannelConfig::reliable())
            .build();
    tokio::spawn(message_loop);

    let can_connect = !matches!((topology, ice), (_, NoServers) | (SymmetricNats, Stun));
    if !can_connect {
        // The failed handshake must still end, with the peer reported disconnected
        let disconnected = timeout(ICE_FAILURE_TIMEOUT, async {
            loop {
                let states: Vec<_> = socket.update_peers().into_iter().map(|(_, s)| s).collect();
                assert!(
                    !states.contains(&PeerState::Connected),
                    "connected where it shouldn't be possible"
                );
                if states.contains(&PeerState::Disconnected) {
                    return;
                }
                sleep(POLL).await;
            }
        });
        disconnected
            .await
            .expect("peer never reported disconnected after the connection failed");
        return;
    }

    let peer = wait_for_peer(&mut socket, PeerState::Connected).await;

    let pair = timeout(TIMEOUT, async {
        loop {
            socket.update_peers();
            if let Some(pair) = socket.candidate_pair(peer) {
                return pair;
            }
            sleep(POLL).await;
        }
    })
    .await
    .expect("candidate pair never reported");
    // Our own end is always the socket's address, the peer's tells the path
    use CandidateType::{Host, PeerReflexive, ServerReflexive};
    match topology {
        // The address STUN told the peer
        SeparateNats => assert_eq!(pair.remote, ServerReflexive, "{pair:?}"),
        // Directly on the LAN. A browser signals its address as an mDNS name, which can't be
        // matched to the address its packets come from, so that stays peer reflexive.
        SharedLan => assert!(matches!(pair.remote, Host | PeerReflexive), "{pair:?}"),
        SymmetricNats => assert!(pair.is_relayed(), "{pair:?}"),
    }

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
