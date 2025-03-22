
use iroh::{endpoint::Connection, protocol::ProtocolHandler, Endpoint, PublicKey};
use matchbox_socket::PeerEvent;
use n0_future::task;

use crate::get_timestamp;

pub const DIRECT_MESSAGE_ALPN: &[u8] = b"/matchbox-direct-message/0";
#[derive(Debug, Clone)]
pub struct DirectMessageProtocol(pub async_broadcast::Sender<(PublicKey, PeerEvent)>);

type DirectMessage = (PeerEvent, u128);

impl DirectMessageProtocol {
    async fn handle_connection(self, connection: Connection) -> anyhow::Result<()> {
        let _remote_node_id = connection.remote_node_id()?;
        let mut recv = connection.accept_uni().await?;
        let data = recv.read_to_end(1024*63).await?;
        let _  = recv.stop(iroh::endpoint::VarInt::from(0_u8));
        connection.close(0u8.into(), b"done");
        let data: DirectMessage = serde_json::from_slice(&data)?;
        let data = match &data.0 {
            PeerEvent::Signal { ..} => {
                (_remote_node_id, data)
            }
            _ => {
                anyhow::bail!("unsupported event type: {:?}", data)
            }
        };
        self.0.broadcast((data.0, data.1.0)).await?;
        Ok(())
    }   
}

impl ProtocolHandler for DirectMessageProtocol {
    fn accept(&self, connection: Connection) -> n0_future::boxed::BoxFuture<anyhow::Result<()>> {
        Box::pin(self.clone().handle_connection(connection))
    }
}
pub async fn send_direct_message(
    endpoint: &Endpoint,
    iroh_target: PublicKey,
    payload: PeerEvent,
) -> anyhow::Result<()> {
    let connection = endpoint.connect(iroh_target, DIRECT_MESSAGE_ALPN).await?;
    let payload: DirectMessage = (payload, get_timestamp());
    let payload = serde_json::to_vec(&payload)?;
    let mut send_stream = connection.open_uni().await?;
    send_stream.write_all(&payload).await?;
    send_stream.finish()?;
    connection.closed().await;
    connection.close(0u8.into(), b"done");
    Ok(())
}