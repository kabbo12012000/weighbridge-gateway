import { useCallback, useEffect, useRef, useState } from 'react';
import {
  WeighbridgeClient,
  type CapturedWeight,
  type ConnectionState,
  type GatewayError,
  type SerialStatus,
  type WeighbridgeClientOptions,
  type WeightReading,
} from './weighbridge-client';

export interface UseWeighbridge {
  connection: ConnectionState;
  /** Latest live reading, or null when there is none / it is stale. */
  weight: WeightReading | null;
  serial: SerialStatus | null;
  lastError: GatewayError | null;
  gatewayUrl: string | null;
  capture: () => CapturedWeight | null;
}

/**
 * React binding for the gateway. Pass `enabled: false` until the token is known.
 *
 *   const wb = useWeighbridge({ token: device.gateway_token, deviceId: device.device_id });
 */
export function useWeighbridge(options: WeighbridgeClientOptions & { enabled?: boolean }): UseWeighbridge {
  const { enabled = true, ...clientOptions } = options;
  const clientRef = useRef<WeighbridgeClient | null>(null);
  const [connection, setConnection] = useState<ConnectionState>('idle');
  const [weight, setWeight] = useState<WeightReading | null>(null);
  const [serial, setSerial] = useState<SerialStatus | null>(null);
  const [lastError, setLastError] = useState<GatewayError | null>(null);
  const [, setTick] = useState(0);

  // Re-create the client only when connection-relevant options change.
  const key = JSON.stringify({ ...clientOptions, token: typeof clientOptions.token === 'string' ? clientOptions.token : 'fn' });
  const optionsRef = useRef(clientOptions);
  optionsRef.current = clientOptions;

  useEffect(() => {
    if (!enabled) return;
    const client = new WeighbridgeClient(optionsRef.current);
    clientRef.current = client;
    const offs = [
      client.on('state', (s) => {
        setConnection(s);
        if (s !== 'connected') setWeight(null);
      }),
      client.on('weight', setWeight),
      client.on('serial', setSerial),
      client.on('error', setLastError),
    ];
    client.start();
    // Re-render periodically so a reading that stops arriving is shown as stale.
    const t = setInterval(() => setTick((n) => n + 1), 1000);
    return () => {
      clearInterval(t);
      offs.forEach((off) => off());
      client.stop();
      clientRef.current = null;
    };
  }, [enabled, key]);

  const live = clientRef.current?.current() ?? null;

  return {
    connection,
    weight: live && weight ? weight : null,
    serial,
    lastError,
    gatewayUrl: clientRef.current?.url ?? null,
    capture: useCallback(() => clientRef.current?.capture() ?? null, []),
  };
}
