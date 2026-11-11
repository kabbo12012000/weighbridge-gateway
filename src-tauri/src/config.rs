//! Persistent gateway configuration (JSON in the per-user app config directory).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppConfig {
    /// Identifies this weighbridge to Laravel (e.g. "WB-001").
    pub device_id: String,
    pub serial: SerialConfig,
    pub parser: ParserConfig,
    pub websocket: WsConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SerialConfig {
    /// "serial" = real COM port, "simulator" = built-in test signal (no hardware needed).
    pub source: String,
    pub port: String,
    pub baud_rate: u32,
    /// 5 | 6 | 7 | 8
    pub data_bits: u8,
    /// 1 | 2
    pub stop_bits: u8,
    /// "none" | "odd" | "even"
    pub parity: String,
    /// "none" | "software" | "hardware"
    pub flow_control: String,
    /// Open the port when the application starts.
    pub auto_connect: bool,
    /// Re-open the port after an unexpected disconnect or failed open.
    pub auto_reconnect: bool,
    pub reconnect_min_ms: u64,
    pub reconnect_max_ms: u64,
    /// Frame terminator: "AUTO" (CR, LF or CRLF) | "CR" | "LF" | "CRLF" | "ETX"
    pub delimiter: String,
    /// Bytes without a terminator are emitted as an "incomplete" frame after this silence (0 = never).
    pub idle_flush_ms: u64,
    pub max_frame_bytes: usize,
    /// Report a "no data" warning after this long without bytes while connected (0 = off).
    pub silence_timeout_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ParserConfig {
    /// "auto" | "demo" | "regex" | "raw"
    pub mode: String,
    /// Used when mode = "regex". Named groups: weight (required), sign, unit, stability, type.
    pub regex: String,
    /// Reverse the captured weight digits (some indicators transmit least-significant digit first).
    pub reverse_digits: bool,
    /// Multiply the parsed number (e.g. 10 when the indicator counts in 10 kg divisions).
    pub scale: f64,
    pub default_unit: String,
    pub stable_codes: Vec<String>,
    pub unstable_codes: Vec<String>,
    pub overload_codes: Vec<String>,
    pub type_codes: BTreeMap<String, String>,
    /// When the indicator does not report stability: the weight must stay within
    /// `motion_tolerance` for this long to be reported stable.
    pub motion_window_ms: u64,
    pub motion_tolerance: f64,
    /// Readings with |weight| above this are rejected as invalid.
    pub max_weight: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WsConfig {
    pub port_start: u16,
    pub port_end: u16,
    /// Exact origins ("https://erp.example.com") or wildcard subdomains ("https://*.example.com").
    pub allowed_origins: Vec<String>,
    /// Non-browser clients send no Origin header. They still need the token.
    pub allow_missing_origin: bool,
    pub require_auth: bool,
    /// Shared with the browser app; sent in the first WebSocket message.
    pub token: String,
    /// Never leaves this PC except to Laravel's device config; signs every weight reading.
    pub signing_secret: String,
    pub heartbeat_ms: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            device_id: "WB-001".into(),
            serial: SerialConfig::default(),
            parser: ParserConfig::default(),
            websocket: WsConfig::default(),
        }
    }
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            source: "serial".into(),
            port: "COM2".into(),
            baud_rate: 9600,
            data_bits: 8,
            stop_bits: 1,
            parity: "none".into(),
            flow_control: "none".into(),
            auto_connect: true,
            auto_reconnect: true,
            reconnect_min_ms: 1000,
            reconnect_max_ms: 30000,
            delimiter: "AUTO".into(),
            idle_flush_ms: 1000,
            max_frame_bytes: 1024,
            silence_timeout_ms: 10000,
        }
    }
}

