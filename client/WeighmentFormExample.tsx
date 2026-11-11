/**
 * Example operator form. Live weight is displayed continuously; nothing is saved until the
 * operator presses Capture and then Save. Adapt the fields/endpoint to the Laravel app.
 */
import { useMemo, useState, type FormEvent } from 'react';
import type { CapturedWeight } from './weighbridge-client';
import { useWeighbridge } from './useWeighbridge';

interface Props {
  /** From Laravel (e.g. Inertia page props): the device this PC's gateway serves. */
  device: { device_id: string; gateway_token: string; require_stable: boolean };
  saveUrl: string; // e.g. route('weighbridge.weighments.store')
}

function xsrfToken(): string {
  const m = document.cookie.match(/(?:^|;\s*)XSRF-TOKEN=([^;]+)/);
  return m ? decodeURIComponent(m[1]) : '';
}

const CONNECTION_TEXT: Record<string, string> = {
  idle: 'Not connected',
  discovering: 'Looking for the weighbridge gateway…',
  authenticating: 'Connecting…',
  connected: 'Connected',
  unavailable: 'Gateway not found — is the Weighbridge Gateway running on this PC?',
  unauthorized: 'Gateway rejected the access token — check the device settings',
};

export function WeighmentFormExample({ device, saveUrl }: Props) {
  const wb = useWeighbridge({ token: device.gateway_token, deviceId: device.device_id });
  const [captured, setCaptured] = useState<CapturedWeight | null>(null);
  const [vehicle, setVehicle] = useState('');
  const [customerId, setCustomerId] = useState('');
  const [productId, setProductId] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
  // One key per capture: retries/double-clicks of the same capture can never create two rows.
  const idempotencyKey = useMemo(() => (captured ? crypto.randomUUID() : null), [captured]);

  const capture = () => {
    const snap = wb.capture();
    if (!snap) return setMessage({ ok: false, text: 'No live weight to capture.' });
    if (device.require_stable && !snap.stable) return setMessage({ ok: false, text: 'Wait until the weight is stable.' });
    setCaptured(snap);
    setMessage(null);
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!captured || submitting) return;
    setSubmitting(true);
    setMessage(null);
    try {
      const res = await fetch(saveUrl, {
        method: 'POST',
        credentials: 'same-origin',
        headers: {
          'Content-Type': 'application/json',
          Accept: 'application/json',
          'X-XSRF-TOKEN': xsrfToken(),
          'Idempotency-Key': idempotencyKey!,
        },
        body: JSON.stringify({
          vehicle_number: vehicle,
          customer_id: customerId,
          product_id: productId,
          reading: captured,
        }),
      });
      const body = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(body.message ?? `Save failed (${res.status})`);
      setMessage({ ok: true, text: `Saved weighment #${body.id}` });
      setCaptured(null);
      setVehicle('');
    } catch (err) {
      setMessage({ ok: false, text: err instanceof Error ? err.message : String(err) });
    } finally {
      setSubmitting(false);
    }
  };

  const live = wb.weight;

  return (
    <form onSubmit={submit}>
      <p>{CONNECTION_TEXT[wb.connection]}</p>
      {wb.serial && wb.serial.status !== 'connected' && (
        <p>Indicator: {wb.serial.status} {wb.serial.message ?? ''}</p>
      )}

      <div style={{ fontSize: 48, fontFamily: 'monospace' }}>
        {live ? `${live.weight} ${live.unit}` : '— — —'}
        <small style={{ marginLeft: 12 }}>{live ? (live.stable ? 'STABLE' : 'MOVING') : ''}</small>
      </div>
      <button type="button" onClick={capture} disabled={!live || submitting}>
        Capture weight
      </button>

      {captured && (
        <p>
          Captured: <b>{captured.weight} {captured.unit}</b> ({captured.stable ? 'stable' : 'unstable'}) at{' '}
          {new Date(captured.captured_at).toLocaleTimeString()}{' '}
          <button type="button" onClick={() => setCaptured(null)}>
            Discard
          </button>
        </p>
      )}

      <input placeholder="Vehicle number" value={vehicle} onChange={(e) => setVehicle(e.target.value)} required />
      <input placeholder="Customer" value={customerId} onChange={(e) => setCustomerId(e.target.value)} required />
      <input placeholder="Product" value={productId} onChange={(e) => setProductId(e.target.value)} required />

      <button type="submit" disabled={!captured || submitting}>
        {submitting ? 'Saving…' : 'Save weighment'}
      </button>
      {message && <p style={{ color: message.ok ? 'green' : 'crimson' }}>{message.text}</p>}
    </form>
  );
}
