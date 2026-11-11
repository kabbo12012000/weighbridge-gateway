//! Serial worker: runs on its own OS thread (serialport is blocking), opens the port,
//! splits the stream into frames, parses weights and reconnects with backoff.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;
use serialport::{DataBits, FlowControl, Parity, SerialPort, SerialPortType, StopBits};

use crate::config::{AppConfig, SerialConfig};
use crate::frame::{Delimiter, Frame, FrameBuffer, FrameStatus};
use crate::motion::MotionDetector;
use crate::parser::{ParseError, Parser, Stability};
use crate::protocol::{now_iso, WeightMsg};
use crate::state::Gateway;

const READ_TIMEOUT: Duration = Duration::from_millis(100);
const PRESENCE_CHECK: Duration = Duration::from_secs(2);
const ERROR_THROTTLE: Duration = Duration::from_secs(10);

pub struct SerialWorker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl SerialWorker {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }

    pub fn is_running(&self) -> bool {
        self.handle.as_ref().map(|h| !h.is_finished()).unwrap_or(false)
    }
}

impl Drop for SerialWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[derive(Serialize)]
pub struct PortInfo {
    pub name: String,
    pub kind: String,
    pub description: Option<String>,
}

pub fn list_ports() -> Result<Vec<PortInfo>, String> {
    let mut ports: Vec<PortInfo> = serialport::available_ports()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|p| {
            let (kind, description) = match p.port_type {
                SerialPortType::UsbPort(u) => ("usb", u.product.or(u.manufacturer)),
                SerialPortType::PciPort => ("pci", None),
                SerialPortType::BluetoothPort => ("bluetooth", None),
                SerialPortType::Unknown => ("unknown", None),
            };
            PortInfo { name: p.port_name, kind: kind.into(), description }
        })
        .collect();
    ports.sort_by_key(|p| {
        let n: u32 = p.name.trim_start_matches(|c: char| !c.is_ascii_digit()).parse().unwrap_or(u32::MAX);
        (n, p.name.clone())
    });
    Ok(ports)
}

fn port_listed(name: &str) -> Option<bool> {
    serialport::available_ports()
        .ok()
        .map(|v| v.iter().any(|p| p.port_name.eq_ignore_ascii_case(name)))
}

pub fn spawn(gw: Arc<Gateway>) -> SerialWorker {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let handle = thread::Builder::new()
        .name("serial-worker".into())
        .spawn(move || run(gw, flag))
        .expect("spawn serial thread");
    SerialWorker { stop, handle: Some(handle) }
}

enum Source {
    Port(Box<dyn SerialPort>),
    Sim(SimPort),
}

impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Source::Port(p) => p.read(buf),
            Source::Sim(s) => s.read(buf),
        }
    }
}

fn open_port(sc: &SerialConfig) -> Result<Source, String> {
    let data_bits = match sc.data_bits {
        5 => DataBits::Five,
        6 => DataBits::Six,
        7 => DataBits::Seven,
        _ => DataBits::Eight,
    };
    let stop_bits = if sc.stop_bits == 2 { StopBits::Two } else { StopBits::One };
    let parity = match sc.parity.as_str() {
        "odd" => Parity::Odd,
        "even" => Parity::Even,
        _ => Parity::None,
    };
    let flow = match sc.flow_control.as_str() {
        "software" => FlowControl::Software,
        "hardware" => FlowControl::Hardware,
        _ => FlowControl::None,
    };
    let port = serialport::new(sc.port.as_str(), sc.baud_rate)
        .data_bits(data_bits)
        .stop_bits(stop_bits)
        .parity(parity)
        .flow_control(flow)
        .timeout(READ_TIMEOUT)
        .open()
        .map_err(|e| match e.kind {
            serialport::ErrorKind::NoDevice => "Port not found (unplugged or wrong COM number)".to_string(),
            serialport::ErrorKind::Io(io::ErrorKind::NotFound) => "Port not found".to_string(),
            serialport::ErrorKind::Io(io::ErrorKind::PermissionDenied) => {
                "Access denied - the port is in use by another program".to_string()
            }
            _ => e.description,
        })?;
    Ok(Source::Port(port))
}

fn sleep_unless_stopped(d: Duration, stop: &AtomicBool) {
    let end = Instant::now() + d;
    while Instant::now() < end && !stop.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
    }
}

