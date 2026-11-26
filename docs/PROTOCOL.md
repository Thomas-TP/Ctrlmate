# Ctrlmate wire and IPC protocol (v0.6)

Types live in `crates/ctrlmate-protocol`; this file summarises how they travel. The architecture
contract (`ARCHITECTURE.md`) is binding; this document describes what `ctrlmated` implements.

## 1. Endpoints

| Side | Address | Transport | Clients |
|---|---|---|---|
| LAN | `0.0.0.0:9487` | TLS **1.3 only**, HTTP/1.1 + WebSocket | Home Assistant |
| IPC | `127.0.0.1:9488` | plain HTTP/WebSocket, loopback | desktop app (via Rust commands), `ctrlmate-agent` |
| MQTT | outbound | optional | HA MQTT discovery |
| mDNS | UDP 5353 | `_ctrlmate._tcp.local.` | HA zeroconf |

mDNS TXT: `id` (device id), `name` (host name), `ver` (daemon version), `fp` (full 64-hex SHA-256 of
the served certificate). The MAC address is not broadcast.

## 2. LAN API (Home Assistant)

* `GET /api/v1/info` (no auth) returns `protocol_version`, `device_id`, `sw_version`,
  `cert_fingerprint`, `pairing_active`. With `Authorization: Bearer <paired token>` it also returns
  `hostname` and `mac_address`.
* `POST /api/v1/pair` body `{"pin","client_name"}`.
  * `200` `PairResponse {client_id, token, device_id, hostname, mac_address, cert_fingerprint}`;
    `cert_fingerprint` is the SHA-256 of the certificate being served.
  * `401` `{"ok":false,"code":"invalid_pin"|"no_session","error":...,"retry_after_s":n}`.
  * `429` `code:"throttled"` with a `Retry-After` header.
  * `409` too many paired clients (16).
  * Throttling is per source IP: 5 wrong PINs per pairing session and an exponential backoff
    (1, 2, 4 ... 300 s). A stranger cannot end the real user's session; only 100 failures from many
    hosts cancel it.
  * `client_name` is sanitised (control characters removed, whitespace collapsed, 64 chars max).
  * Tokens are `cm2_` + 192 random bits; only SHA-256 is stored.
* `GET /ws` WebSocket (`WireMessage` JSON text frames, 64 KiB max):
  1. server `hello {protocol_version, device_id, hostname:"" , sw_version, cert_fingerprint}`
     (`device_id` is the stored id; the hostname is withheld before authentication)
  2. client `auth {token, client_name}` within 10 s; failure gives `auth_error {reason:"invalid_token"}`
     and closes the socket
  3. server `auth_ok`, `manifest`, `state_batch` (full state, only when `permissions.telemetry`)
  4. then deltas (`state_batch`, `manifest`), `command` / `command_result`, `ping` / `pong`
* `command_result {request_id, ok, code, message?}`. Codes: `ok`, `permission_denied`, `no_agent`,
  `agent_timeout`, `not_supported`, `invalid_argument`, `hardware_error`, `busy`, `unavailable`,
  `internal`. An unparseable frame is answered with `invalid_argument` (the `request_id` is echoed
  when it can be read). Up to 4 commands run concurrently per connection (`busy` beyond).
* Revoking a client sends `auth_error {reason:"token_revoked"}` and closes every live socket of
  that client immediately. Each socket has its own connection id inside the daemon.
* A receiver that lagged behind the broadcast gets `manifest` + full `state_batch` again.
* `permissions.telemetry = false`: no state frames to any client and no MQTT state publishing.
  Re-enabling it resends the full state.
* Limits: 64 concurrent TCP connections, 8 per IP, 10 s TLS handshake, 10 s header read timeout.

### State values

`EntityState {id, value, attributes, updated_at}` with `value` = `{"kind": "float|int|bool|text|light|update|unavailable", "value": ...}`.
A missing sensor is `{"kind":"unavailable"}` (or absent), never a fabricated number.

