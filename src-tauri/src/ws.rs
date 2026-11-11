//! Local WebSocket server on 127.0.0.1 (never reachable from the network).
//!
//! * Binds the first free port in `port_start..=port_end` (default 8765-8800).
//! * Serves ws:// and wss:// on the same port: a TLS ClientHello starts with 0x16.
//! * Rejects browser origins not in `allowed_origins` during the HTTP upgrade (403).
//! * Requires `{"type":"auth","token":"..."}` as the first message (10 s limit).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

use crate::protocol::{now_iso, secure_eq, InMsg, OutMsg, GATEWAY_NAME, VERSION};
use crate::state::Gateway;

const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Close codes in the 4000-4999 private range; the client library understands them.
const CLOSE_UNAUTHORIZED: u16 = 4401;
const CLOSE_RECONFIGURED: u16 = 4000;

pub async fn run(gw: Arc<Gateway>) {
    loop {
        match bind(&gw).await {
            Ok((listener, port)) => {
                gw.set_ws("running", Some(port), None);
                loop {
                    tokio::select! {
                        accepted = listener.accept() => match accepted {
                            Ok((stream, addr)) => {
                                if !addr.ip().is_loopback() {
                                    continue;
                                }
                                let gw = gw.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = handle_connection(gw, stream).await {
                                        tracing::debug!("connection ended: {e}");
                                    }
                                });
                            }
                            Err(e) => {
                                tracing::warn!("accept failed: {e}");
                                tokio::time::sleep(Duration::from_millis(100)).await;
                            }
                        },
                        _ = gw.ws_restart.notified() => break,
                    }
                }
                gw.set_ws("restarting", None, None);
            }
            Err(e) => {
                gw.error("ws_bind_failed", e.clone(), false);
                gw.set_ws("error", None, Some(e));
                // Retry later, or immediately when the port range is changed.
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                    _ = gw.ws_restart.notified() => {}
                }
            }
        }
    }
}

async fn bind(gw: &Gateway) -> Result<(TcpListener, u16), String> {
    let cfg = gw.config().websocket;
    for port in cfg.port_start..=cfg.port_end {
        if let Ok(l) = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await {
            if port != cfg.port_start {
                tracing::info!("port {} busy, using {port}", cfg.port_start);
            }
            return Ok((l, port));
        }
    }
    Err(format!("No free port between {} and {}", cfg.port_start, cfg.port_end))
}

async fn handle_connection(gw: Arc<Gateway>, stream: TcpStream) -> Result<(), String> {
    let _ = stream.set_nodelay(true);
    let mut first = [0u8; 1];
    let n = timeout(HANDSHAKE_TIMEOUT, stream.peek(&mut first))
        .await
        .map_err(|_| "no data".to_string())?
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(());
    }
    if first[0] == 0x16 {
        let Some(acceptor) = gw.tls_acceptor() else {
            return Err("wss:// requested but TLS is not set up".into());
        };
        let tls = timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream))
            .await
            .map_err(|_| "TLS handshake timeout".to_string())?
            .map_err(|e| format!("TLS handshake: {e}"))?;
        serve(gw, tls, true).await
    } else {
        serve(gw, stream, false).await
    }
}

fn normalize_origin(o: &str) -> String {
    o.trim().trim_end_matches('/').to_ascii_lowercase()
}

pub fn origin_allowed(allowed: &[String], origin: &str) -> bool {
    let origin = normalize_origin(origin);
    allowed.iter().any(|pattern| {
        let p = normalize_origin(pattern);
        if p == "*" {
            return true;
        }
        if let Some((scheme, rest)) = p.split_once("://*.") {
            // https://*.example.com matches https://erp.example.com (not https://example.com)
            return origin
                .strip_prefix(&format!("{scheme}://"))
                .map(|host| host.ends_with(&format!(".{rest}")))
                .unwrap_or(false);
        }
        p == origin
    })
}

