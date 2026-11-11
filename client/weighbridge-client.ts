/**
 * Browser client for the Weighbridge Gateway (copy into the Laravel React app).
 *
 * - Discovers the gateway on 127.0.0.1 by scanning the configured port range
 *   (last working URL is tried first), and identifies it by its `hello` message.
 * - Authenticates with the device token, receives live readings, reconnects automatically.
 * - Never talks to the serial port and never saves anything: saving is the operator's
 *   explicit action, done with `capture()` + your Laravel API call.
 */

export type ConnectionState =
  | 'idle'
  | 'discovering'
  | 'authenticating'
  | 'connected'
  | 'unavailable' // no gateway found / connection lost; retrying
  | 'unauthorized'; // token rejected; retrying slowly

export interface WeightReading {
  type: 'weight';
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

export interface SerialStatus {
  type: 'serial_status';
  status: 'connected' | 'connecting' | 'reconnecting' | 'disconnected' | 'error';
  source: string;
  port: string;
  message: string | null;
  timestamp: string;
}

export interface GatewayStatus {
  type: 'gateway_status';
  status: string;
  event: 'snapshot' | 'client_connected' | 'client_disconnected';
  url: string;
  secure_url: string | null;
  port: number;
  clients: number;
  device_id: string;
  version: string;
  timestamp: string;
}

export interface GatewayError {
  type: 'error';
  code: string;
  message: string;
  timestamp: string;
}

/** What gets submitted to Laravel: the gateway reading exactly as signed, plus when it was captured. */
export interface CapturedWeight {
  device_id: string;
  reading_id: string;
  weight: number;
  unit: string;
  stable: boolean;
  stability_source: string;
  weight_type: string;
  raw_data: string;
  measured_at: string; // gateway timestamp (part of the signature)
  signature: string;
  captured_at: string; // when the operator pressed Capture
}

export interface WeighbridgeClientOptions {
  /** Device token from the gateway, served to the logged-in operator by Laravel. */
  token: string | (() => Promise<string>);
  /** Only accept a gateway reporting this device_id (recommended when a PC could run several). */
  deviceId?: string;
  host?: string;
  portStart?: number;
  portEnd?: number;
  /** 'auto' tries wss:// then ws:// on HTTPS pages, ws:// only on HTTP pages. */
  secure?: 'auto' | 'always' | 'never';
  /** A reading older than this is treated as stale (no live weight). */
  staleAfterMs?: number;
  /** Also receive every raw frame as `serial_data` (debugging). */
  raw?: boolean;
  reconnectMinMs?: number;
  reconnectMaxMs?: number;
  storageKey?: string;
}

type Events = {
  state: ConnectionState;
  weight: WeightReading;
  serial: SerialStatus;
  gateway: GatewayStatus;
  error: GatewayError;
  message: { type: string; [k: string]: unknown };
};

type Listener<T> = (value: T) => void;

const GATEWAY_NAME = 'weighbridge-gateway';
const PROBE_TIMEOUT_MS = 1500;
const AUTH_TIMEOUT_MS = 5000;
/** Gateway heartbeats every 5 s; silence for this long means the connection is dead. */
const WATCHDOG_MS = 15000;
const CLOSE_UNAUTHORIZED = 4401;
const CLOSE_RECONFIGURED = 4000;

export class WeighbridgeClient {
  private opts: Required<Omit<WeighbridgeClientOptions, 'deviceId'>> & { deviceId?: string };
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private listeners = new Map<keyof Events, Set<Listener<any>>>();
  private ws: WebSocket | null = null;
  private stopped = true;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private watchdog: ReturnType<typeof setTimeout> | null = null;
  private backoff: number;

  state: ConnectionState = 'idle';
  url: string | null = null;
  latest: WeightReading | null = null;
  serial: SerialStatus | null = null;
  gateway: GatewayStatus | null = null;

  constructor(options: WeighbridgeClientOptions) {
    this.opts = {
      host: '127.0.0.1',
      portStart: 8765,
      portEnd: 8800,
      secure: 'auto',
      staleAfterMs: 3000,
      raw: false,
      reconnectMinMs: 1000,
      reconnectMaxMs: 10000,
      storageKey: 'weighbridge.gateway.url',
      // Drop explicit `undefined`s so they don't override the defaults.
      ...(Object.fromEntries(Object.entries(options).filter(([, v]) => v !== undefined)) as WeighbridgeClientOptions),
    };
    this.backoff = this.opts.reconnectMinMs;
  }

  on<K extends keyof Events>(event: K, fn: Listener<Events[K]>): () => void {
    let set = this.listeners.get(event);
    if (!set) this.listeners.set(event, (set = new Set()));
    set.add(fn);
    return () => {
      set.delete(fn);
    };
  }

  start(): void {
    if (!this.stopped) return;
    this.stopped = false;
    void this.connect();
  }

  stop(): void {
    this.stopped = true;
    this.clearTimers();
    const ws = this.ws;
    this.ws = null;
    ws?.close(1000);
    this.setState('idle');
  }

  /** Latest reading if it is live (not stale). */
  current(): WeightReading | null {
    const w = this.latest;
    if (!w || this.state !== 'connected') return null;
    return Date.now() - new Date(w.timestamp).getTime() <= this.opts.staleAfterMs ? w : null;
  }

  /** Freezes the current reading for the form. Returns null when there is no live reading. */
  capture(): CapturedWeight | null {
    const w = this.current();
    if (!w) return null;
    return {
      device_id: w.device_id,
      reading_id: w.reading_id,
      weight: w.weight,
      unit: w.unit,
      stable: w.stable,
      stability_source: w.stability_source,
      weight_type: w.weight_type,
      raw_data: w.raw_data,
      measured_at: w.timestamp,
      signature: w.signature,
      captured_at: new Date().toISOString(),
    };
  }

