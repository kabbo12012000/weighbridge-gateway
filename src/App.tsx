import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { api, errorText } from './api';
import type { AppConfig, Parsed, PortInfo, Snapshot } from './types';

type Tab = 'dashboard' | 'serial' | 'parser' | 'gateway' | 'monitor';
type Toast = { kind: 'ok' | 'err'; text: string } | null;

const TABS: { id: Tab; label: string }[] = [
  { id: 'dashboard', label: 'Dashboard' },
  { id: 'serial', label: 'Serial port' },
  { id: 'parser', label: 'Weight parsing' },
  { id: 'gateway', label: 'WebSocket & security' },
  { id: 'monitor', label: 'Monitor' },
];

const BAUD_RATES = [1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200];

export default function App() {
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [saved, setSaved] = useState<AppConfig | null>(null);
  const [draft, setDraft] = useState<AppConfig | null>(null);
  const [tab, setTab] = useState<Tab>('dashboard');
  const [toast, setToast] = useState<Toast>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.getConfig().then((c) => {
      setSaved(c);
      setDraft(c);
    });
    let alive = true;
    const poll = async () => {
      try {
        const s = await api.getState();
        if (alive) setSnap(s);
      } catch {
        /* window reloading */
      }
    };
    poll();
    const t = setInterval(poll, 400);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, []);

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), toast.kind === 'ok' ? 2500 : 6000);
    return () => clearTimeout(t);
  }, [toast]);

  const dirty = useMemo(() => JSON.stringify(saved) !== JSON.stringify(draft), [saved, draft]);

  const run = useCallback(async (fn: () => Promise<unknown>, ok?: string) => {
    setBusy(true);
    try {
      await fn();
      if (ok) setToast({ kind: 'ok', text: ok });
    } catch (e) {
      setToast({ kind: 'err', text: errorText(e) });
    } finally {
      setBusy(false);
    }
  }, []);

  const save = () =>
    run(async () => {
      const c = await api.saveConfig(draft!);
      setSaved(c);
      setDraft(c);
    }, 'Settings saved');

  const applyConfig = (c: AppConfig) => {
    setSaved(c);
    setDraft(c);
  };

  if (!snap || !draft) return <div className="loading">Starting gateway…</div>;

  return (
    <div className="app">
      <header className="header">
        <div>
          <h1>Weighbridge Gateway</h1>
          <span className="muted">
            {snap.device_id} · v{snap.version}
          </span>
        </div>
        <div className="pills">
          <Pill label="Serial" status={snap.serial.status} />
          <Pill label="WebSocket" status={snap.ws.status} />
          <span className="pill neutral">{snap.clients.length} client(s)</span>
        </div>
      </header>

      <nav className="tabs">
        {TABS.map((t) => (
          <button key={t.id} className={tab === t.id ? 'tab active' : 'tab'} onClick={() => setTab(t.id)}>
            {t.label}
          </button>
        ))}
      </nav>

      <main className="content">
        {snap.config_warning && <div className="banner err">{snap.config_warning}</div>}
        {tab === 'dashboard' && <Dashboard snap={snap} run={run} />}
        {tab === 'serial' && <SerialTab snap={snap} draft={draft} setDraft={setDraft} run={run} busy={busy} />}
        {tab === 'parser' && <ParserTab snap={snap} draft={draft} setDraft={setDraft} />}
        {tab === 'gateway' && (
          <GatewayTab snap={snap} draft={draft} setDraft={setDraft} run={run} dirty={dirty} applyConfig={applyConfig} />
        )}
        {tab === 'monitor' && <MonitorTab snap={snap} run={run} />}
      </main>

      {dirty && (
        <div className="savebar">
          <span>You have unsaved changes.</span>
          <button className="ghost" onClick={() => setDraft(saved)} disabled={busy}>
            Discard
          </button>
          <button className="primary" onClick={save} disabled={busy}>
            Save settings
          </button>
        </div>
      )}
      {toast && <div className={`toast ${toast.kind}`}>{toast.text}</div>}
    </div>
  );
}

// ---- shared bits ---------------------------------------------------------------------

type Run = (fn: () => Promise<unknown>, ok?: string) => Promise<void>;
type DraftProps = { draft: AppConfig; setDraft: (c: AppConfig) => void };

