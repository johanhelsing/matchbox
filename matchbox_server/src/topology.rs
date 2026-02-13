use crate::state::{Peer, ServerState};
use async_trait::async_trait;
use axum::extract::ws::Message;
use futures::StreamExt;
use matchbox_protocol::{JsonPeerEvent, PeerRequest};
use matchbox_signaling::{
    ClientRequestError, NoCallbacks, SignalingTopology, WsStateMeta, common_logic::parse_request,
};
use tracing::{error, info, warn};

#[derive(Debug, Default)]
pub struct MatchmakingDemoTopology;

#[async_trait]
impl SignalingTopology<NoCallbacks, ServerState> for MatchmakingDemoTopology {
    async fn state_machine(upgrade: WsStateMeta<NoCallbacks, ServerState>) {
        let WsStateMeta {
            peer_id,
            sender,
            mut receiver,
            mut state,
            ..
        } = upgrade;

        let room = state.remove_waiting_peer(peer_id);
        let peer = Peer {
            uuid: peer_id,
            sender: sender.clone(),
            room,
        };

        // Tell other waiting peers about me!
        let peers = state.add_peer(peer);
        let event_text = JsonPeerEvent::NewPeer(peer_id).to_string();
        let event = Message::Text((&event_text).into());
        for peer_id in peers {
            if let Err(e) = state.try_send(peer_id, event.clone()) {
                error!("error sending to {peer_id:?}: {e:?}");
            } else {
                info!("{peer_id} -> {event_text:?}");
            }
        }

        // The state machine for the data channel established for this websocket.
        while let Some(request) = receiver.next().await {
            let request = match parse_request(request) {
                Ok(request) => request,
                Err(e) => {
                    match e {
                        ClientRequestError::Axum(_) => {
                            // Most likely a ConnectionReset or similar.
                            warn!("Unrecoverable error with {peer_id:?}: {e:?}");
                            break;
                        }
                        ClientRequestError::Close => {
                            info!("Connection closed by {peer_id:?}");
                            break;
                        }
                        ClientRequestError::Json(_) | ClientRequestError::UnsupportedType(_) => {
                            error!("Error with request: {:?}", e);
                            continue; // Recoverable error
                        }
                    };
                }
            };

            match request {
                PeerRequest::Signal { receiver, data } => {
                    let event = Message::Text(
                        JsonPeerEvent::Signal {
                            sender: peer_id,
                            data,
                        }
                        .to_string()
                        .into(),
                    );
                    if let Some(peer) = state.get_peer(&receiver) {
                        if let Err(e) = peer.sender.send(Ok(event)) {
                            error!("error sending signal event: {e:?}");
                        }
                    } else {
                        warn!("peer not found ({receiver:?}), ignoring signal");
                    }
                }
                PeerRequest::KeepAlive => {
                    // Do nothing. KeepAlive packets are used to protect against idle websocket
                    // connections getting automatically disconnected, common for reverse proxies.
                }
            }
        }

        // Peer disconnected or otherwise ended communication.
        info!("Removing peer: {:?}", peer_id);
        if let Some(removed_peer) = state.remove_peer(&peer_id) {
            let room = removed_peer.room;
            let other_peers = state
                .get_room_peers(&room)
                .into_iter()
                .filter(|other_id| *other_id != peer_id);
            // Tell each connected peer about the disconnected peer.
            let event = Message::Text(
                JsonPeerEvent::PeerLeft(removed_peer.uuid)
                    .to_string()
                    .into(),
            );

            //Check if the peer was matched by next?
            let matcheds = state.remove_matched_peer(peer_id);
            if !matcheds.is_empty() {
                //Those where matched by next?
                for matched in matcheds {
                    match state.try_send(matched, event.clone()) {
                        Ok(()) => info!("Sent peer remove to: {:?}", peer_id),
                        Err(e) => error!("Failure sending peer remove: {e:?}"),
                    }
                }
            } else {
                for peer_id in other_peers {
                    match state.try_send(peer_id, event.clone()) {
                        Ok(()) => info!("Sent peer remove to: {:?}", peer_id),
                        Err(e) => error!("Failure sending peer remove: {e:?}"),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MatchmakingDemoTopology;
    use crate::{
        ServerState,
        state::{RequestedRoom, RoomId},
    };
    use futures::{SinkExt, StreamExt, pin_mut};
    use matchbox_protocol::{JsonPeerEvent, PeerId};
    use matchbox_signaling::{SignalingServer, SignalingServerBuilder};
    use std::{net::Ipv4Addr, str::FromStr, time::Duration};
    use tokio::{net::TcpStream, select, time};
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

    fn app() -> SignalingServer {
        let mut state = ServerState::default();
        SignalingServerBuilder::new(
            (Ipv4Addr::LOCALHOST, 0),
            MatchmakingDemoTopology,
            state.clone(),
        )
        .on_connection_request({
            let mut state = state.clone();
            move |connection| {
                let room_id = RoomId(connection.path.clone().unwrap_or_default());
                let next = connection
                    .query_params
                    .get("next")
                    .and_then(|next| next.parse::<usize>().ok());
                let room = RequestedRoom { id: room_id, next };
                {
                    state.add_waiting_client(connection.origin, room);
                }
                Ok(true)
            }
        })
        .on_id_assignment({
            move |(origin, peer_id)| {
                state.assign_id_to_waiting_client(origin, peer_id);
            }
        })
        .build()
    }

    // Helper to take the next PeerEvent from a stream
    async fn recv_peer_event(
        client: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    ) -> JsonPeerEvent {
        let message: Message = client
            .next()
            .await
            .expect("some message")
            .expect("socket message");
        JsonPeerEvent::from_str(&message.to_string()).expect("json peer event")
    }

    fn get_peer_id(peer_event: JsonPeerEvent) -> PeerId {
        if let JsonPeerEvent::IdAssigned(id) = peer_event {
            id
        } else {
            panic!("Peer_event was not IdAssigned: {peer_event:?}");
        }
    }

    #[tokio::test]
    async fn ws_connect() {
        let mut server = app();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
            .await
            .expect("handshake");
    }

    #[tokio::test]
    async fn uuid_assigned() {
        let mut server = app();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut client, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let id_assigned_event = recv_peer_event(&mut client).await;

        assert!(matches!(id_assigned_event, JsonPeerEvent::IdAssigned(..)));
    }

    #[tokio::test]
    async fn new_peer() {
        let mut server = app();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let _a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        let new_peer_event = recv_peer_event(&mut client_a).await;

        assert_eq!(new_peer_event, JsonPeerEvent::NewPeer(b_uuid));
    }

    #[tokio::test]
    async fn disconnect_peer() {
        let mut server = app();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let _a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        // Ensure Peer B was received
        let new_peer_event = recv_peer_event(&mut client_a).await;
        assert_eq!(new_peer_event, JsonPeerEvent::NewPeer(b_uuid));

        // Disconnect Peer B
        _ = client_b.close(None).await;
        let peer_left_event = recv_peer_event(&mut client_a).await;

        assert_eq!(peer_left_event, JsonPeerEvent::PeerLeft(b_uuid));
    }

    #[tokio::test]
    async fn signal() {
        let mut server = app();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_a"))
                .await
                .unwrap();

        let _b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        let new_peer_event = recv_peer_event(&mut client_a).await;
        let peer_uuid = match new_peer_event {
            JsonPeerEvent::NewPeer(PeerId(peer_uuid)) => peer_uuid.to_string(),
            _ => panic!("unexpected event"),
        };

        _ = client_a
            .send(Message::text(format!(
                "{{\"Signal\": {{\"receiver\": \"{peer_uuid}\", \"data\": \"123\" }}}}"
            )))
            .await;

        let signal_event = recv_peer_event(&mut client_b).await;
        assert_eq!(
            signal_event,
            JsonPeerEvent::Signal {
                data: serde_json::Value::String("123".to_string()),
                sender: a_uuid,
            }
        );
    }

    #[tokio::test]
    async fn match_pairs() {
        let mut server = app();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_name?next=2"))
                .await
                .unwrap();

        let _a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_name?next=2"))
                .await
                .unwrap();

        let b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        let (mut client_c, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_name?next=2"))
                .await
                .unwrap();

        let _c_uuid = get_peer_id(recv_peer_event(&mut client_c).await);

        let (mut client_d, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_name?next=2"))
                .await
                .unwrap();

        let d_uuid = get_peer_id(recv_peer_event(&mut client_d).await);

        // Clients should be matched in pairs as they arrive, i.e. a + b and c + d
        let new_peer_b = recv_peer_event(&mut client_a).await;
        let new_peer_d = recv_peer_event(&mut client_c).await;

        assert_eq!(new_peer_b, JsonPeerEvent::NewPeer(b_uuid));
        assert_eq!(new_peer_d, JsonPeerEvent::NewPeer(d_uuid));

        let timeout = time::sleep(Duration::from_millis(100));
        pin_mut!(timeout);
        select! {
            _ = client_a.next() => panic!("unexpected message"),
            _ = client_b.next() => panic!("unexpected message"),
            _ = client_c.next() => panic!("unexpected message"),
            _ = client_d.next() => panic!("unexpected message"),
            _ = &mut timeout => {}
        }
    }
    #[tokio::test]
    async fn match_pair_and_other_alone_room_without_next() {
        let mut server = app();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_name?next=2"))
                .await
                .unwrap();

        let _a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_name"))
                .await
                .unwrap();

        let _b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        let (mut client_c, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/room_name?next=2"))
                .await
                .unwrap();

        let c_uuid = get_peer_id(recv_peer_event(&mut client_c).await);

        // Clients should be matched in pairs as they arrive, i.e. a + b and c + d
        let new_peer_c = recv_peer_event(&mut client_a).await;

        assert_eq!(new_peer_c, JsonPeerEvent::NewPeer(c_uuid));

        let timeout = time::sleep(Duration::from_millis(100));
        pin_mut!(timeout);
        select! {
            _ = client_a.next() => panic!("unexpected message"),
            _ = client_b.next() => panic!("unexpected message"),
            _ = client_c.next() => panic!("unexpected message"),
            _ = &mut timeout => {}
        }
    }

    #[tokio::test]
    async fn match_different_id_same_next() {
        let mut server = app();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=2"))
                .await
                .unwrap();

        let _a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_2?next=2"))
                .await
                .unwrap();

        let _b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        let (mut client_c, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=2"))
                .await
                .unwrap();

        let c_uuid = get_peer_id(recv_peer_event(&mut client_c).await);

        let (mut client_d, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_2?next=2"))
                .await
                .unwrap();

        let d_uuid = get_peer_id(recv_peer_event(&mut client_d).await);

        // Clients should be matched in pairs as they arrive, i.e. a + c and b + d
        let new_peer_c = recv_peer_event(&mut client_a).await;
        let new_peer_d = recv_peer_event(&mut client_b).await;

        assert_eq!(new_peer_c, JsonPeerEvent::NewPeer(c_uuid));
        assert_eq!(new_peer_d, JsonPeerEvent::NewPeer(d_uuid));

        let timeout = time::sleep(Duration::from_millis(100));
        pin_mut!(timeout);
        select! {
            _ = client_a.next() => panic!("unexpected message"),
            _ = client_b.next() => panic!("unexpected message"),
            _ = client_c.next() => panic!("unexpected message"),
            _ = client_d.next() => panic!("unexpected message"),
            _ = &mut timeout => {}
        }
    }
    #[tokio::test]
    async fn match_same_id_different_next() {
        let mut server = app();
        let addr = server.bind().unwrap();
        tokio::spawn(server.serve());

        let (mut client_a, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=2"))
                .await
                .unwrap();

        let _a_uuid = get_peer_id(recv_peer_event(&mut client_a).await);

        let (mut client_b, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=3"))
                .await
                .unwrap();

        let _b_uuid = get_peer_id(recv_peer_event(&mut client_b).await);

        let (mut client_c, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=2"))
                .await
                .unwrap();

        let c_uuid = get_peer_id(recv_peer_event(&mut client_c).await);

        let (mut client_d, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=3"))
                .await
                .unwrap();

        let d_uuid = get_peer_id(recv_peer_event(&mut client_d).await);

        let (mut client_e, _response) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/scope_1?next=3"))
                .await
                .unwrap();

        let e_uuid = get_peer_id(recv_peer_event(&mut client_e).await);

        // Clients should be matched in pairs as they arrive, i.e. a + c and (b + d ; b + e ; d + e)
        let new_peer_c = recv_peer_event(&mut client_a).await;
        let new_peer_d = recv_peer_event(&mut client_b).await;
        let new_peer_e = recv_peer_event(&mut client_b).await;
        assert_eq!(new_peer_e, JsonPeerEvent::NewPeer(e_uuid));
        let new_peer_e = recv_peer_event(&mut client_d).await;

        assert_eq!(new_peer_c, JsonPeerEvent::NewPeer(c_uuid));
        assert_eq!(new_peer_d, JsonPeerEvent::NewPeer(d_uuid));
        assert_eq!(new_peer_d, JsonPeerEvent::NewPeer(d_uuid));
        assert_eq!(new_peer_e, JsonPeerEvent::NewPeer(e_uuid));

        let timeout = time::sleep(Duration::from_millis(100));
        pin_mut!(timeout);
        select! {
            _ = client_a.next() => panic!("unexpected message"),
            _ = client_b.next() => panic!("unexpected message"),
            _ = client_c.next() => panic!("unexpected message"),
            _ = client_d.next() => panic!("unexpected message"),
            _ = client_e.next() => panic!("unexpected message"),
            _ = &mut timeout => {}
        }
    }
}