impl Default for ParserConfig {
    fn default() -> Self {
        let type_codes = [
            ("G", "gross"),
            ("GS", "gross"),
            ("N", "net"),
            ("NT", "net"),
            ("T", "tare"),
            ("TR", "tare"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        Self {
            mode: "auto".into(),
            regex: String::new(),
            reverse_digits: false,
            scale: 1.0,
            default_unit: "kg".into(),
            stable_codes: vec!["ST".into(), "S".into()],
            unstable_codes: vec!["US".into(), "U".into(), "MO".into()],
            overload_codes: vec!["OL".into(), "OVER".into()],
            type_codes,
            motion_window_ms: 1500,
            motion_tolerance: 0.0,
            max_weight: 200_000.0,
        }
    }
}

impl Default for WsConfig {
    fn default() -> Self {
        Self {
            port_start: 8765,
            port_end: 8800,
            allowed_origins: vec![
                "http://localhost:5173".into(),
                "http://127.0.0.1:5173".into(),
                "http://localhost:8000".into(),
            ],
            allow_missing_origin: true,
            require_auth: true,
            token: String::new(),
            signing_secret: String::new(),
            heartbeat_ms: 5000,
        }
    }
}

pub fn random_secret() -> String {
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    hex::encode(b)
}

impl AppConfig {
    /// Loads the config, falling back to defaults, and fills in missing secrets.
    /// Returns the config and a warning to show the operator, if any.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let mut warning = None;
        let mut cfg = match fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<AppConfig>(&text) {
                Ok(c) => c,
                Err(e) => {
                    // Keep the broken file for inspection instead of silently overwriting it.
                    let _ = fs::copy(path, path.with_extension("json.invalid"));
                    warning = Some(format!("Config file was invalid ({e}); defaults loaded"));
                    AppConfig::default()
                }
            },
            Err(_) => AppConfig::default(),
        };
        let mut dirty = !path.exists() || warning.is_some();
        if cfg.websocket.token.len() < 32 {
            cfg.websocket.token = random_secret();
            dirty = true;
        }
        if cfg.websocket.signing_secret.len() < 32 {
            cfg.websocket.signing_secret = random_secret();
            dirty = true;
        }
        if dirty {
            if let Err(e) = cfg.save(path) {
                warning = Some(format!("Could not write config: {e}"));
            }
        }
        (cfg, warning)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        // Write-then-rename so a crash never leaves a half-written config.
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json).map_err(|e| e.to_string())?;
        fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn validate(&self) -> Result<(), String> {
        let d = self.device_id.trim();
        if d.is_empty() || d.len() > 64 {
            return Err("Device ID must be 1-64 characters".into());
        }
        let s = &self.serial;
        if !["serial", "simulator"].contains(&s.source.as_str()) {
            return Err("Source must be serial or simulator".into());
        }
        if s.source == "serial" && s.port.trim().is_empty() {
            return Err("Select a serial port".into());
        }
        if s.baud_rate < 110 || s.baud_rate > 921_600 {
            return Err("Baud rate must be between 110 and 921600".into());
        }
        if !(5..=8).contains(&s.data_bits) {
            return Err("Data bits must be 5, 6, 7 or 8".into());
        }
        if ![1, 2].contains(&s.stop_bits) {
            return Err("Stop bits must be 1 or 2".into());
        }
        if !["none", "odd", "even"].contains(&s.parity.as_str()) {
            return Err("Parity must be none, odd or even".into());
        }
        if !["none", "software", "hardware"].contains(&s.flow_control.as_str()) {
            return Err("Flow control must be none, software or hardware".into());
        }
        if !["AUTO", "CR", "LF", "CRLF", "ETX"].contains(&s.delimiter.as_str()) {
            return Err("Delimiter must be AUTO, CR, LF, CRLF or ETX".into());
        }
        if s.reconnect_min_ms < 200 || s.reconnect_max_ms < s.reconnect_min_ms {
            return Err("Reconnect delays are invalid".into());
        }
        if s.max_frame_bytes < 8 || s.max_frame_bytes > 65_536 {
            return Err("Max frame bytes must be 8-65536".into());
        }
        // Builds the parser, which validates the regex and its named groups.
        crate::parser::Parser::new(&self.parser)?;
        let p = &self.parser;
        if !(p.scale.is_finite() && p.scale > 0.0) {
            return Err("Scale must be a positive number".into());
        }
        if !(p.max_weight.is_finite() && p.max_weight > 0.0) {
            return Err("Max weight must be a positive number".into());
        }
        if !(p.motion_tolerance.is_finite() && p.motion_tolerance >= 0.0) {
            return Err("Motion tolerance must be zero or positive".into());
        }
        let w = &self.websocket;
        if w.port_start < 1024 || w.port_end < w.port_start {
            return Err("WebSocket port range is invalid (ports must be >= 1024, start <= end)".into());
        }
        if w.require_auth && w.token.len() < 32 {
            return Err("Token must be at least 32 characters".into());
        }
        if w.signing_secret.len() < 32 {
            return Err("Signing secret must be at least 32 characters".into());
        }
        if w.heartbeat_ms < 1000 {
            return Err("Heartbeat must be at least 1000 ms".into());
        }
        for o in &w.allowed_origins {
            let o = o.trim();
            if o != "*" && !(o.starts_with("http://") || o.starts_with("https://") || o.starts_with("tauri://")) {
                return Err(format!("Allowed origin \"{o}\" must start with http:// or https://"));
            }
        }
        Ok(())
    }
}
