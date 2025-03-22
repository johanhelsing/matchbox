use std::time::UNIX_EPOCH;

use iroh::{endpoint::Connection, protocol::ProtocolHandler, Endpoint, PublicKey};
use matchbox_socket::PeerEvent;
use n0_future::task;

use crate::get_timestamp;

pub const DIRECT_MESSAGE_ALPN: &[u8] = b"/matchbox-direct-message/0";
#[derive(Debug, Clone)]
pub struct DirectMessageProtocol(pub async_broadcast::Sender<(PublicKey, PeerEvent)>);

impl DirectMessageProtocol {
    async fn handle_connection(self, connection: Connection) -> anyhow::Result<()> {
        let _remote_node_id = connection.remote_node_id()?;
        let (mut send, mut recv) = connection.accept_bi().await?;
        let data = recv.read_to_end(1024*63).await?;
        send.write(b"ok").await?;
        send.finish()?;
        let data: (PeerEvent, u64) = serde_json::from_slice(&data)?;
        let data = match &data.0 {
            PeerEvent::Signal { ..} => {
                (_remote_node_id, data)
            }
            _ => {
                anyhow::bail!("unsupported event type: {:?}", data)
            }
        };
        self.0.broadcast((data.0, data.1.0)).await?;
        send.stopped().await?;
        connection.close(0u8.into(), b"done");
        Ok(())
    }   
}

impl ProtocolHandler for DirectMessageProtocol {
    fn accept(&self, connection: Connection) -> n0_future::future::Boxed<anyhow::Result<()>> {
        Box::pin(self.clone().handle_connection(connection))
    }
}
pub async fn send_direct_message(
    endpoint: &Endpoint,
    iroh_target: PublicKey,
    payload: PeerEvent,
) -> anyhow::Result<()> {
    let connection = endpoint.connect(iroh_target, DIRECT_MESSAGE_ALPN).await?;
    let payload = (payload, get_timestamp());
    let payload = serde_json::to_vec(&payload)?;
    let (mut send_stream, mut recv_stream) = connection.open_bi().await?;
    let send_task = task::spawn({
        async move {
            send_stream.write_all(&payload).await?;
            send_stream.finish()?;
            send_stream.stopped().await?;
            anyhow::Ok(())
        }
    });
    let _n = tokio::io::copy(&mut recv_stream, &mut tokio::io::sink()).await?;
    connection.close(0u8.into(), b"done");
    send_task.await??;
    Ok(())
}