`update` carries `UpdatesInfo`: `checked`, `last_checked`, `pending_count` (null until a scan
succeeded), `pending[{title,kb}]`, `titles`, `kb_ids`, `reboot_required`, `in_progress`, `error`
(result code), `installed_version`. There is no `latest_version`.

Daemon-owned entity: `agent_connected` (binary sensor, diagnostic, parent device, `connectivity`).

Session-bound entities (`active_application`, `gaming_mode`, `microphone_in_use`, `webcam_in_use`,
`master_volume`, `audio_mute`, `audio_output`) always come from the agent's `SessionState`
(or the in-process fallback in dev runs) and are `unavailable` without one.

## 3. IPC API (desktop app)

Every request must carry `Host: 127.0.0.1:<port>` or `localhost:<port>` (else `421`) and **no**
`Origin` header (else `403`). There is no CORS layer. Every REST route needs
`Authorization: Bearer <ipc token>` (else `401`).

| Route | Body | Result |
|---|---|---|
| `GET /ipc/snapshot` | | `DaemonSnapshot` |
| `POST /ipc/pairing/start` / `cancel` | | `DaemonSnapshot` |
| `DELETE /ipc/clients/{client_id}` | | `DaemonSnapshot` (`404` unknown) |
| `POST /ipc/command` | `ControlCommand` | `CommandResult` (HTTP 200) |
| `PUT /ipc/settings` | `SettingsUpdate` | `DaemonSnapshot`, or `400 CommandResult` |
| `POST /ipc/hardware/rescan` | | `DaemonSnapshot` |
| `GET /ipc/ws` (WebSocket) | first frame `{"type":"auth","token"}` within 5 s | `IpcEvent` frames: `auth_ok`, then `snapshot` on every change |
| `GET /ipc/agent` (WebSocket) | first frame `auth`, then the agent protocol | see section 4 |

Token: 32 random bytes, hex, `ipc.token` in the data dir, constant-time comparison.
`ctrlmated --rotate-ipc-token` replaces it; a running daemon notices within 2 s, closes every IPC
socket and rejects the old token. Clients look in: env `CTRLMATE_IPC_TOKEN`, the per-user data dir,
the system data dir.

Data dirs: Windows service `%ProgramData%\Ctrlmate` (SYSTEM + Administrators; `Users` read on
`ipc.token` only), Linux service `/var/lib/ctrlmate` (dir 0750, secrets 0600, token 0640 group
`ctrlmate`), per-user dir otherwise. `ServiceInfo.data_dir_kind` reports `system|user|custom`.

### Snapshot (`DaemonSnapshot`) additions in v0.6

`cooling_preset`, `cooling_policy` (per-preset `fan_curve[{temp_c,duty}]` + `pump_floor`,
`thresholds`, `requested_preset`, `effective_preset`, `state: normal|failsafe`, `reason`),
`sensor_status{entity_id: {available, reason?}}` (`not_present`, `requires_deep_hardware`,
`anti_cheat_active`, `error`, `permission`), `session` (`SessionState` or null),
`service {running_as_service, version, protocol_version, session_type, agent_connected,
in_process_session, data_dir_kind, agent_version?, agent_session_id?}`, `updates` (`UpdatesInfo`),
`include_optional_updates`, `mqtt_password_set` (the password itself is never returned), `paired_clients[].connections`.

`PUT /ipc/settings` also accepts `thermal` (`ThermalThresholds`: `cpu_critical_c` clamped to 85-98, default 93;
`gpu_critical_c` 70-92, default 88; `liquid_critical_c` 40-59, default 55; `hysteresis_c`; `pump_floor_c`). It is
persisted in `config.json` (`thermal`) and pushed to the running cooling engine, which applies it on its next tick
(no restart). `include_optional_updates` (also in the snapshot) widens OS update scans from security + quality
updates to optional updates, drivers and feature upgrades. `mqtt.password` absent keeps the stored password, `""` clears it. `scripts`
replaces the allowlist after validation (id `[a-z0-9_-]{1,32}`, unique, absolute `executable`, args
as a vector, absolute `working_dir`, at most 32 scripts); the executable's existence is checked by
the agent at execution time.

