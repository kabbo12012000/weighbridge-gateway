//! Turns a frame's text into a weight. Indicator protocols vary, so the rules are configurable:
//!
//! * `auto`  – finds the weight number and recognises stability/type/overload codes as tokens.
//!             Handles "ST,GS,+025450 kg", "ST,GS,25450kg", "US 25450", "+025450", "=05452000" (with reverse).
//! * `demo`  – strict "ST,GS,+001250 KG" format (the built-in simulator / JS gateway demo format).
//! * `regex` – operator-supplied pattern with named groups weight, sign, unit, stability, type.
//! * `raw`   – no parsing; frames are only shown/broadcast as serial_data (use while discovering a protocol).

use std::collections::{BTreeMap, HashSet};

use regex::Regex;
use serde::Serialize;

use crate::config::ParserConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stability {
    Stable,
    Unstable,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Parsed {
    pub weight: f64,
    pub unit: String,
    pub stability: Stability,
    pub weight_type: String,
}

#[derive(Debug, PartialEq)]
pub enum ParseError {
    /// Parsing is switched off (raw mode) – not an error.
    Skipped,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Auto,
    Demo,
    Regex,
    Raw,
}

pub struct Parser {
    mode: Mode,
    pattern: Option<Regex>,
    reverse: bool,
    scale: f64,
    default_unit: String,
    stable: HashSet<String>,
    unstable: HashSet<String>,
    overload: HashSet<String>,
    types: BTreeMap<String, String>,
}

const AUTO_NUMBER: &str = r"(?i)(?P<sign>[+-])?\s*(?P<weight>\d+(?:\.\d+)?)\s*(?P<unit>kg|lbs?|t)?\b";
const DEMO: &str = r"(?i)^(?P<stability>ST|US|OL),(?P<type>GS|NT|TR),(?P<sign>[+-]?)\s*(?P<weight>\d+(?:\.\d+)?)\s*(?P<unit>[A-Za-z]{1,3})$";

fn upper_set(v: &[String]) -> HashSet<String> {
    v.iter().map(|s| s.trim().to_ascii_uppercase()).filter(|s| !s.is_empty()).collect()
}

impl Parser {
    pub fn new(cfg: &ParserConfig) -> Result<Self, String> {
        let mode = match cfg.mode.as_str() {
            "auto" => Mode::Auto,
            "demo" => Mode::Demo,
            "regex" => Mode::Regex,
            "raw" => Mode::Raw,
            other => return Err(format!("Unknown parser mode \"{other}\"")),
        };
        let pattern = match mode {
            Mode::Auto => Some(Regex::new(AUTO_NUMBER).expect("built-in regex")),
            Mode::Demo => Some(Regex::new(DEMO).expect("built-in regex")),
            Mode::Regex => {
                if cfg.regex.trim().is_empty() {
                    return Err("Regex mode needs a pattern".into());
                }
                let re = Regex::new(&cfg.regex).map_err(|e| format!("Invalid regex: {e}"))?;
                if !re.capture_names().any(|n| n == Some("weight")) {
                    return Err("Regex must contain a named group (?<weight>...)".into());
                }
                Some(re)
            }
            Mode::Raw => None,
        };
        let mut overload = upper_set(&cfg.overload_codes);
        if mode == Mode::Demo {
            overload.insert("OL".into());
        }
        let mut stable = upper_set(&cfg.stable_codes);
        let mut unstable = upper_set(&cfg.unstable_codes);
        if mode == Mode::Demo {
            stable.insert("ST".into());
            unstable.insert("US".into());
        }
        Ok(Self {
            mode,
            pattern,
            reverse: cfg.reverse_digits,
            scale: if cfg.scale > 0.0 { cfg.scale } else { 1.0 },
            default_unit: cfg.default_unit.trim().to_ascii_lowercase(),
            stable,
            unstable,
            overload,
            types: cfg
                .type_codes
                .iter()
                .map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim().to_ascii_lowercase()))
                .collect(),
        })
    }

    pub fn parse(&self, text: &str) -> Result<Parsed, ParseError> {
        if self.mode == Mode::Raw {
            return Err(ParseError::Skipped);
        }
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        let clean = clean.trim();
        if clean.is_empty() {
            return Err(ParseError::Failed("Empty message".into()));
        }
        match self.mode {
            Mode::Auto => self.parse_auto(clean),
            _ => self.parse_pattern(clean),
        }
    }

    fn parse_pattern(&self, text: &str) -> Result<Parsed, ParseError> {
        let re = self.pattern.as_ref().expect("pattern for mode");
        let caps = re
            .captures(text)
            .ok_or_else(|| ParseError::Failed(format!("No match: \"{}\"", truncate(text))))?;
        let get = |n: &str| caps.name(n).map(|m| m.as_str().trim().to_string());
        let mut stability = Stability::Unknown;
        if let Some(code) = get("stability").map(|c| c.to_ascii_uppercase()) {
            if self.overload.contains(&code) {
                return Err(ParseError::Failed("Indicator reports overload".into()));
            }
            if self.stable.contains(&code) {
                stability = Stability::Stable;
            } else if self.unstable.contains(&code) {
                stability = Stability::Unstable;
            }
        }
        let weight_type = get("type")
            .and_then(|t| self.types.get(&t.to_ascii_uppercase()).cloned())
            .unwrap_or_else(|| "unknown".into());
        let negative = get("sign").as_deref() == Some("-");
        let weight = self.number(&get("weight").unwrap_or_default(), negative)?;
        Ok(Parsed { weight, unit: self.unit(get("unit")), stability, weight_type })
    }

    fn parse_auto(&self, text: &str) -> Result<Parsed, ParseError> {
        let mut stability = Stability::Unknown;
        let mut weight_type = "unknown".to_string();
        for token in text.split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| !t.is_empty()) {
            let t = token.to_ascii_uppercase();
            if self.overload.contains(&t) {
                return Err(ParseError::Failed("Indicator reports overload".into()));
            }
            if self.stable.contains(&t) {
                stability = Stability::Stable;
            } else if self.unstable.contains(&t) {
                stability = Stability::Unstable;
            }
            if let Some(ty) = self.types.get(&t) {
                weight_type = ty.clone();
            }
        }
        // Pick the number that carries a unit; otherwise the one with the most digits
        // (so an address/ID prefix like "WB01" does not win over "025450").
        let re = self.pattern.as_ref().expect("auto pattern");
        let best = re
            .captures_iter(text)
            .max_by_key(|c| {
                let has_unit = c.name("unit").is_some() as usize;
                let digits = c.name("weight").map(|m| m.as_str().len()).unwrap_or(0);
                (has_unit, digits)
            })
            .ok_or_else(|| ParseError::Failed(format!("No number in \"{}\"", truncate(text))))?;
        let negative = best.name("sign").map(|m| m.as_str()) == Some("-");
        let weight = self.number(best.name("weight").map(|m| m.as_str()).unwrap_or(""), negative)?;
        let unit = best.name("unit").map(|m| m.as_str().to_string());
        Ok(Parsed { weight, unit: self.unit(unit), stability, weight_type })
    }

    fn number(&self, digits: &str, mut negative: bool) -> Result<f64, ParseError> {
        let mut s: String = digits.chars().filter(|c| !c.is_whitespace()).collect();
        if let Some(rest) = s.strip_prefix('-') {
            negative = true;
            s = rest.to_string();
        } else if let Some(rest) = s.strip_prefix('+') {
            s = rest.to_string();
        }
        if self.reverse {
            s = s.chars().rev().collect();
        }
        let valid = !s.is_empty()
            && s.chars().all(|c| c.is_ascii_digit() || c == '.')
            && s.matches('.').count() <= 1
            && !s.starts_with('.')
            && !s.ends_with('.');
        if !valid {
            return Err(ParseError::Failed(format!("Weight \"{digits}\" is not numeric")));
        }
        let mut w: f64 = s.parse().map_err(|_| ParseError::Failed(format!("Weight \"{digits}\" is not numeric")))?;
        if negative {
            w = -w;
        }
        w = ((w * self.scale) * 1000.0).round() / 1000.0;
        if w == 0.0 {
            w = 0.0; // normalise -0
        }
        Ok(w)
    }

    fn unit(&self, u: Option<String>) -> String {
        match u.map(|s| s.trim().to_ascii_lowercase()) {
            Some(s) if !s.is_empty() => s,
            _ => self.default_unit.clone(),
        }
    }
}

