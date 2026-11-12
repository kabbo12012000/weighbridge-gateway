# Weighbridge Gateway (Tauri 2)

A small Windows desktop app that reads a weighbridge indicator over RS-232 and serves the
readings to the cloud-hosted Laravel + React app through a **local WebSocket on 127.0.0.1**.

```
Indicator ──RS-232──► Gateway (Rust: serial + WebSocket) ──ws://127.0.0.1:8765──► React page in the browser
                                                                                     │ operator presses Save
                                                                                     ▼ HTTPS
                                                                              Laravel ──► MySQL
```

The gateway **never saves anything**. Saving happens only when the operator presses Save in the web app,
and Laravel checks each reading before it stores it.

| Folder | What it is |
| --- | --- |
| `src-tauri/` | Rust backend: serial worker, frame splitting, parser, WebSocket server, TLS, tray. |
| `src/` | Gateway UI (React + TS): settings, status, raw monitor. |
| `client/` | **Drop-in files for the Laravel React app**: `weighbridge-client.ts`, `useWeighbridge.ts`, an example form. |
| `laravel-integration/` | Reference Laravel code: migration, models, signature check, request validation, idempotent controller. |

The frame splitting and parsing rules match the Node gateway in `../js project` (same AUTO/CR/LF/CRLF/ETX
delimiters, idle flush, and `demo`/`regex`/`raw` parsers), so the indicator settings you worked out there still apply.

---

## Build

Prerequisites (developer PC only; operators just run the installer):

1. **Rust** (stable, MSVC): <https://rustup.rs>
2. **Visual Studio 2022 Build Tools** with the *Desktop development with C++* workload
3. **Node.js 20+**
4. WebView2 is already installed on Windows 10/11. The installer embeds the WebView2 bootstrapper as a fallback.

```powershell
npm install
npm run test:rust        # unit tests: framing, parsers, motion detection, signatures, origins
npm run tauri dev        # run with hot reload
npm run tauri build      # -> src-tauri/target/release/bundle/{nsis,msi}/
```

To change the icon, run `npm run tauri icon path\to\logo.png`.

## Operator setup (once per weighbridge PC)

1. Run the installer. The gateway starts and puts an icon in the system tray. Closing the window keeps it
   running in the background; quit it from the tray menu.
2. **Serial port** tab: choose the COM port (default COM2, 9600 8-N-1, no flow control), then **Save** and
   **Connect**. Turn on *Connect automatically* and *Reconnect automatically*.
3. **Monitor** tab: check that frames arrive and that the *result* column shows a weight. If it doesn't,
   adjust the **Weight parsing** tab and test it with *Use last received frame*.
4. **WebSocket & security** tab:
   - Set **Device ID** (e.g. `WB-001`) to match the Laravel device record.
   - Add the web app's origin (e.g. `https://erp.example.com`) to **Allowed origins**.
   - Copy the **Access token** and **Signing secret** into the Laravel device record.
   - Turn on **Start with Windows**.

No Node.js, terminals or manual background processes are needed on the weighbridge PC.

### Testing without hardware

Set **Data source** to *Built-in simulator*. Every 40 s it simulates a truck driving on, settling and
driving off, sending `ST,GS,+025450 kg` frames in small chunks, like a real serial port.

### Manual weight check

When hardware is unavailable, `manual_scale.py` can send test weight frames through a virtual
serial-port pair. From the `weighbridge` folder, create or activate the virtual environment and
install `pyserial`:

```powershell
.\.venv\Scripts\python.exe -m pip install pyserial
```

Set `PORT` in `manual_scale.py` to the COM port connected to the gateway (for example `COM3`),
then run:

```powershell
.\.venv\Scripts\python.exe .\manual_scale.py
```

Enter a positive weight in kilograms, such as `25450`, and press Enter. The script sends a frame
like `ST,GS,+025450 kg`. Type `q` to stop. The gateway must be configured for the other end of
the virtual serial-port pair; close any other program using that COM port before starting the script.

## Weight parsing

| Mode | Use for |
| --- | --- |
| `auto` (default) | Most indicators. Picks the weight number (preferring one with a unit, then the longest) and reads stable, unstable, overload and gross/net/tare codes as separate tokens. Handles `ST,GS,25450kg`, `US,NT,-120 kg`, `<STX>+025450<ETX>`. |
| `demo` | Strict `ST,GS,+001250 KG` format. |
| `regex` | Custom pattern with named groups `weight` (required), `sign`, `unit`, `stability`, `type`. |
| `raw` | No parsing. Use it to capture an unknown protocol from the Monitor tab. |

Other options: **reverse digits** (for indicators that send the least significant digit first, e.g. `=05452000` → 25450),
**scale factor**, **max weight**, and the code lists.

**Stability:** if the indicator sends a stable or motion flag, the gateway uses it (`stability_source: "indicator"`).
If it doesn't, the reading counts as stable once the weight has stayed within the tolerance for the window
(default 1.5 s) (`stability_source: "motion"`).

## WebSocket protocol

Endpoint: `ws://127.0.0.1:<port>`. The port is the first free one in 8765–8800 and is shown in the UI.
Clients find it by scanning that range (`client/weighbridge-client.ts` does this and remembers the last URL that worked).

1. On connect, the server sends
   `{"type":"hello","gateway":"weighbridge-gateway","version":"1.0.0","device_id":"WB-001","auth_required":true}`.
2. The client must reply within 10 s with `{"type":"auth","token":"<access token>"}`.
3. The server replies `{"type":"auth_ok"}` and then sends `gateway_status`, `serial_status` and the last `weight`.

Messages from the server:

| `type` | When |
| --- | --- |
| `weight` | Every parsed reading. See the example below. |
| `serial_status` | `connected` / `connecting` / `reconnecting` / `disconnected` / `error`, with a `message`. |
| `gateway_status` | When the server starts, and when a client connects or disconnects (`event` field), with the client count and URLs. |
| `serial_data` | Every raw frame (`raw_data`, `hex`, `frame_status`). Sent only to clients that send `{"type":"subscribe","raw":true}`. |
| `error` | `serial_open_failed`, `serial_disconnected`, `no_data`, `parse_failed`, `out_of_range`, `auth_failed`, … |
| `heartbeat` | Every 5 s, with `serial_status` and `last_data_at`. |
| `pong` | Reply to `{"type":"ping"}`. |

Client → server: `auth`, `ping`, `subscribe` (`raw: bool`), `get_status`.

```json
{
  "type": "weight",
  "device_id": "WB-001",
  "reading_id": "9f3a1c02-1842",
  "weight": 25450.0,
  "unit": "kg",
  "stable": true,
  "stability_source": "indicator",
  "weight_type": "gross",
  "raw_data": "ST,GS,25450kg",
  "timestamp": "2026-11-11T12:30:00.000Z",
  "signature": "5d1e…(64 hex)"
}
```

Close codes: `4401` = bad token (the client retries every 30 s), `4000` = settings changed (reconnect now).

## Security model

| Threat | Mitigation |
| --- | --- |
| Other PCs on the network | The server binds to `127.0.0.1` only and rejects non-loopback peers. |
| Other websites open in the same browser | The `Origin` header is checked against **Allowed origins** during the upgrade (HTTP 403). |
| Local programs or unknown pages | A 256-bit **access token** is required as the first message. Comparison is constant-time, and each failure costs 1 s. |
| Operator edits the weight in DevTools | Each reading carries an **HMAC-SHA256 signature**, keyed with a secret that only the gateway and Laravel know. Laravel checks it before saving. |
| Double-clicked Save or network retry | An `Idempotency-Key` per capture, plus a unique `reading_id` in the database. |
| Saving an old reading | Laravel rejects readings older than `max_reading_age_seconds` (default 30 min). |

Secrets are stored in `%APPDATA%\com.barkat.weighbridge-gateway\gateway.json`, which is per-user and covered by
normal Windows ACLs. Changing the token or the allowed origins disconnects every client so they authenticate again.
Logs are written daily to `%LOCALAPPDATA%\com.barkat.weighbridge-gateway\logs\` and kept for 14 days.

### HTTPS pages and mixed content

Browsers treat `127.0.0.1` as a secure context. Chrome, Edge and Firefox therefore let an `https://` page open
`ws://127.0.0.1:…` without a mixed-content error. Recent Chrome and Edge versions may show a one-time
**"Access other apps and services on this device"** (local network access) prompt; the operator must allow it.
An enterprise policy can pre-approve it for the ERP origin.

If a browser still blocks it (for example Safari, or a locked-down policy), use **WebSocket & security → Set up wss://**:

- The gateway creates a local CA and a certificate for `127.0.0.1` / `localhost`, and **throws the CA private key away**,
  so the trusted root can never sign anything else.
- It installs the CA in the *current user's* Trusted Root store. Windows asks the operator to confirm.
- `wss://` and `ws://` are then both served on the **same port**. The client tries `wss://` first on HTTPS pages.
- Chrome and Edge use the Windows store automatically. Firefox needs `security.enterprise_roots.enabled = true`.

## Integrating the Laravel + React app

1. Copy `client/weighbridge-client.ts` and `client/useWeighbridge.ts` into the frontend.
2. Give the weighment page the device record (`device_id`, `gateway_token`, `require_stable`), either as an
   Inertia prop or from `GET /weighbridge/devices/{device_id}/gateway`. **Never send the `signing_secret` to the browser.**
3. Use the hook. `WeighmentFormExample.tsx` shows the full flow:

   ```tsx
   const wb = useWeighbridge({ token: device.gateway_token, deviceId: device.device_id });
   // wb.connection, wb.weight (live or null if stale), wb.serial, wb.capture()
   ```

   - The live weight updates continuously.
   - **Capture** freezes a snapshot, including the signature, `measured_at` and `captured_at`.
   - **Save** POSTs `{ vehicle_number, customer_id, product_id, reading }` with an `Idempotency-Key` header.
4. On the Laravel side, adapt `laravel-integration/`. In the modular ERP, the files belong in a module rather than `app/`.
   - The migration creates `weighbridge_devices` and `weighments`.
   - `StoreWeighmentRequest` checks the device, signature, weight range, stability and reading age.
   - `WeighmentController@store` saves exactly once per idempotency key and returns 409 when a reading is reused.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| `Access denied - the port is in use` | Another program has the COM port open (indicator software, the old Node gateway). |
| Connected but no frames | Baud rate, parity or cable (null-modem vs straight). Watch the Monitor tab and the `no_data` warning. |
| Frames shown but `No match` / `No number` | Weight parsing tab: try `raw`, look at the frames, then set up `auto` or `regex`. |
| Frames shown as `overflow` | Wrong delimiter. Try `ETX` or `CR`. |
| Web app says *Gateway not found* | Check that the gateway is running (tray icon) and that the page origin is in Allowed origins. The Errors list shows `origin_rejected`. |
| Web app says token rejected | The token in Laravel doesn't match the gateway's token. |
| Laravel says *could not be verified* | The signing secret in Laravel doesn't match the gateway's, or Device ID differs. |
| Laravel says *too old* | Capture again, or fix the weighbridge PC clock. |