## 4. Agent protocol (`/ipc/agent`)

Daemon to agent: `auth_ok`, `scripts {scripts}` (enabled scripts; re-sent on every change),
`request {id, action}` with `action.type` in `lock | display_off | notify{title,body} |
set_volume{percent} | set_mute{muted} | set_audio_output{id} | run_script{script_id}`.

Agent to daemon: `hello {agent_version, os, session_id, user}`, `session_state {...SessionState}`
on change and at least every 5 s (state older than 15 s counts as stale), `response {id, ok, code,
message?}`.

The daemon closes an agent socket with code `4401` when the IPC token was rotated (the agent re-reads the token and
reconnects at once) and `4409` when a newer agent replaced it (the agent waits 5 minutes before trying again).
`ctrlmate-agent` finds the token in the order env `CTRLMATE_IPC_TOKEN`, per-user dir, system dir, only talks to a
loopback endpoint, and reconnects with a 1-30 s exponential backoff. `session_state` is sent when something
significant changed (idle time only counts across the 5 minute idle threshold) and at least every 5 s. Values the
agent cannot measure are `null` (never 50 % / false): for example Wayland has no foreground-window API.

No agent: `no_agent`. No answer within 10 s: `agent_timeout`. A newer agent replaces the older
connection. The in-process fallback exists only for interactive/dev daemon runs, never runs scripts
(`not_supported`), and is bypassed whenever an agent is connected.

## 4b. Cooling, anti-cheat and honest sensors (HAL behaviour)

* The cooling watchdog runs on its own 1 s timer inside `ctrlmate-hal` (not inside the sampled poll) and is the
  source of `cooling_policy` curves, `state` and `reason` (`temperature | sensor_lost | write_failed`); the daemon
  overlays `requested_preset`, `effective_preset` (`auto` -> `gaming` while the agent reports a game) and the
  configured thresholds. Pump >= 50 % on every path; a critical temperature, a sensor stale > 5 s, or all sensors
  missing > 10 s drive the failsafe (see ARCHITECTURE section 6). Failed hardware writes are retried each tick.
* Windows zero-kernel mode has no CPU temperature: there is no `cpu_0_temperature` state and `sensor_status` says
  `requires_deep_hardware`; the policy then relies on the coolant and GPU temperatures. With the bridge the
  daemon reports Tctl plus one entity per CCD (`cpu_0_ccd<n>_temperature`).
* Chassis fan entities are `motherboard_0_fan_<chip>_<n>` (from the LibreHardwareMonitor id), stable across runs.
* The manifest is rebuilt (`manifest_dirty` from the poll) when the bridge starts delivering VRM / chipset /
  fan data or the hardware topology changes. Light entities exist only for zones a backend really drives.
* Anti-cheats (FACEIT, Vanguard, EasyAntiCheat, BattlEye, Ricochet, Javelin; one table in
  `ctrlmate_hal::anticheat`) stop the bridge and every SMBus / PawnIO / OpenRGB path within 2 s; they resume after
  the anti-cheat has been gone for 30 s. A dead or silent bridge loses its snapshot and is restarted with a backoff.
* `audio_output` select options are the agent's output names; the manifest is republished when they change.
* `RgbDeviceStatus.reason` values are stable codes (`ok`, `requires_deep_hardware`, `not_present`, ...).

## 5. Adaptive sampling

Sampling interval by activity tier (gaming / active / idle >= 5 min / locked) and listeners, never
above 5 s (cooling safety). See `crates/ctrlmate-daemon/src/sampler.rs` for the table. Publishing to
HA is a separate delta filter (`PollingProfile`, documented on the type).

## 6. CLI

`--service`, `--foreground`, `--data-dir`, `--ha-port`, `--ipc-port`, `--no-in-process-session`,
`--print-telemetry`, `--rotate-ipc-token`, and on Windows (elevated) `--install-service`,
`--upgrade-service`, `--uninstall-service [--purge-data]`.
