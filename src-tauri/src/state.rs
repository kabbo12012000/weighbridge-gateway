//! Shared gateway state: the serial worker, WebSocket server and UI all go through `Gateway`.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use rand::{rngs::OsRng, RngCore};
use serde::Serialize;
use tokio::sync::{broadcast, watch, Notify};
use tokio_rustls::TlsAcceptor;

use crate::config::AppConfig;
use crate::frame::Frame;
use crate::protocol::{now_iso, OutMsg, WeightMsg, VERSION};
use crate::serial::SerialWorker;

const MAX_RAW: usize = 200;
const MAX_ERRORS: usize = 100;

pub struct Paths {
    pub config_file: PathBuf,
    pub tls_dir: PathBuf,
    pub log_dir: PathBuf,
}

/// A serialized message ready for every connected client.
pub struct Outbound {
    pub raw: bool,
    pub json: String,
}

#[derive(Clone, Serialize)]
pub struct SerialState {
    pub status: String,
    pub source: String,
    pub port: String,
    pub message: Option<String>,
    pub since: String,
}

#[derive(Clone, Serialize)]
pub struct WsState {
    pub status: String,
    pub port: Option<u16>,
    pub url: Option<String>,
    pub secure_url: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct ClientInfo {
    pub id: u64,
    pub origin: Option<String>,
    pub secure: bool,
    pub connected_at: String,
}

#[derive(Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Serialize)]
pub struct RawEntry {
    pub timestamp: String,
    pub text: String,
    pub hex: String,
    pub status: String,
    /// Parse outcome shown in the monitor, e.g. "25450 kg stable" or the parse error.
    pub result: Option<String>,
}

#[derive(Clone, Default, Serialize)]
pub struct Stats {
    pub bytes: u64,
    pub frames: u64,
    pub weights: u64,
    pub parse_failures: u64,
}

#[derive(Clone, Serialize)]
pub struct PathsInfo {
    pub config_file: String,
    pub log_dir: String,
    pub tls_dir: String,
}

#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub version: &'static str,
    pub device_id: String,
    pub serial: SerialState,
    pub ws: WsState,
    pub clients: Vec<ClientInfo>,
    pub last_data: Option<String>,
    pub last_data_at: Option<String>,
    pub last_weight: Option<WeightMsg>,
    pub errors: Vec<LogEntry>,
    pub raw: Vec<RawEntry>,
    pub stats: Stats,
    pub tls_configured: bool,
    pub paths: PathsInfo,
    pub config_warning: Option<String>,
}

struct Status {
    serial: SerialState,
    ws: WsState,
    clients: Vec<ClientInfo>,
    last_data: Option<String>,
    last_data_at: Option<String>,
    last_weight: Option<WeightMsg>,
    errors: VecDeque<LogEntry>,
    raw: VecDeque<RawEntry>,
    stats: Stats,
    config_warning: Option<String>,
}

