// Mirrors the Rust structs in src-tauri/src (config.rs, state.rs, protocol.rs).

export interface SerialConfig {
  source: 'serial' | 'simulator';
  port: string;
  baud_rate: number;
  data_bits: number;
  stop_bits: number;
  parity: 'none' | 'odd' | 'even';
  flow_control: 'none' | 'software' | 'hardware';
  auto_connect: boolean;
  auto_reconnect: boolean;
  reconnect_min_ms: number;
  reconnect_max_ms: number;
  delimiter: 'AUTO' | 'CR' | 'LF' | 'CRLF' | 'ETX';
  idle_flush_ms: number;
  max_frame_bytes: number;
  silence_timeout_ms: number;
}

export interface ParserConfig {
  mode: 'auto' | 'demo' | 'regex' | 'raw';
  regex: string;
  reverse_digits: boolean;
  scale: number;
  default_unit: string;
  stable_codes: string[];
  unstable_codes: string[];
  overload_codes: string[];
  type_codes: Record<string, string>;
  motion_window_ms: number;
  motion_tolerance: number;
  max_weight: number;
}

export interface WsConfig {
  port_start: number;
  port_end: number;
  allowed_origins: string[];
  allow_missing_origin: boolean;
  require_auth: boolean;
  token: string;
  signing_secret: string;
  heartbeat_ms: number;
}

export interface AppConfig {
  device_id: string;
  serial: SerialConfig;
  parser: ParserConfig;
  websocket: WsConfig;
}

export interface WeightMsg {
  device_id: string;
  reading_id: string;
  weight: number;
  unit: string;
  stable: boolean;
  stability_source: 'indicator' | 'motion';
  weight_type: string;
  raw_data: string;
  timestamp: string;
  signature: string;
}

export interface Snapshot {
  version: string;
  device_id: string;
  serial: { status: string; source: string; port: string; message: string | null; since: string };
  ws: { status: string; port: number | null; url: string | null; secure_url: string | null; message: string | null };
  clients: { id: number; origin: string | null; secure: boolean; connected_at: string }[];
  last_data: string | null;
  last_data_at: string | null;
  last_weight: WeightMsg | null;
  errors: { timestamp: string; code: string; message: string }[];
  raw: { timestamp: string; text: string; hex: string; status: string; result: string | null }[];
  stats: { bytes: number; frames: number; weights: number; parse_failures: number };
  tls_configured: boolean;
  paths: { config_file: string; log_dir: string; tls_dir: string };
  config_warning: string | null;
}

export interface PortInfo {
  name: string;
  kind: string;
  description: string | null;
}

export interface Parsed {
  weight: number;
  unit: string;
  stability: 'stable' | 'unstable' | 'unknown';
  weight_type: string;
}