function tone(status: string): string {
  if (['connected', 'running'].includes(status)) return 'ok';
  if (['connecting', 'reconnecting', 'starting', 'restarting'].includes(status)) return 'warn';
  if (status === 'error') return 'err';
  return 'neutral';
}

function Pill({ label, status }: { label: string; status: string }) {
  return (
    <span className={`pill ${tone(status)}`}>
      <i className="dot" /> {label}: {status}
    </span>
  );
}

function Card({ title, children, actions }: { title: string; children: ReactNode; actions?: ReactNode }) {
  return (
    <section className="card">
      <div className="card-head">
        <h2>{title}</h2>
        {actions}
      </div>
      {children}
    </section>
  );
}

function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="field">
      <span className="label">{label}</span>
      {children}
      {hint && <span className="hint">{hint}</span>}
    </label>
  );
}

function Row({ k, v }: { k: string; v: ReactNode }) {
  return (
    <div className="kv">
      <span className="k">{k}</span>
      <span className="v">{v ?? '—'}</span>
    </div>
  );
}

function NumberInput({ value, onChange, step }: { value: number; onChange: (n: number) => void; step?: number }) {
  return <input type="number" value={value} step={step ?? 1} onChange={(e) => onChange(Number(e.target.value))} />;
}

function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (b: boolean) => void; label: string }) {
  return (
    <label className="toggle">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      {label}
    </label>
  );
}

function fmtTime(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  const age = Math.round((Date.now() - d.getTime()) / 1000);
  return `${d.toLocaleTimeString()} (${age <= 0 ? 'now' : `${age}s ago`})`;
}

function copy(text: string, run: Run) {
  return run(() => navigator.clipboard.writeText(text), 'Copied');
}

function Secret({ value, run }: { value: string; run: Run }) {
  const [show, setShow] = useState(false);
  return (
    <div className="secret">
      <code>{show ? value : '•'.repeat(24)}</code>
      <button className="ghost small" onClick={() => setShow(!show)}>
        {show ? 'Hide' : 'Show'}
      </button>
      <button className="ghost small" onClick={() => copy(value, run)}>
        Copy
      </button>
    </div>
  );
}

// ---- tabs ----------------------------------------------------------------------------

function Dashboard({ snap, run }: { snap: Snapshot; run: Run }) {
  const w = snap.last_weight;
  const stale = !w || Date.now() - new Date(w.timestamp).getTime() > 5000;
  return (
    <div className="grid">
      <section className={`weight-card ${stale ? 'stale' : w!.stable ? 'stable' : 'moving'}`}>
        <div className="weight-value">
          {w ? w.weight.toLocaleString(undefined, { maximumFractionDigits: 3 }) : '—'}
          <span className="unit">{w?.unit ?? ''}</span>
        </div>
        <div className="weight-meta">
          {stale ? 'No recent reading' : w!.stable ? 'STABLE' : 'MOVING'}
          {w && !stale && w.stability_source === 'motion' && <span className="muted"> (detected)</span>}
          {w && <span className="muted"> · {w.weight_type}</span>}
        </div>
      </section>

      <Card title="Serial connection">
        <Row k="Status" v={<Pill label="Serial" status={snap.serial.status} />} />
        <Row k="Source" v={snap.serial.source === 'simulator' ? 'Built-in simulator' : snap.serial.port} />
        <Row k="Message" v={snap.serial.message} />
        <Row k="Last data" v={snap.last_data && <code>{snap.last_data}</code>} />
        <Row k="Last received" v={fmtTime(snap.last_data_at)} />
        <div className="actions">
          <button className="primary" onClick={() => run(api.connect)}>
            {snap.serial.status === 'disconnected' || snap.serial.status === 'error' ? 'Connect' : 'Reconnect'}
          </button>
          <button className="ghost" onClick={() => run(api.disconnect)}>
            Disconnect
          </button>
        </div>
      </Card>

      <Card title="WebSocket server">
        <Row k="Status" v={<Pill label="WebSocket" status={snap.ws.status} />} />
        <Row
          k="URL"
          v={
            snap.ws.url && (
              <>
                <code>{snap.ws.url}</code>{' '}
                <button className="ghost small" onClick={() => copy(snap.ws.url!, run)}>
                  Copy
                </button>
              </>
            )
          }
        />
        <Row k="Secure URL" v={snap.ws.secure_url ? <code>{snap.ws.secure_url}</code> : 'Not set up'} />
        <Row k="Clients" v={snap.clients.length} />
        <Row k="Message" v={snap.ws.message} />
      </Card>

      <Card title="Recent problems">
        {snap.errors.length === 0 ? (
          <p className="muted">No errors.</p>
        ) : (
          <ul className="errors">
            {snap.errors.slice(0, 6).map((e, i) => (
              <li key={i}>
                <span className="muted">{new Date(e.timestamp).toLocaleTimeString()}</span> <b>{e.code}</b> {e.message}
              </li>
            ))}
          </ul>
        )}
      </Card>
    </div>
  );
}