pub struct Gateway {
    pub paths: Paths,
    config: RwLock<AppConfig>,
    status: Mutex<Status>,
    pub tx: broadcast::Sender<Arc<Outbound>>,
    /// Bumped to disconnect every client (token/origin change); they must reconnect and re-authenticate.
    pub kick: watch::Sender<u64>,
    /// Makes the WebSocket server re-bind (port range change).
    pub ws_restart: Notify,
    serial: Mutex<Option<SerialWorker>>,
    tls: RwLock<Option<TlsAcceptor>>,
    boot_id: String,
    reading_seq: AtomicU64,
    client_seq: AtomicU64,
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Gateway {
    pub fn new(paths: Paths, config: AppConfig, config_warning: Option<String>) -> Arc<Self> {
        let (tx, _) = broadcast::channel(512);
        let (kick, _) = watch::channel(0);
        let mut b = [0u8; 4];
        OsRng.fill_bytes(&mut b);
        let serial = SerialState {
            status: "disconnected".into(),
            source: config.serial.source.clone(),
            port: config.serial.port.clone(),
            message: None,
            since: now_iso(),
        };
        let gw = Arc::new(Self {
            paths,
            config: RwLock::new(config),
            status: Mutex::new(Status {
                serial,
                ws: WsState { status: "starting".into(), port: None, url: None, secure_url: None, message: None },
                clients: Vec::new(),
                last_data: None,
                last_data_at: None,
                last_weight: None,
                errors: VecDeque::new(),
                raw: VecDeque::new(),
                stats: Stats::default(),
                config_warning: config_warning.clone(),
            }),
            tx,
            kick,
            ws_restart: Notify::new(),
            serial: Mutex::new(None),
            tls: RwLock::new(None),
            boot_id: hex::encode(b),
            reading_seq: AtomicU64::new(0),
            client_seq: AtomicU64::new(0),
        });
        if let Some(w) = config_warning {
            gw.error("config", w, false);
        }
        gw
    }

    // ---- configuration -------------------------------------------------------------

    pub fn config(&self) -> AppConfig {
        self.config.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn update_config(&self, cfg: AppConfig) -> Result<(), String> {
        cfg.save(&self.paths.config_file)?;
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = cfg;
        lock(&self.status).config_warning = None;
        Ok(())
    }

    // ---- broadcasting --------------------------------------------------------------

    pub fn publish(&self, msg: &OutMsg) {
        let raw = matches!(msg, OutMsg::SerialData { .. });
        match serde_json::to_string(msg) {
            Ok(json) => {
                // Err only means nobody is listening.
                let _ = self.tx.send(Arc::new(Outbound { raw, json }));
            }
            Err(e) => tracing::error!("serialize message: {e}"),
        }
    }

    /// Records an error for the UI and log; optionally broadcasts it to WebSocket clients.
    pub fn error(&self, code: &str, message: impl Into<String>, broadcast: bool) {
        let message = message.into();
        tracing::warn!(code, "{message}");
        let entry = LogEntry { timestamp: now_iso(), code: code.into(), message: message.clone() };
        {
            let mut s = lock(&self.status);
            s.errors.push_front(entry.clone());
            s.errors.truncate(MAX_ERRORS);
        }
        if broadcast {
            self.publish(&OutMsg::Error { code: entry.code, message, timestamp: entry.timestamp });
        }
    }

    // ---- serial --------------------------------------------------------------------

    pub fn set_serial(&self, status: &str, message: Option<String>) {
        let cfg = self.config();
        let msg = {
            let mut s = lock(&self.status);
            if s.serial.status == status && s.serial.message == message && s.serial.port == cfg.serial.port {
                return;
            }
            s.serial = SerialState {
                status: status.into(),
                source: cfg.serial.source.clone(),
                port: cfg.serial.port.clone(),
                message: message.clone(),
                since: now_iso(),
            };
            Self::serial_msg(&s.serial)
        };
        tracing::info!(status, port = cfg.serial.port.as_str(), "serial {}", message.as_deref().unwrap_or(""));
        self.publish(&msg);
    }

    fn serial_msg(s: &SerialState) -> OutMsg {
        OutMsg::SerialStatus {
            status: s.status.clone(),
            source: s.source.clone(),
            port: s.port.clone(),
            message: s.message.clone(),
            timestamp: now_iso(),
        }
    }

    pub fn serial_status(&self) -> String {
        lock(&self.status).serial.status.clone()
    }

    pub fn record_frame(&self, frame: &Frame, result: Option<String>) {
        let ts = now_iso();
        let text = frame.printable();
        let hex = frame.hex();
        {
            let mut s = lock(&self.status);
            s.stats.frames += 1;
            s.last_data = Some(text.clone());
            s.last_data_at = Some(ts.clone());
            s.raw.push_front(RawEntry {
                timestamp: ts.clone(),
                text: text.clone(),
                hex: hex.clone(),
                status: frame.status.as_str().into(),
                result,
            });
            s.raw.truncate(MAX_RAW);
        }
        let device_id = self.config().device_id;
        self.publish(&OutMsg::SerialData {
            device_id,
            raw_data: text,
            hex,
            frame_status: frame.status.as_str().into(),
            timestamp: ts,
        });
    }

    pub fn add_bytes(&self, n: usize) {
        lock(&self.status).stats.bytes += n as u64;
    }

    pub fn add_parse_failure(&self) {
        lock(&self.status).stats.parse_failures += 1;
    }

    pub fn next_reading_id(&self) -> String {
        format!("{}-{}", self.boot_id, self.reading_seq.fetch_add(1, Ordering::Relaxed) + 1)
    }

    pub fn record_weight(&self, msg: WeightMsg) {
        let out = OutMsg::Weight(msg.clone());
        {
            let mut s = lock(&self.status);
            s.stats.weights += 1;
            s.last_weight = Some(msg);
        }
        self.publish(&out);
    }

    pub fn start_serial(self: &Arc<Self>) {
        self.stop_serial();
        let worker = crate::serial::spawn(self.clone());
        *lock(&self.serial) = Some(worker);
    }

    pub fn stop_serial(&self) {
        let worker = lock(&self.serial).take();
        if let Some(w) = worker {
            w.stop();
            self.set_serial("disconnected", Some("Disconnected by operator".into()));
        }
    }

    pub fn serial_running(&self) -> bool {
        lock(&self.serial).as_ref().map(|w| w.is_running()).unwrap_or(false)
    }

    // ---- websocket -----------------------------------------------------------------

    pub fn set_ws(&self, status: &str, port: Option<u16>, message: Option<String>) {
        let secure = self.tls_acceptor().is_some();
        {
            let mut s = lock(&self.status);
            s.ws = WsState {
                status: status.into(),
                port,
                url: port.map(|p| format!("ws://127.0.0.1:{p}")),
                secure_url: port.filter(|_| secure).map(|p| format!("wss://127.0.0.1:{p}")),
                message: message.clone(),
            };
        }
        tracing::info!(status, ?port, "websocket {}", message.as_deref().unwrap_or(""));
        if status == "running" {
            self.publish(&self.gateway_status_msg("snapshot"));
        }
    }

    pub fn gateway_status_msg(&self, event: &str) -> OutMsg {
        let s = lock(&self.status);
        OutMsg::GatewayStatus {
            status: s.ws.status.clone(),
            event: event.into(),
            url: s.ws.url.clone().unwrap_or_default(),
            secure_url: s.ws.secure_url.clone(),
            port: s.ws.port.unwrap_or(0),
            clients: s.clients.len(),
            device_id: self.config().device_id,
            version: VERSION,
            timestamp: now_iso(),
        }
    }

    pub fn next_client_id(&self) -> u64 {
        self.client_seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn client_connected(&self, id: u64, origin: Option<String>, secure: bool) {
        tracing::info!(id, origin = origin.as_deref().unwrap_or("-"), secure, "client connected");
        lock(&self.status).clients.push(ClientInfo { id, origin, secure, connected_at: now_iso() });
        self.publish(&self.gateway_status_msg("client_connected"));
    }

    pub fn client_disconnected(&self, id: u64) {
        tracing::info!(id, "client disconnected");
        lock(&self.status).clients.retain(|c| c.id != id);
        self.publish(&self.gateway_status_msg("client_disconnected"));
    }

    /// What a client receives right after authenticating, so it never waits for the next change.
    pub fn initial_messages(&self) -> Vec<OutMsg> {
        let mut v = vec![self.gateway_status_msg("snapshot")];
        let s = lock(&self.status);
        v.push(Self::serial_msg(&s.serial));
        if let Some(w) = &s.last_weight {
            v.push(OutMsg::Weight(w.clone()));
        }
        v
    }

    pub fn heartbeat_msg(&self) -> OutMsg {
        let s = lock(&self.status);
        OutMsg::Heartbeat {
            device_id: self.config().device_id,
            serial_status: s.serial.status.clone(),
            last_data_at: s.last_data_at.clone(),
            clients: s.clients.len(),
            timestamp: now_iso(),
        }
    }

    pub fn kick_clients(&self) {
        self.kick.send_modify(|v| *v += 1);
    }

    pub fn tls_acceptor(&self) -> Option<TlsAcceptor> {
        self.tls.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_tls(&self, acceptor: Option<TlsAcceptor>) {
        *self.tls.write().unwrap_or_else(|e| e.into_inner()) = acceptor;
        let (status, port, message) = {
            let s = lock(&self.status);
            (s.ws.status.clone(), s.ws.port, s.ws.message.clone())
        };
        self.set_ws(&status, port, message);
    }

    // ---- UI ------------------------------------------------------------------------

    pub fn snapshot(&self) -> Snapshot {
        let tls_configured = self.tls_acceptor().is_some();
        let device_id = self.config().device_id;
        let s = lock(&self.status);
        Snapshot {
            version: VERSION,
            device_id,
            serial: s.serial.clone(),
            ws: s.ws.clone(),
            clients: s.clients.clone(),
            last_data: s.last_data.clone(),
            last_data_at: s.last_data_at.clone(),
            last_weight: s.last_weight.clone(),
            errors: s.errors.iter().cloned().collect(),
            raw: s.raw.iter().cloned().collect(),
            stats: s.stats.clone(),
            tls_configured,
            paths: PathsInfo {
                config_file: self.paths.config_file.display().to_string(),
                log_dir: self.paths.log_dir.display().to_string(),
                tls_dir: self.paths.tls_dir.display().to_string(),
            },
            config_warning: s.config_warning.clone(),
        }
    }

    pub fn clear_monitor(&self) {
        let mut s = lock(&self.status);
        s.raw.clear();
        s.errors.clear();
    }
}
