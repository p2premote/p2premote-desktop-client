//! Bridge the shared punch signal engine to the existing authorized WS client.
use crate::ws::ServiceWsClient;
use anyhow::{anyhow, Result};
use p2premote_punch::easyp2p::signal::{
    ExternalSignalSession, SendFuture, SessionIdentity, TransportOffer, WireSender,
};
use std::{sync::Arc, time::Instant};

pub fn local_offer() -> TransportOffer {
    // The transition build can disable WS for a new attempt before negotiation.
    // A selected WS attempt never falls back to MQTT on a transport error.
    TransportOffer::local(std::env::var("P2PREMOTE_SIGNAL_TRANSPORT").as_deref() != Ok("mqtt"))
}

struct WsSender {
    ws: ServiceWsClient,
    generation: u64,
    connection: String,
    peer: i64,
    grant: String,
}
impl WireSender for WsSender {
    fn send(&self, data: String, deadline: Instant) -> SendFuture<'_> {
        Box::pin(async move {
            self.ws
                .send_p2p_notify_for_generation(
                    self.connection.clone(),
                    self.peer,
                    uuid::Uuid::new_v4().to_string(),
                    self.grant.clone(),
                    data,
                    self.generation,
                    deadline,
                )
                .await
                .map_err(|error| {
                    if error == "p2p_notify_ack_timeout" {
                        "signal_server_ack_timeout".into()
                    } else {
                        error
                    }
                })
        })
    }
}

pub fn create_session(
    ws: ServiceWsClient,
    connection: String,
    attempt: String,
    local: i64,
    peer: i64,
    grant: String,
) -> Result<Arc<ExternalSignalSession>> {
    let generation = ws.generation();
    if generation == 0 {
        return Err(anyhow!("signal_connection_lost"));
    }
    let sender = Arc::new(WsSender {
        ws,
        generation,
        connection: connection.clone(),
        peer,
        grant,
    });
    ExternalSignalSession::new(
        SessionIdentity {
            connection_id: connection,
            attempt_id: attempt,
            local_device_id: local,
            peer_device_id: peer,
        },
        sender,
    )
    .map_err(|e| anyhow!(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ws::{WsEvent, WsMessage};
    use futures_util::{SinkExt, StreamExt};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::{mpsc, Mutex};
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    #[tokio::test]
    async fn actual_websocket_configuration_exchange_uses_existing_server_ack_only() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let peers = Arc::new(Mutex::new(HashMap::<i64, mpsc::Sender<Message>>::new()));
        let messages = Arc::new(AtomicUsize::new(0));
        let server = tokio::spawn({
            let peers = peers.clone();
            let messages = messages.clone();
            async move {
                let mut workers = tokio::task::JoinSet::new();
                for source in 1..=2 {
                    let (socket, _) = listener.accept().await.unwrap();
                    let stream = accept_async(socket).await.unwrap();
                    let (mut write, mut read) = stream.split();
                    let (tx, mut rx) = mpsc::channel(8);
                    peers.lock().await.insert(source, tx);
                    let peers = peers.clone();
                    let messages = messages.clone();
                    workers.spawn(async move {
                        loop {
                            tokio::select! {
                                outgoing = rx.recv() => match outgoing {
                                    Some(message) => { if write.send(message).await.is_err() { break; } },
                                    None => break,
                                },
                                incoming = read.next() => match incoming {
                                    Some(Ok(Message::Ping(data))) => { if write.send(Message::Pong(data)).await.is_err() { break; } },
                                    Some(Ok(Message::Text(data))) => {
                                        let message: WsMessage = serde_json::from_str(&data).unwrap();
                                        if let WsMessage::P2PNotify { connection_id, target_device_id, message_id, access_grant, data, .. } = message {
                                            messages.fetch_add(1, Ordering::Relaxed);
                                            let target = peers.lock().await.get(&target_device_id).cloned().unwrap();
                                            let forwarded = WsMessage::P2PNotify { connection_id, source_device_id: source, target_device_id, message_id: message_id.clone(), access_grant, data };
                                            target.send(Message::Text(serde_json::to_string(&forwarded).unwrap().into())).await.unwrap();
                                            let ack = WsMessage::P2PNotifyAck { message_id, accepted: true, error: None };
                                            write.send(Message::Text(serde_json::to_string(&ack).unwrap().into())).await.unwrap();
                                        } else { panic!("unexpected client message"); }
                                    },
                                    _ => break,
                                }
                            }
                        }
                    });
                }
                while workers.join_next().await.is_some() {}
            }
        });
        let left = ServiceWsClient::new();
        let right = ServiceWsClient::new();
        let (ltx, mut lrx) = mpsc::unbounded_channel();
        let (rtx, mut rrx) = mpsc::unbounded_channel();
        left.connect(&url, "synthetic", 1, "left", ltx)
            .await
            .unwrap();
        right
            .connect(&url, "synthetic", 2, "right", rtx)
            .await
            .unwrap();
        let a = create_session(
            left.clone(),
            "connection".into(),
            "attempt".into(),
            1,
            2,
            "synthetic".into(),
        )
        .unwrap();
        let b = create_session(
            right.clone(),
            "connection".into(),
            "attempt".into(),
            2,
            1,
            "synthetic".into(),
        )
        .unwrap();
        let ac = a
            .register_call("wg-config", "abcdefghijklmnop-kx", Duration::from_secs(5))
            .unwrap();
        let bc = b
            .register_call("wg-config", "abcdefghijklmnop-kx", Duration::from_secs(5))
            .unwrap();
        let receivers = [
            tokio::spawn(async move {
                while let Some(event) = lrx.recv().await {
                    if let WsEvent::P2PNotify {
                        connection_id,
                        source_device_id,
                        data,
                        ..
                    } = event
                    {
                        a.deliver_json(&connection_id, source_device_id, &data)
                            .unwrap();
                    }
                }
            }),
            tokio::spawn(async move {
                while let Some(event) = rrx.recv().await {
                    if let WsEvent::P2PNotify {
                        connection_id,
                        source_device_id,
                        data,
                        ..
                    } = event
                    {
                        b.deliver_json(&connection_id, source_device_id, &data)
                            .unwrap();
                    }
                }
            }),
        ];
        let scope = ac.scope(Duration::from_secs(5));
        let request = "active config".to_string();
        let active = ac.exchange(0, &request, "wg_config", 0, &scope, scope.remaining());
        let passive = async {
            let received: String = bc
                .exchange(1, &String::new(), "wg_config", 0, &scope, scope.remaining())
                .await
                .unwrap();
            assert_eq!(received, "active config");
            bc.exchange(
                2,
                &"passive config".to_string(),
                "wg_config",
                0,
                &scope,
                scope.remaining(),
            )
            .await
            .unwrap()
        };
        let (response, original): (_, String) = tokio::join!(active, passive);
        assert_eq!(response.unwrap(), "passive config");
        assert_eq!(original, "active config");
        assert_eq!(messages.load(Ordering::Relaxed), 2);
        left.disconnect().await;
        right.disconnect().await;
        for task in receivers {
            task.abort();
            let _ = task.await;
        }
        server.abort();
        let _ = server.await;
    }
}