fn run(gw: Arc<Gateway>, stop: Arc<AtomicBool>) {
    let cfg = gw.config();
    let sc = cfg.serial.clone();
    let parser = match Parser::new(&cfg.parser) {
        Ok(p) => p,
        Err(e) => {
            gw.error("parser_config", e.clone(), true);
            gw.set_serial("error", Some(format!("Parser configuration: {e}")));
            return;
        }
    };
    let mut pipeline = Pipeline::new(&cfg, parser);
    let min = Duration::from_millis(sc.reconnect_min_ms);
    let max = Duration::from_millis(sc.reconnect_max_ms);
    let mut backoff = min;
    let label = if sc.source == "simulator" { "Simulator".to_string() } else { sc.port.clone() };

    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        gw.set_serial("connecting", None);
        let opened = if sc.source == "simulator" { Ok(Source::Sim(SimPort::new())) } else { open_port(&sc) };
        match opened {
            Ok(mut port) => {
                backoff = min;
                pipeline.reset();
                gw.set_serial("connected", None);
                let reason = read_loop(&gw, &mut port, &sc, &mut pipeline, &stop);
                drop(port);
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                gw.error("serial_disconnected", format!("{label}: {reason}"), true);
                gw.set_serial("disconnected", Some(reason));
            }
            Err(e) => {
                gw.error("serial_open_failed", format!("{label}: {e}"), true);
                gw.set_serial("error", Some(e));
            }
        }
        if !sc.auto_reconnect {
            return;
        }
        gw.set_serial("reconnecting", Some(format!("Retrying in {} s", backoff.as_secs().max(1))));
        sleep_unless_stopped(backoff, &stop);
        backoff = (backoff * 2).min(max);
    }
}