fn truncate(s: &str) -> String {
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(60).collect::<String>())
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser(mode: &str) -> Parser {
        let cfg = ParserConfig { mode: mode.into(), ..Default::default() };
        Parser::new(&cfg).unwrap()
    }

    #[test]
    fn auto_parses_spec_example() {
        let p = parser("auto").parse("ST,GS,25450kg").unwrap();
        assert_eq!(p.weight, 25450.0);
        assert_eq!(p.unit, "kg");
        assert_eq!(p.stability, Stability::Stable);
        assert_eq!(p.weight_type, "gross");
    }

    #[test]
    fn auto_parses_signed_and_unstable() {
        let p = parser("auto").parse("US,NT,-000120 kg").unwrap();
        assert_eq!(p.weight, -120.0);
        assert_eq!(p.stability, Stability::Unstable);
        assert_eq!(p.weight_type, "net");
    }

    #[test]
    fn auto_prefers_number_with_unit() {
        let p = parser("auto").parse("WB01 ST 25450 kg").unwrap();
        assert_eq!(p.weight, 25450.0);
    }

    #[test]
    fn auto_bare_number_defaults_unit_and_unknown_stability() {
        let p = parser("auto").parse("\x02+025450\x03").unwrap();
        assert_eq!(p.weight, 25450.0);
        assert_eq!(p.unit, "kg");
        assert_eq!(p.stability, Stability::Unknown);
    }

    #[test]
    fn auto_rejects_overload() {
        assert!(matches!(parser("auto").parse("OL,GS,999999kg"), Err(ParseError::Failed(_))));
    }

    #[test]
    fn reverse_digits() {
        let cfg = ParserConfig { reverse_digits: true, ..Default::default() };
        let p = Parser::new(&cfg).unwrap().parse("=05452000").unwrap();
        assert_eq!(p.weight, 25450.0);
    }

    #[test]
    fn demo_format() {
        let p = parser("demo").parse("ST,GS,+001250 KG").unwrap();
        assert_eq!(p.weight, 1250.0);
        assert_eq!(p.unit, "kg");
        assert!(parser("demo").parse("hello").is_err());
    }

    #[test]
    fn regex_mode_with_codes_and_scale() {
        let cfg = ParserConfig {
            mode: "regex".into(),
            regex: r"^WT:\s*(?<sign>[+-]?)\s*(?<weight>\d+)\s*(?<stability>[SU])$".into(),
            scale: 10.0,
            ..Default::default()
        };
        let p = Parser::new(&cfg).unwrap().parse("WT: 2545 S").unwrap();
        assert_eq!(p.weight, 25450.0);
        assert_eq!(p.stability, Stability::Stable);
    }

    #[test]
    fn regex_requires_weight_group() {
        let cfg = ParserConfig { mode: "regex".into(), regex: r"(\d+)".into(), ..Default::default() };
        assert!(Parser::new(&cfg).is_err());
    }

    #[test]
    fn raw_mode_skips() {
        assert_eq!(parser("raw").parse("anything"), Err(ParseError::Skipped));
    }
}