async fn serve<S>(gw: Arc<Gateway>, stream: S, secure: bool) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let cfg = gw.config().websocket;
    let mut origin: Option<String> = None;
    let mut rejected = false;
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        origin = req.headers().get("origin").and_then(|v| v.to_str().ok()).map(str::to_string);
        let ok = match origin.as_deref() {
            Some(o) => origin_allowed(&cfg.allowed_origins, o),
            None => cfg.allow_missing_origin,
        };
        if ok {
            Ok(resp)
        } else {
            rejected = true;
            let mut r = ErrorResponse::new(Some("Origin not allowed".into()));
            *r.status_mut() = StatusCode::FORBIDDEN;
            Err(r)
        }
    };
    let accepted = timeout(HANDSHAKE_TIMEOUT, tokio_tungstenite::accept_hdr_async(stream, check)).await;
    let ws = match accepted {
        Ok(Ok(ws)) => ws,
        Ok(Err(e)) => {
            if rejected {
                gw.error(
                    "origin_rejected",
                    format!("Rejected connection from origin {} (add it to Allowed origins)", origin.as_deref().unwrap_or("<none>")),
                    false,
                );
            }
            return Err(format!("handshake: {e}"));
        }
        Err(_) => return Err("handshake timeout".into()),
    };

    let (mut sink, mut source) = ws.split();
    let client_id = gw.next_client_id();
    let device_id = gw.config().device_id;

    send(&mut sink, &OutMsg::Hello {
        gateway: GATEWAY_NAME,
        version: VERSION,
        device_id,
        auth_required: cfg.require_auth,
        timestamp: now_iso(),
    })
    .await?;

    if cfg.require_auth {
        let outcome = timeout(AUTH_TIMEOUT, async {
            while let Some(msg) = source.next().await {
                match msg {
                    Ok(Message::Text(t)) => {
                        return match serde_json::from_str::<InMsg>(&t) {
                            Ok(InMsg::Auth { token }) => secure_eq(&token, &cfg.token),
                            _ => false,
                        };
                    }
                    Ok(Message::Close(_)) | Err(_) => return false,
                    Ok(_) => continue,
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        if !outcome {
            gw.error(
                "auth_failed",
                format!("Client from {} failed authentication", origin.as_deref().unwrap_or("<no origin>")),
                false,
            );
            // Slow down guessing; tokens are 256-bit anyway.
            tokio::time::sleep(Duration::from_secs(1)).await;
            let _ = send(&mut sink, &OutMsg::Error {
                code: "auth_failed".into(),
                message: "Invalid or missing token".into(),
                timestamp: now_iso(),
            })
            .await;
            let _ = sink.send(close(CLOSE_UNAUTHORIZED, "Unauthorized")).await;
            return Ok(());
        }
    }

    send(&mut sink, &OutMsg::AuthOk { client_id, timestamp: now_iso() }).await?;
    // Subscribe before sending the initial snapshot so no update falls between the two.
    let mut rx = gw.tx.subscribe();
    let mut kick = gw.kick.subscribe();
    gw.client_connected(client_id, origin.clone(), secure);
    let result = client_loop(&gw, &mut sink, &mut source, &mut rx, &mut kick).await;
    gw.client_disconnected(client_id);
    result
}

async fn client_loop<Si, St>(
    gw: &Gateway,
    sink: &mut Si,
    source: &mut St,
    rx: &mut tokio::sync::broadcast::Receiver<Arc<crate::state::Outbound>>,
    kick: &mut tokio::sync::watch::Receiver<u64>,
) -> Result<(), String>
where
    Si: futures_util::Sink<Message> + Unpin,
    Si::Error: std::fmt::Display,
    St: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    for m in gw.initial_messages() {
        send(sink, &m).await?;
    }
    let mut want_raw = false;
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    if ev.raw && !want_raw {
                        continue;
                    }
                    sink.send(Message::Text(ev.json.clone())).await.map_err(|e| e.to_string())?;
                }
                // A slow client missed messages; weights are a live stream so just carry on.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return Ok(()),
            },
            msg = source.next() => match msg {
                Some(Ok(Message::Text(t))) => match serde_json::from_str::<InMsg>(&t) {
                    Ok(InMsg::Ping) => send(sink, &OutMsg::Pong { timestamp: now_iso() }).await?,
                    Ok(InMsg::Subscribe { raw }) => want_raw = raw,
                    Ok(InMsg::GetStatus) => {
                        for m in gw.initial_messages() {
                            send(sink, &m).await?;
                        }
                    }
                    Ok(InMsg::Auth { .. }) => {}
                    Err(_) => send(sink, &OutMsg::Error {
                        code: "invalid_message".into(),
                        message: "Unknown or malformed message".into(),
                        timestamp: now_iso(),
                    }).await?,
                },
                Some(Ok(Message::Close(_))) | None => return Ok(()),
                Some(Err(e)) => return Err(e.to_string()),
                Some(Ok(_)) => {}
            },
            _ = kick.changed() => {
                let _ = sink.send(close(CLOSE_RECONFIGURED, "Gateway configuration changed")).await;
                return Ok(());
            }
        }
    }
}

async fn send<Si>(sink: &mut Si, msg: &OutMsg) -> Result<(), String>
where
    Si: futures_util::Sink<Message> + Unpin,
    Si::Error: std::fmt::Display,
{
    let json = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    sink.send(Message::Text(json)).await.map_err(|e| e.to_string())
}

fn close(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: reason.into() }))
}

#[cfg(test)]
mod tests {
    use super::origin_allowed;

    #[test]
    fn origins() {
        let allowed = vec!["https://erp.example.com/".to_string(), "https://*.barkat.com".to_string()];
        assert!(origin_allowed(&allowed, "https://ERP.example.com"));
        assert!(!origin_allowed(&allowed, "http://erp.example.com"));
        assert!(origin_allowed(&allowed, "https://mill.barkat.com"));
        assert!(!origin_allowed(&allowed, "https://barkat.com"));
        assert!(!origin_allowed(&allowed, "https://evilbarkat.com"));
        assert!(!origin_allowed(&allowed, "https://erp.example.com.evil.io"));
    }
}
