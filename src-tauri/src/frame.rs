//! Splits the serial byte stream into frames. Port of the JS gateway's FrameBuffer:
//! serial data arrives in arbitrary chunks, so a frame may span reads and a read may hold many frames.

use std::time::{Duration, Instant};

const CR: u8 = 0x0d;
const LF: u8 = 0x0a;
const STX: u8 = 0x02;
const ETX: u8 = 0x03;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delimiter {
    Auto,
    Cr,
    Lf,
    CrLf,
    Etx,
}

impl Delimiter {
    pub fn parse(s: &str) -> Delimiter {
        match s.to_ascii_uppercase().as_str() {
            "CR" => Delimiter::Cr,
            "LF" => Delimiter::Lf,
            "CRLF" => Delimiter::CrLf,
            "ETX" => Delimiter::Etx,
            _ => Delimiter::Auto,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameStatus {
    Complete,
    /// Bytes followed by silence and no terminator (idle flush).
    Incomplete,
    /// Frame exceeded max_frame_bytes and was cut.
    Overflow,
}

impl FrameStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            FrameStatus::Complete => "complete",
            FrameStatus::Incomplete => "incomplete",
            FrameStatus::Overflow => "overflow",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub raw: Vec<u8>,
    /// Latin-1 decoded, STX stripped in ETX mode. Control characters are kept.
    pub text: String,
    pub status: FrameStatus,
}

impl Frame {
    pub fn hex(&self) -> String {
        self.raw.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    }

    /// Text with control characters made visible, for logs and the monitor.
    pub fn printable(&self) -> String {
        self.text
            .chars()
            .map(|c| match c {
                '\u{20}'..='\u{7e}' => c.to_string(),
                c => format!("<{:02X}>", c as u32),
            })
            .collect()
    }
}

pub struct FrameBuffer {
    delimiter: Delimiter,
    max_frame_bytes: usize,
    idle_flush: Option<Duration>,
    bytes: Vec<u8>,
    last_was_cr: bool,
    last_byte_at: Option<Instant>,
}

impl FrameBuffer {
    pub fn new(delimiter: Delimiter, max_frame_bytes: usize, idle_flush_ms: u64) -> Self {
        Self {
            delimiter,
            max_frame_bytes: max_frame_bytes.max(1),
            idle_flush: (idle_flush_ms > 0).then(|| Duration::from_millis(idle_flush_ms)),
            bytes: Vec::with_capacity(128),
            last_was_cr: false,
            last_byte_at: None,
        }
    }

    pub fn push(&mut self, data: &[u8], now: Instant) -> Vec<Frame> {
        let mut out = Vec::new();
        for &byte in data {
            match self.delimiter {
                Delimiter::Auto => {
                    if byte == CR {
                        self.emit(FrameStatus::Complete, &mut out);
                        self.last_was_cr = true;
                        continue;
                    }
                    if byte == LF {
                        if self.last_was_cr {
                            self.last_was_cr = false; // second half of CRLF
                            continue;
                        }
                        self.emit(FrameStatus::Complete, &mut out);
                        continue;
                    }
                    self.last_was_cr = false;
                }
                Delimiter::Cr => {
                    if byte == CR {
                        self.emit(FrameStatus::Complete, &mut out);
                        continue;
                    }
                }
                Delimiter::Lf => {
                    if byte == LF {
                        if self.bytes.last() == Some(&CR) {
                            self.bytes.pop();
                        }
                        self.emit(FrameStatus::Complete, &mut out);
                        continue;
                    }
                }
                Delimiter::CrLf => {
                    if byte == LF && self.bytes.last() == Some(&CR) {
                        self.bytes.pop();
                        self.emit(FrameStatus::Complete, &mut out);
                        continue;
                    }
                }
                Delimiter::Etx => {
                    if byte == ETX {
                        self.emit(FrameStatus::Complete, &mut out);
                        continue;
                    }
                }
            }
            self.bytes.push(byte);
            if self.bytes.len() >= self.max_frame_bytes {
                self.emit(FrameStatus::Overflow, &mut out);
            }
        }
        if !data.is_empty() {
            self.last_byte_at = Some(now);
        }
        out
    }

    /// Emits buffered bytes as an incomplete frame once the line has been idle long enough.
    pub fn poll_idle(&mut self, now: Instant) -> Option<Frame> {
        let idle = self.idle_flush?;
        let last = self.last_byte_at?;
        if !self.bytes.is_empty() && now.duration_since(last) >= idle {
            let mut out = Vec::with_capacity(1);
            self.emit(FrameStatus::Incomplete, &mut out);
            return out.pop();
        }
        None
    }

    pub fn clear(&mut self) {
        self.bytes.clear();
        self.last_was_cr = false;
    }

    fn emit(&mut self, status: FrameStatus, out: &mut Vec<Frame>) {
        if self.bytes.is_empty() {
            return;
        }
        let raw = std::mem::take(&mut self.bytes);
        let body = if self.delimiter == Delimiter::Etx && raw.first() == Some(&STX) {
            &raw[1..]
        } else {
            &raw[..]
        };
        let text: String = body.iter().map(|&b| b as char).collect();
        out.push(Frame { raw, text, status });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(frames: &[Frame]) -> Vec<String> {
        frames.iter().map(|f| f.text.clone()).collect()
    }

    #[test]
    fn auto_handles_cr_lf_and_crlf() {
        let mut fb = FrameBuffer::new(Delimiter::Auto, 1024, 0);
        let f = fb.push(b"A\rB\nC\r\nD", Instant::now());
        assert_eq!(texts(&f), vec!["A", "B", "C"]);
    }

    #[test]
    fn frame_split_across_reads() {
        let mut fb = FrameBuffer::new(Delimiter::CrLf, 1024, 0);
        let now = Instant::now();
        assert!(fb.push(b"ST,GS,+0254", now).is_empty());
        let f = fb.push(b"50 kg\r", now);
        assert!(f.is_empty());
        let f = fb.push(b"\nST", now);
        assert_eq!(texts(&f), vec!["ST,GS,+025450 kg"]);
    }

    #[test]
    fn etx_strips_stx() {
        let mut fb = FrameBuffer::new(Delimiter::Etx, 1024, 0);
        let f = fb.push(b"\x02+025450\x03", Instant::now());
        assert_eq!(texts(&f), vec!["+025450"]);
    }

    #[test]
    fn overflow_cuts_frame() {
        let mut fb = FrameBuffer::new(Delimiter::Cr, 4, 0);
        let f = fb.push(b"123456", Instant::now());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].status, FrameStatus::Overflow);
        assert_eq!(f[0].text, "1234");
    }

    #[test]
    fn idle_flush_emits_incomplete() {
        let mut fb = FrameBuffer::new(Delimiter::Cr, 1024, 100);
        let t0 = Instant::now();
        fb.push(b"25450", t0);
        assert!(fb.poll_idle(t0 + Duration::from_millis(50)).is_none());
        let f = fb.poll_idle(t0 + Duration::from_millis(150)).unwrap();
        assert_eq!(f.status, FrameStatus::Incomplete);
        assert_eq!(f.text, "25450");
    }
}