  // ---- internals -----------------------------------------------------------------

  private emit<K extends keyof Events>(event: K, value: Events[K]) {
    this.listeners.get(event)?.forEach((fn) => {
      try {
        fn(value);
      } catch (e) {
        console.error('[weighbridge] listener failed', e);
      }
    });
  }

  private setState(s: ConnectionState) {
    if (this.state === s) return;
    this.state = s;
    this.emit('state', s);
  }

  private clearTimers() {
    if (this.retryTimer) clearTimeout(this.retryTimer);
    if (this.watchdog) clearTimeout(this.watchdog);
    this.retryTimer = this.watchdog = null;
  }

  private candidates(): string[] {
    const { secure, host, portStart, portEnd, storageKey } = this.opts;
    const https = typeof location !== 'undefined' && location.protocol === 'https:';
    const schemes = secure === 'always' ? ['wss'] : secure === 'never' ? ['ws'] : https ? ['wss', 'ws'] : ['ws'];
    const urls: string[] = [];
    try {
      const cached = localStorage.getItem(storageKey);
      if (cached) urls.push(cached);
    } catch {
      /* storage unavailable */
    }
    for (let p = portStart; p <= portEnd; p++) {
      for (const s of schemes) {
        const u = `${s}://${host}:${p}`;
        if (!urls.includes(u)) urls.push(u);
      }
    }
    return urls;
  }

  /** Opens a socket and resolves it once the gateway's hello arrives; null if it is not our gateway. */
  private probe(url: string): Promise<WebSocket | null> {
    return new Promise((resolve) => {
      let done = false;
      let ws: WebSocket;
      try {
        ws = new WebSocket(url);
      } catch {
        resolve(null);
        return;
      }
      const finish = (ok: boolean) => {
        if (done) return;
        done = true;
        clearTimeout(timer);
        ws.onmessage = ws.onerror = ws.onclose = null;
        if (!ok) ws.close();
        resolve(ok ? ws : null);
      };
      const timer = setTimeout(() => finish(false), PROBE_TIMEOUT_MS);
      ws.onerror = () => finish(false);
      ws.onclose = () => finish(false);
      ws.onmessage = (ev) => {
        try {
          const m = JSON.parse(String(ev.data));
          const deviceOk = !this.opts.deviceId || m.device_id === this.opts.deviceId;
          finish(m.type === 'hello' && m.gateway === GATEWAY_NAME && deviceOk);
        } catch {
          finish(false);
        }
      };
    });
  }

  private async connect(): Promise<void> {
    this.clearTimers();
    this.setState('discovering');
    let ws: WebSocket | null = null;
    for (const url of this.candidates()) {
      if (this.stopped) return;
      ws = await this.probe(url);
      if (ws) {
        this.url = url;
        try {
          localStorage.setItem(this.opts.storageKey, url);
        } catch {
          /* ignore */
        }
        break;
      }
    }
    if (this.stopped) {
      ws?.close();
      return;
    }
    if (!ws) {
      this.scheduleRetry('unavailable');
      return;
    }
    this.attach(ws);
  }

  private async attach(ws: WebSocket) {
    this.ws = ws;
    this.setState('authenticating');
    const authTimer = setTimeout(() => ws.close(), AUTH_TIMEOUT_MS);

    ws.onmessage = (ev) => {
      this.kickWatchdog(ws);
      let m: { type: string; [k: string]: unknown };
      try {
        m = JSON.parse(String(ev.data));
      } catch {
        return;
      }
      this.emit('message', m);
      switch (m.type) {
        case 'auth_ok':
          clearTimeout(authTimer);
          this.backoff = this.opts.reconnectMinMs;
          this.setState('connected');
          if (this.opts.raw) ws.send(JSON.stringify({ type: 'subscribe', raw: true }));
          break;
        case 'weight':
          this.latest = m as unknown as WeightReading;
          this.emit('weight', this.latest);
          break;
        case 'serial_status':
          this.serial = m as unknown as SerialStatus;
          this.emit('serial', this.serial);
          break;
        case 'gateway_status':
          this.gateway = m as unknown as GatewayStatus;
          this.emit('gateway', this.gateway);
          break;
        case 'error':
          this.emit('error', m as unknown as GatewayError);
          break;
      }
    };

    ws.onclose = (ev) => {
      clearTimeout(authTimer);
      if (this.ws !== ws) return;
      this.ws = null;
      if (this.stopped) return;
      if (ev.code === CLOSE_UNAUTHORIZED) {
        this.scheduleRetry('unauthorized', 30000);
      } else if (ev.code === CLOSE_RECONFIGURED) {
        this.scheduleRetry('unavailable', 500);
      } else {
        this.scheduleRetry('unavailable');
      }
    };
    ws.onerror = () => {
      /* onclose follows */
    };

    try {
      const t = this.opts.token;
      const token = typeof t === 'function' ? await t() : t;
      if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: 'auth', token }));
    } catch (e) {
      console.error('[weighbridge] could not obtain gateway token', e);
      ws.close();
    }
    this.kickWatchdog(ws);
  }

  private kickWatchdog(ws: WebSocket) {
    if (this.watchdog) clearTimeout(this.watchdog);
    this.watchdog = setTimeout(() => ws.close(4001, 'watchdog'), WATCHDOG_MS);
  }

  private scheduleRetry(state: ConnectionState, delay?: number) {
    this.clearTimers();
    this.setState(state);
    const wait = delay ?? this.backoff;
    this.backoff = Math.min(this.backoff * 2, this.opts.reconnectMaxMs);
    this.retryTimer = setTimeout(() => void this.connect(), wait);
  }
}