function SerialTab({ snap, draft, setDraft, run, busy }: DraftProps & { snap: Snapshot; run: Run; busy: boolean }) {
  const [ports, setPorts] = useState<PortInfo[]>([]);
  const s = draft.serial;
  const set = (patch: Partial<AppConfig['serial']>) => setDraft({ ...draft, serial: { ...s, ...patch } });

  const refresh = useCallback(() => run(async () => setPorts(await api.listPorts())), [run]);
  useEffect(() => {
    refresh();
  }, [refresh]);

  const portKnown = ports.some((p) => p.name === s.port);

  return (
    <div className="grid">
      <Card
        title="Port settings"
        actions={
          <button className="ghost small" onClick={refresh} disabled={busy}>
            Refresh ports
          </button>
        }
      >
        <Field label="Data source">
          <select value={s.source} onChange={(e) => set({ source: e.target.value as 'serial' | 'simulator' })}>
            <option value="serial">Weighbridge indicator (serial port)</option>
            <option value="simulator">Built-in simulator (testing, no hardware)</option>
          </select>
        </Field>
        <div className="form-grid">
          <Field label="Serial port" hint={ports.length === 0 ? 'No ports detected' : undefined}>
            <select value={s.port} onChange={(e) => set({ port: e.target.value })} disabled={s.source !== 'serial'}>
              {!portKnown && <option value={s.port}>{s.port} (not detected)</option>}
              {ports.map((p) => (
                <option key={p.name} value={p.name}>
                  {p.name}
                  {p.description ? ` — ${p.description}` : ''}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Manual port name" hint="Type a port that is not in the list">
            <input value={s.port} onChange={(e) => set({ port: e.target.value })} disabled={s.source !== 'serial'} />
          </Field>
          <Field label="Baud rate">
            <select value={s.baud_rate} onChange={(e) => set({ baud_rate: Number(e.target.value) })}>
              {[...new Set([...BAUD_RATES, s.baud_rate])].sort((a, b) => a - b).map((b) => (
                <option key={b} value={b}>
                  {b}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Data bits">
            <select value={s.data_bits} onChange={(e) => set({ data_bits: Number(e.target.value) })}>
              {[5, 6, 7, 8].map((b) => (
                <option key={b}>{b}</option>
              ))}
            </select>
          </Field>
          <Field label="Stop bits">
            <select value={s.stop_bits} onChange={(e) => set({ stop_bits: Number(e.target.value) })}>
              <option value={1}>1</option>
              <option value={2}>2</option>
            </select>
          </Field>
          <Field label="Parity">
            <select value={s.parity} onChange={(e) => set({ parity: e.target.value as 'none' })}>
              <option value="none">None</option>
              <option value="odd">Odd</option>
              <option value="even">Even</option>
            </select>
          </Field>
          <Field label="Flow control">
            <select value={s.flow_control} onChange={(e) => set({ flow_control: e.target.value as 'none' })}>
              <option value="none">None</option>
              <option value="software">Software (XON/XOFF)</option>
              <option value="hardware">Hardware (RTS/CTS)</option>
            </select>
          </Field>
          <Field label="Frame delimiter" hint="AUTO accepts CR, LF or CRLF">
            <select value={s.delimiter} onChange={(e) => set({ delimiter: e.target.value as 'AUTO' })}>
              {['AUTO', 'CR', 'LF', 'CRLF', 'ETX'].map((d) => (
                <option key={d}>{d}</option>
              ))}
            </select>
          </Field>
        </div>
      </Card>

      <Card title="Connection behaviour">
        <Toggle checked={s.auto_connect} onChange={(b) => set({ auto_connect: b })} label="Connect automatically when the gateway starts" />
        <Toggle checked={s.auto_reconnect} onChange={(b) => set({ auto_reconnect: b })} label="Reconnect automatically after a disconnect" />
        <div className="form-grid">
          <Field label="First retry after (ms)">
            <NumberInput value={s.reconnect_min_ms} onChange={(n) => set({ reconnect_min_ms: n })} />
          </Field>
          <Field label="Longest retry interval (ms)">
            <NumberInput value={s.reconnect_max_ms} onChange={(n) => set({ reconnect_max_ms: n })} />
          </Field>
          <Field label="No-data warning after (ms)" hint="0 = off">
            <NumberInput value={s.silence_timeout_ms} onChange={(n) => set({ silence_timeout_ms: n })} />
          </Field>
          <Field label="Flush unterminated data after (ms)" hint="0 = never">
            <NumberInput value={s.idle_flush_ms} onChange={(n) => set({ idle_flush_ms: n })} />
          </Field>
        </div>
        <div className="actions">
          <button className="primary" onClick={() => run(api.connect)} disabled={busy}>
            Connect
          </button>
          <button className="ghost" onClick={() => run(api.disconnect)} disabled={busy}>
            Disconnect
          </button>
          <span className="muted">
            Current: {snap.serial.status}
            {snap.serial.message ? ` — ${snap.serial.message}` : ''}
          </span>
        </div>
        <p className="hint">Save settings first; Connect uses the saved configuration.</p>
      </Card>
    </div>
  );
}

function ParserTab({ snap, draft, setDraft }: DraftProps & { snap: Snapshot }) {
  const p = draft.parser;
  const set = (patch: Partial<AppConfig['parser']>) => setDraft({ ...draft, parser: { ...p, ...patch } });
  const list = (v: string[]) => v.join(', ');
  const parseList = (s: string) => s.split(',').map((x) => x.trim()).filter(Boolean);
  const [sample, setSample] = useState('');
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  const test = async (text: string) => {
    try {
      const r: Parsed = await api.testParser(draft, text);
      setResult({ ok: true, text: `${r.weight} ${r.unit} · ${r.stability} · ${r.weight_type}` });
    } catch (e) {
      setResult({ ok: false, text: errorText(e) });
    }
  };

  return (
    <div className="grid">
      <Card title="Parsing rules">
        <Field label="Mode">
          <select value={p.mode} onChange={(e) => set({ mode: e.target.value as 'auto' })}>
            <option value="auto">Auto — find the weight number and status codes</option>
            <option value="demo">Demo — strict "ST,GS,+001250 kg" format</option>
            <option value="regex">Regex — custom pattern for this indicator</option>
            <option value="raw">Raw — don't parse (to inspect an unknown protocol)</option>
          </select>
        </Field>
        {p.mode === 'regex' && (
          <Field label="Pattern" hint="Named groups: weight (required), sign, unit, stability, type. Example: ^WT:\s*(?<sign>[+-]?)\s*(?<weight>\d+)\s*(?<unit>kg)\s*(?<stability>[SU])$">
            <input className="mono" value={p.regex} onChange={(e) => set({ regex: e.target.value })} />
          </Field>
        )}
        <div className="form-grid">
          <Field label="Scale factor" hint="Multiply the number, e.g. 10 for 10 kg divisions">
            <NumberInput value={p.scale} step={0.001} onChange={(n) => set({ scale: n })} />
          </Field>
          <Field label="Default unit">
            <input value={p.default_unit} onChange={(e) => set({ default_unit: e.target.value })} />
          </Field>
          <Field label="Maximum valid weight">
            <NumberInput value={p.max_weight} onChange={(n) => set({ max_weight: n })} />
          </Field>
          <Field label="Stable codes">
            <input value={list(p.stable_codes)} onChange={(e) => set({ stable_codes: parseList(e.target.value) })} />
          </Field>
          <Field label="Unstable / motion codes">
            <input value={list(p.unstable_codes)} onChange={(e) => set({ unstable_codes: parseList(e.target.value) })} />
          </Field>
          <Field label="Overload codes">
            <input value={list(p.overload_codes)} onChange={(e) => set({ overload_codes: parseList(e.target.value) })} />
          </Field>
        </div>
        <Toggle
          checked={p.reverse_digits}
          onChange={(b) => set({ reverse_digits: b })}
          label="Digits arrive reversed (least significant first, e.g. =05452000 → 25450)"
        />
      </Card>

      <Card title="Stability when the indicator sends no flag">
        <p className="hint">
          If a frame has no stable/motion code, the weight is reported stable once it stays within the tolerance for the
          window below.
        </p>
        <div className="form-grid">
          <Field label="Window (ms)">
            <NumberInput value={p.motion_window_ms} onChange={(n) => set({ motion_window_ms: n })} />
          </Field>
          <Field label="Tolerance (same unit as weight)">
            <NumberInput value={p.motion_tolerance} step={0.1} onChange={(n) => set({ motion_tolerance: n })} />
          </Field>
        </div>
      </Card>

      <Card title="Test the rules (uses unsaved settings)">
        <Field label="Sample frame">
          <input className="mono" value={sample} onChange={(e) => setSample(e.target.value)} placeholder="ST,GS,+025450 kg" />
        </Field>
        <div className="actions">
          <button className="primary" onClick={() => test(sample)}>
            Test
          </button>
          {snap.raw[0] && (
            <button
              className="ghost"
              onClick={() => {
                setSample(snap.raw[0].text);
                test(snap.raw[0].text);
              }}
            >
              Use last received frame
            </button>
          )}
        </div>
        {result && <div className={`banner ${result.ok ? 'ok' : 'err'}`}>{result.text}</div>}
      </Card>
    </div>
  );
}

function GatewayTab({
  snap,
  draft,
  setDraft,
  run,
  dirty,
  applyConfig,
}: DraftProps & { snap: Snapshot; run: Run; dirty: boolean; applyConfig: (c: AppConfig) => void }) {
  const w = draft.websocket;
  const set = (patch: Partial<AppConfig['websocket']>) => setDraft({ ...draft, websocket: { ...w, ...patch } });
  const [autostart, setAutostart] = useState<boolean | null>(null);
  useEffect(() => {
    api.getAutostart().then(setAutostart).catch(() => setAutostart(null));
  }, []);

  const regenerate = (which: 'token' | 'signing_secret', what: string) => {
    if (dirty && !confirm('Unsaved changes will be discarded. Continue?')) return;
    if (!confirm(`Generate a new ${what}? Every place that uses the old one must be updated.`)) return;
    run(async () => applyConfig(await api.regenerate(which)), `New ${what} saved`);
  };

  return (
    <div className="grid">
      <Card title="Device">
        <Field label="Device ID" hint="Must match the weighbridge record in Laravel">
          <input value={draft.device_id} onChange={(e) => setDraft({ ...draft, device_id: e.target.value })} />
        </Field>
        <Toggle
          checked={!!autostart}
          onChange={(b) => run(async () => {
            await api.setAutostart(b);
            setAutostart(b);
          }, b ? 'Gateway will start with Windows' : 'Autostart off')}
          label="Start the gateway when Windows starts (minimised to tray)"
        />
      </Card>

      <Card title="Endpoint">
        <Row k="Active URL" v={snap.ws.url ? <code>{snap.ws.url}</code> : snap.ws.status} />
        <Row k="Secure URL" v={snap.ws.secure_url ? <code>{snap.ws.secure_url}</code> : '—'} />
        <div className="form-grid">
          <Field label="Port range start">
            <NumberInput value={w.port_start} onChange={(n) => set({ port_start: n })} />
          </Field>
          <Field label="Port range end">
            <NumberInput value={w.port_end} onChange={(n) => set({ port_end: n })} />
          </Field>
        </div>
        <p className="hint">The first free port in the range is used. The web app scans the same range to find the gateway.</p>
      </Card>

      <Card title="Access control">
        <Field label="Allowed origins (one per line)" hint="e.g. https://erp.example.com or https://*.example.com">
          <textarea
            rows={4}
            className="mono"
            value={w.allowed_origins.join('\n')}
            onChange={(e) => set({ allowed_origins: e.target.value.split('\n') })}
          />
        </Field>
        <Toggle checked={w.require_auth} onChange={(b) => set({ require_auth: b })} label="Require access token" />
        <Toggle
          checked={w.allow_missing_origin}
          onChange={(b) => set({ allow_missing_origin: b })}
          label="Allow non-browser clients (no Origin header; still need the token)"
        />
        <Field label="Access token" hint="Configure this in Laravel's weighbridge device settings; the web app sends it on connect">
          <Secret value={w.token} run={run} />
        </Field>
        <button className="ghost small" onClick={() => regenerate('token', 'access token')}>
          Generate new token
        </button>
      </Card>

      <Card title="Reading signatures">
        <p className="hint">
          Every weight message is signed (HMAC-SHA256). Laravel verifies the signature on save, so a weight cannot be typed
          or edited in the browser. Store this secret only in Laravel — never in the frontend.
        </p>
        <Field label="Signing secret">
          <Secret value={w.signing_secret} run={run} />
        </Field>
        <button className="ghost small" onClick={() => regenerate('signing_secret', 'signing secret')}>
          Generate new signing secret
        </button>
      </Card>

      <Card title="Secure WebSocket (wss://) for HTTPS sites">
        <p className="hint">
          Chrome, Edge and Firefox allow an HTTPS page to use <code>ws://127.0.0.1</code>. If your browser blocks it, set
          up a locally trusted certificate. Windows asks you to confirm installing it.
        </p>
        <Row k="Status" v={snap.tls_configured ? 'Certificate installed — wss:// available' : 'Not set up'} />
        <div className="actions">
          <button className="primary" onClick={() => run(api.setupTls, 'Secure WebSocket enabled')}>
            {snap.tls_configured ? 'Recreate certificate' : 'Set up wss://'}
          </button>
          {snap.tls_configured && (
            <button className="ghost" onClick={() => run(api.removeTls, 'Certificate removed')}>
              Remove
            </button>
          )}
        </div>
      </Card>

      <Card title="Connected clients">
        {snap.clients.length === 0 ? (
          <p className="muted">No clients connected.</p>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>#</th>
                <th>Origin</th>
                <th>Secure</th>
                <th>Since</th>
              </tr>
            </thead>
            <tbody>
              {snap.clients.map((c) => (
                <tr key={c.id}>
                  <td>{c.id}</td>
                  <td>{c.origin ?? '(no origin)'}</td>
                  <td>{c.secure ? 'wss' : 'ws'}</td>
                  <td>{new Date(c.connected_at).toLocaleTimeString()}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>

      <Card title="Files">
        <Row k="Settings" v={<code>{snap.paths.config_file}</code>} />
        <Row
          k="Logs"
          v={
            <>
              <code>{snap.paths.log_dir}</code>{' '}
              <button className="ghost small" onClick={() => run(() => api.openFolder(snap.paths.log_dir))}>
                Open
              </button>
            </>
          }
        />
      </Card>
    </div>
  );
}

function MonitorTab({ snap, run }: { snap: Snapshot; run: Run }) {
  const [hex, setHex] = useState(false);
  const [paused, setPaused] = useState<Snapshot['raw'] | null>(null);
  const rows = paused ?? snap.raw;
  return (
    <div className="grid single">
      <Card
        title={`Raw serial data (${snap.stats.frames} frames, ${snap.stats.bytes} bytes, ${snap.stats.parse_failures} not parsed)`}
        actions={
          <div className="actions">
            <Toggle checked={hex} onChange={setHex} label="Hex" />
            <button className="ghost small" onClick={() => setPaused(paused ? null : snap.raw)}>
              {paused ? 'Resume' : 'Pause'}
            </button>
            <button className="ghost small" onClick={() => run(api.clearMonitor)}>
              Clear
            </button>
          </div>
        }
      >
        <div className="raw">
          {rows.length === 0 && <p className="muted">Nothing received yet.</p>}
          {rows.map((r, i) => (
            <div key={i} className={`raw-row ${r.status}`}>
              <span className="muted">{new Date(r.timestamp).toLocaleTimeString()}</span>
              <code>{hex ? r.hex : r.text}</code>
              <span className="muted">{r.status !== 'complete' ? r.status : ''}</span>
              <span className="result">{r.result ?? ''}</span>
            </div>
          ))}
        </div>
      </Card>
      <Card title="Errors">
        {snap.errors.length === 0 ? (
          <p className="muted">No errors.</p>
        ) : (
          <ul className="errors">
            {snap.errors.map((e, i) => (
              <li key={i}>
                <span className="muted">{new Date(e.timestamp).toLocaleString()}</span> <b>{e.code}</b> {e.message}
              </li>
            ))}
          </ul>
        )}
      </Card>
    </div>
  );
}
