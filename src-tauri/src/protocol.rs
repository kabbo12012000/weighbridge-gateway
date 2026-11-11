//! JSON messages exchanged with WebSocket clients. Every outbound message has a `type` field.

use chrono::{SecondsFormat, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

pub const GATEWAY_NAME: &str = "weighbridge-gateway";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[derive(Clone, Debug, Serialize)]
pub struct WeightMsg {
    pub device_id: String,
    /// Unique per reading ("<boot id>-<sequence>"); Laravel stores it with a unique index.
    pub reading_id: String,
    pub weight: f64,
    pub unit: String,
    pub stable: bool,
    /// "indicator" when the device reported it, "motion" when derived from consecutive readings.
    pub stability_source: String,
    pub weight_type: String,
    pub raw_data: String,
    pub timestamp: String,
    /// HMAC-SHA256 (hex) of `signing_payload()`, keyed with the gateway's signing secret.
    pub signature: String,
}

impl WeightMsg {
    /// Canonical string Laravel recomputes: device_id|reading_id|weight(3dp)|unit|stable(1/0)|timestamp
    pub fn signing_payload(&self) -> String {
        format!(
            "{}|{}|{:.3}|{}|{}|{}",
            self.device_id,
            self.reading_id,
            self.weight,
            self.unit,
            if self.stable { 1 } else { 0 },
            self.timestamp
        )
    }

    pub fn sign(&mut self, secret: &str) {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
        mac.update(self.signing_payload().as_bytes());
        self.signature = hex::encode(mac.finalize().into_bytes());
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutMsg {
    /// Sent immediately after the WebSocket opens (before auth) so clients can identify the gateway.
    Hello {
        gateway: &'static str,
        version: &'static str,
        device_id: String,
        auth_required: bool,
        timestamp: String,
    },
    AuthOk {
        client_id: u64,
        timestamp: String,
    },
    GatewayStatus {
        status: String,
        /// "snapshot" | "client_connected" | "client_disconnected"
        event: String,
        url: String,
        secure_url: Option<String>,
        port: u16,
        clients: usize,
        device_id: String,
        version: &'static str,
        timestamp: String,
    },
    SerialStatus {
        /// "connected" | "connecting" | "reconnecting" | "disconnected" | "error"
        status: String,
        source: String,
        port: String,
        message: Option<String>,
        timestamp: String,
    },
    Weight(WeightMsg),
    /// Every frame received, parsed or not. Only sent to clients that subscribed with raw=true.
    SerialData {
        device_id: String,
        raw_data: String,
        hex: String,
        frame_status: String,
        timestamp: String,
    },
    Error {
        code: String,
        message: String,
        timestamp: String,
    },
    Heartbeat {
        device_id: String,
        serial_status: String,
        last_data_at: Option<String>,
        clients: usize,
        timestamp: String,
    },
    Pong {
        timestamp: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InMsg {
    Auth { token: String },
    Ping,
    Subscribe {
        #[serde(default)]
        raw: bool,
    },
    GetStatus,
}

/// Constant-time comparison so the token cannot be guessed byte by byte from response timing.
pub fn secure_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_matches_reference() {
        let mut m = WeightMsg {
            device_id: "WB-001".into(),
            reading_id: "a1b2c3d4-42".into(),
            weight: 25450.0,
            unit: "kg".into(),
            stable: true,
            stability_source: "indicator".into(),
            weight_type: "gross".into(),
            raw_data: "ST,GS,25450kg".into(),
            timestamp: "2026-11-11T12:30:00.000Z".into(),
            signature: String::new(),
        };
        assert_eq!(m.signing_payload(), "WB-001|a1b2c3d4-42|25450.000|kg|1|2026-11-11T12:30:00.000Z");
        m.sign("secret");
        assert_eq!(m.signature.len(), 64);
    }

    #[test]
    fn weight_message_shape() {
        let m = OutMsg::Pong { timestamp: "t".into() };
        assert_eq!(serde_json::to_string(&m).unwrap(), r#"{"type":"pong","timestamp":"t"}"#);
    }

    #[test]
    fn inbound_auth() {
        let m: InMsg = serde_json::from_str(r#"{"type":"auth","token":"x"}"#).unwrap();
        assert!(matches!(m, InMsg::Auth { token } if token == "x"));
    }

    #[test]
    fn secure_eq_works() {
        assert!(secure_eq("abc", "abc"));
        assert!(!secure_eq("abc", "abd"));
        assert!(!secure_eq("abc", "ab"));
    }
}
