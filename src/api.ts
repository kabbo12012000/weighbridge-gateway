import { invoke } from '@tauri-apps/api/core';
import type { AppConfig, Parsed, PortInfo, Snapshot } from './types';

const browserConfig: AppConfig = {
  device_id: 'WB-001',
  serial: {
    source: 'simulator',
    port: 'COM2',
    baud_rate: 9600,
    data_bits: 8,
    stop_bits: 1,
    parity: 'none',
    flow_control: 'none',
    auto_connect: true,
    auto_reconnect: true,
    reconnect_min_ms: 1000,
    reconnect_max_ms: 30000,
    delimiter: 'AUTO',
    idle_flush_ms: 1000,
    max_frame_bytes: 1024,
    silence_timeout_ms: 10000,
  },
  parser: {
    mode: 'auto',
    regex: '',
    reverse_digits: false,
    scale: 1,
    default_unit: 'kg',
    stable_codes: ['ST', 'S'],
    unstable_codes: ['US', 'U', 'MO'],
    overload_codes: ['OL', 'OVER'],
    type_codes: { G: 'gross', GS: 'gross', N: 'net', NT: 'net', T: 'tare', TR: 'tare' },
    motion_window_ms: 1500,
    motion_tolerance: 0,
    max_weight: 200000,
  },
  websocket: {
    port_start: 8765,
    port_end: 8775,
    allowed_origins: [],
    allow_missing_origin: true,
    require_auth: true,
    token: 'browser-preview',
    signing_secret: 'browser-preview',
    heartbeat_ms: 15000,
  },
};

const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

function browserState(): Snapshot {
  const now = new Date().toISOString();
  return {
    version: 'browser-preview',
    device_id: browserConfig.device_id,
    serial: {
      status: 'disconnected',
      source: 'simulator',
      port: browserConfig.serial.port,
      message: 'Browser preview: native gateway unavailable',
      since: now,
    },
    ws: {
      status: 'disconnected',
      port: null,
      url: null,
      secure_url: null,
      message: 'Run the desktop Tauri app to start the gateway',
    },
    clients: [],
    last_data: null,
    last_data_at: null,
    last_weight: null,
    errors: [],
    raw: [],
    stats: { bytes: 0, frames: 0, weights: 0, parse_failures: 0 },
    tls_configured: false,
    paths: { config_file: '', log_dir: '', tls_dir: '' },
    config_warning: 'Browser preview mode. Serial and WebSocket features require the Tauri desktop app.',
  };
}

function native<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri) {
    return Promise.reject(new Error('This action is available only in the Tauri desktop app.'));
  }
  return invoke<T>(command, args);
}

export const api = {
  getState: () => (isTauri ? native<Snapshot>('get_state') : Promise.resolve(browserState())),
  getConfig: () => (isTauri ? native<AppConfig>('get_config') : Promise.resolve(browserConfig)),
  saveConfig: (config: AppConfig) => native<AppConfig>('save_config', { config }),
  listPorts: () => native<PortInfo[]>('list_ports'),
  connect: () => native<void>('connect_serial'),
  disconnect: () => native<void>('disconnect_serial'),
  regenerate: (which: 'token' | 'signing_secret') => native<AppConfig>('regenerate_secret', { which }),
  testParser: (config: AppConfig, text: string) => native<Parsed>('test_parser', { config, text }),
  setupTls: () => native<void>('setup_tls'),
  removeTls: () => native<void>('remove_tls'),
  clearMonitor: () => native<void>('clear_monitor'),
  getAutostart: () => native<boolean>('get_autostart'),
  setAutostart: (enabled: boolean) => native<void>('set_autostart', { enabled }),
  openFolder: (path: string) => native<void>('open_folder', { path }),
};

export function errorText(e: unknown): string {
  return typeof e === 'string' ? e : e instanceof Error ? e.message : JSON.stringify(e);
}