/// Returns why the connection ended.
fn read_loop(gw: &Gateway, port: &mut Source, sc: &SerialConfig, pipeline: &mut Pipeline, stop: &AtomicBool) -> String {
    let mut fb = FrameBuffer::new(Delimiter::parse(&sc.delimiter), sc.max_frame_bytes, sc.idle_flush_ms);
    let mut buf = [0u8; 1024];
    // Windows USB-serial adapters often keep "reading" (timeouts) after being unplugged,
    // so also watch the port list. Only for ports that were listed when we connected.
    let watch_presence = sc.source == "serial" && port_listed(&sc.port) == Some(true);
    let silence = (sc.silence_timeout_ms > 0).then(|| Duration::from_millis(sc.silence_timeout_ms));
    let mut last_check = Instant::now();
    let mut last_rx = Instant::now();
    let mut silent = false;

    loop {
        if stop.load(Ordering::SeqCst) {
            return "stopped".into();
        }
        match port.read(&mut buf) {
            Ok(0) => thread::sleep(Duration::from_millis(10)),
            Ok(n) => {
                let now = Instant::now();
                last_rx = now;
                if silent {
                    silent = false;
                    gw.set_serial("connected", None);
                }
                gw.add_bytes(n);
                for frame in fb.push(&buf[..n], now) {
                    pipeline.handle(gw, &frame, now);
                }
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
            Err(e) => return format!("Read failed: {e}"),
        }
        let now = Instant::now();
        if let Some(frame) = fb.poll_idle(now) {
            pipeline.handle(gw, &frame, now);
        }
        if watch_presence && now.duration_since(last_check) >= PRESENCE_CHECK {
            last_check = now;
            if port_listed(&sc.port) == Some(false) {
                return "Port disappeared (cable or USB adapter unplugged?)".into();
            }
        }
        if let Some(limit) = silence {
            if !silent && now.duration_since(last_rx) >= limit {
                silent = true;
                let msg = format!("No data for {} s - check the indicator and cable", limit.as_secs());
                gw.error("no_data", msg.clone(), true);
                gw.set_serial("connected", Some(msg));
            }
        }
    }
}

struct Pipeline {
    parser: Parser,
    motion: MotionDetector,
    device_id: String,
    secret: String,
    max_weight: f64,
    last_error_at: Option<Instant>,
}

impl Pipeline {
    fn new(cfg: &AppConfig, parser: Parser) -> Self {
        Self {
            parser,
            motion: MotionDetector::new(cfg.parser.motion_window_ms, cfg.parser.motion_tolerance),
            device_id: cfg.device_id.trim().to_string(),
            secret: cfg.websocket.signing_secret.clone(),
            max_weight: cfg.parser.max_weight,
            last_error_at: None,
        }
    }

    fn reset(&mut self) {
        self.motion.reset();
    }

    /// Reports at most one parse error per ERROR_THROTTLE so a wrong parser setting cannot flood clients.
    fn throttled_error(&mut self, gw: &Gateway, code: &str, msg: String, now: Instant) {
        if self.last_error_at.map(|t| now.duration_since(t) >= ERROR_THROTTLE).unwrap_or(true) {
            self.last_error_at = Some(now);
            gw.error(code, msg, true);
        }
    }

    fn handle(&mut self, gw: &Gateway, frame: &Frame, now: Instant) {
        if frame.status == FrameStatus::Overflow {
            gw.record_frame(frame, Some("Frame too long - not parsed (check delimiter)".into()));
            return;
        }
        match self.parser.parse(&frame.text) {
            Ok(p) => {
                if p.weight.abs() > self.max_weight {
                    let msg = format!("Weight {} exceeds max {}", p.weight, self.max_weight);
                    gw.add_parse_failure();
                    gw.record_frame(frame, Some(msg.clone()));
                    self.throttled_error(gw, "out_of_range", msg, now);
                    return;
                }
                let motion_stable = self.motion.push(now, p.weight);
                let (stable, source) = match p.stability {
                    Stability::Stable => (true, "indicator"),
                    Stability::Unstable => (false, "indicator"),
                    Stability::Unknown => (motion_stable, "motion"),
                };
                gw.record_frame(
                    frame,
                    Some(format!("{} {} {}", p.weight, p.unit, if stable { "stable" } else { "moving" })),
                );
                let mut msg = WeightMsg {
                    device_id: self.device_id.clone(),
                    reading_id: gw.next_reading_id(),
                    weight: p.weight,
                    unit: p.unit,
                    stable,
                    stability_source: source.into(),
                    weight_type: p.weight_type,
                    raw_data: frame.printable(),
                    timestamp: now_iso(),
                    signature: String::new(),
                };
                msg.sign(&self.secret);
                gw.record_weight(msg);
            }
            Err(ParseError::Skipped) => gw.record_frame(frame, None),
            Err(ParseError::Failed(e)) => {
                gw.add_parse_failure();
                gw.record_frame(frame, Some(e.clone()));
                self.throttled_error(gw, "parse_failed", e, now);
            }
        }
    }
}

/// Built-in test signal: a truck drives on, settles, drives off, every 40 s.
/// Emits "ST,GS,+025450 kg\r\n" frames in small chunks, like a real port does.
struct SimPort {
    start: Instant,
    next: Instant,
    pending: VecDeque<u8>,
}

impl SimPort {
    fn new() -> Self {
        let now = Instant::now();
        Self { start: now, next: now, pending: VecDeque::new() }
    }

    fn frame(&self, now: Instant) -> String {
        let t = now.duration_since(self.start).as_secs_f64();
        let cycle = (t / 40.0) as i64;
        let phase = t % 40.0;
        let target = (12_000 + (cycle * 3_770) % 20_000) / 10 * 10;
        let (w, stable) = if phase < 5.0 {
            (0, true)
        } else if phase < 12.0 {
            let f = (phase - 5.0) / 7.0;
            ((target as f64 * f + (t * 7.3).sin() * 120.0) as i64, false)
        } else if phase < 30.0 {
            (target, true)
        } else if phase < 34.0 {
            let f = 1.0 - (phase - 30.0) / 4.0;
            ((target as f64 * f) as i64, false)
        } else {
            (0, true)
        };
        let w = w.max(0) / 10 * 10;
        format!("{},GS,{:+07} kg\r\n", if stable { "ST" } else { "US" }, w)
    }
}

impl Read for SimPort {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() {
            let now = Instant::now();
            if now < self.next {
                thread::sleep((self.next - now).min(READ_TIMEOUT));
                if Instant::now() < self.next {
                    return Err(io::ErrorKind::TimedOut.into());
                }
            }
            let now = Instant::now();
            let frame = self.frame(now);
            self.pending.extend(frame.bytes());
            self.next = (self.next + Duration::from_millis(250)).max(now);
        }
        let n = buf.len().min(self.pending.len()).min(7);
        for (slot, byte) in buf.iter_mut().zip(self.pending.drain(..n)) {
            *slot = byte;
        }
        Ok(n)
    }
}
