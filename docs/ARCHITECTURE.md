# Ctrlmate architecture contract (v0.6)

This document is the single source of truth for the v0.6 rework. Every component must follow it.
If you must deviate, change this file in the same change and say why.

## 1. Processes

| Process | Runs as | Session | Role |
|---|---|---|---|
| `ctrlmated` | SYSTEM (Windows service) / root or dedicated user (systemd) / current user (dev, `--foreground`) | 0 / system | Hardware access (NVML, HID, SMBus, OpenRGB), cooling engine + safety watchdog, lighting engine, Home Assistant transports (TLS WebSocket, mDNS, MQTT), pairing, IPC server. |
| `ctrlmate-agent` | the logged-in user | user session | Headless. Everything that is bound to the interactive session: lock, screens off, toast notifications, volume/mute/audio output, foreground app + gaming detection, mic/webcam in use, **custom script execution**. Autostarted at login (Windows `HKCU\...\Run`, Linux XDG autostart). |
| `Ctrlmate` desktop (Tauri v2) | the logged-in user | user session | UI only. Talks to the daemon **through Rust commands**; the IPC token never reaches the webview. |

The daemon never executes anything that must happen in the user's session itself. It forwards to the agent.
If the daemon itself runs in an interactive session (dev mode / `--foreground`), it MAY run the session
features in-process through the same `ctrlmate_hal::session` module the agent uses.

## 2. Local IPC security (loopback `127.0.0.1:9488`)

- Every `/ipc/*` route requires `Authorization: Bearer <token>`; compared in constant time.
- The WebSocket endpoints (`/ipc/ws`, `/ipc/agent`) authenticate with a first text frame
  `{"type":"auth","token":"..."}` within 5 s, otherwise the connection is closed.
- Requests carrying an `Origin` header are rejected (403). No CORS layer at all.
- `Host` must be `127.0.0.1:<port>` or `localhost:<port>` (anti DNS-rebinding), otherwise 421.
- Token: 32 random bytes, hex, generated on first start, rotatable (`ctrlmated --rotate-ipc-token`).
  Stored in `ipc.token` in the daemon data dir:
  - Windows service: `%ProgramData%\Ctrlmate\ipc.token`, ACL: SYSTEM + Administrators full, `Users` read.
    The rest of the data dir (TLS key, config, token hashes, MQTT password) is SYSTEM + Administrators only.
  - Linux service: `/var/lib/ctrlmate/ipc.token`, mode `0640`, group `ctrlmate`. Data dir `0750`; secrets `0600`.
  - Dev / per-user daemon: per-user config dir, mode `0600` / user-only ACL.
- Client lookup order (desktop + agent): env `CTRLMATE_IPC_TOKEN`, per-user dir, system dir.
- The `/ipc/*` surface is **not** a privilege boundary against code running as the same user (they can read the
  token). It IS a boundary against web pages, other users, and anything without the token.
- `from_remote=false` (IPC) commands still pass the `power_control` / `custom_scripts` / etc. toggles for
  *execution*; the toggles gate what Home Assistant may do, and `custom_scripts` additionally gates execution
  for everyone. Editing the script allowlist is only possible over authenticated IPC and never over HA.
- LAN side (`:9487`, TLS): TLS 1.3 only, per-IP PIN attempt throttling (not a global counter), connection and
  handshake limits, `client_name` length-limited, revoked tokens close their live sockets immediately,
  `connected_clients` keyed by connection id. `/api/v1/info` must not leak the MAC / hostname unless paired.
- `permissions.telemetry = false` means: no state frames to that client / no MQTT state publishing.

## 3. Agent protocol (`/ipc/agent`, JSON text frames)

Agent → daemon:
- `{"type":"hello","agent_version":"…","os":"windows|linux","session_id":N,"user":"…"}`
- `{"type":"session_state", ...SessionState}` on change and at least every 5 s (see protocol crate `SessionState`):
  `foreground_app{process_name,window_title?}`, `gaming_mode`, `locked`, `idle_seconds`, `mic_in_use`,
  `webcam_in_use`, `audio{volume,muted,output_id,outputs[{id,name}]}`.
- `{"type":"response","id":"…","ok":bool,"code":"…","message":"…?"}`

Daemon → agent:
- `{"type":"request","id":"…","action":{…SessionAction}}` with `SessionAction` =
  `lock | display_off | notify{title,body} | set_volume{percent} | set_mute{muted} | set_audio_output{id} |
  run_script{script_id}` (the agent looks the script up from a daemon-provided allowlist snapshot sent in
  `{"type":"scripts","scripts":[…]}`; it never accepts an executable path from a request).
- Requests time out after 10 s (`code:"agent_timeout"`). With no agent connected the daemon answers
  `code:"no_agent"` — never a fake success.
- Scripts: absolute executable path required, args as a vector (no shell), 60 s timeout, output captured
  (truncated to 8 KiB) and returned in `message`, child waited (no zombies).

## 4. Result codes, no human strings from Rust

`CommandResult { ok: bool, code: String, message: Option<String> }`.
Codes (stable, lower snake): `ok`, `permission_denied`, `no_agent`, `agent_timeout`, `not_supported`,
`invalid_argument`, `hardware_error`, `busy`, `unavailable`, `internal`.
The UI and the HA integration translate codes. Rust never sends French or English sentences to a UI.
(HA entity *names* are the exception: English defaults provided by the manifest, translated in HA through
`translation_key`.)

## 5. Honest sensors

Telemetry values are `Option`. A missing sensor is `null` with a `SensorStatus { available: bool,
reason: "not_present" | "requires_deep_hardware" | "anti_cheat_active" | "error" | "permission" }`
entry in the snapshot. Never fabricate defaults (no "50% unmuted Default Output", no `latest_version`
strings that look real). Windows zero-kernel mode has **no CPU temperature** (impossible without a driver);
that is reported as `requires_deep_hardware`, not hidden.

## 6. Cooling policy (daemon is the only source of truth)

Presets: `auto`, `silent`, `balanced`, `performance`, `gaming`. (`full_speed` is NOT a user preset any more;
it is the emergency state only.) `gaming` is applied automatically when `auto` is selected and gaming mode
is detected by the agent.
The snapshot carries `cooling_policy`: per-preset fan curve points `[{temp_c,duty}]` and `pump_floor`, the
watchdog thresholds (`cpu_critical_c`, `gpu_critical_c`, `liquid_critical_c`, `hysteresis_c`,
`pump_floor_c`), the current effective state (`normal|failsafe`) and the reason
(`temperature|sensor_lost|write_failed`). The UI renders exactly this, nothing hardcoded.
Safety rules (non-bypassable, all paths):
1. Pump never below 50 %.
2. A critical temperature on any known sensor ⇒ 100 % fans + pump, until all known sensors are ≥ hysteresis below.
3. Missing data is unsafe, not safe: a sensor that was present and is stale > 5 s, or *all* of CPU/GPU/liquid
   missing for > 10 s, forces the failsafe floor (fans ≥ 70 %, pump 100 %). A sensor lost while in
   failsafe stays in failsafe.
4. A failed write is retried on the next tick (target not marked as applied) and logged at `error`, and the
   fan write is attempted even if the pump write failed (and vice versa).
5. Defaults must fit a Ryzen 9 7900X3D (Tctl normally up to ~89 °C): `cpu_critical_c` default 93, configurable
   85–98; GPU 88; liquid 55 (Kraken).
6. On clean shutdown restore firmware control (Kraken default curve, GPU auto fan, motherboard PWM auto).
   A curve written to the Kraken must itself end in a firmware-side 100 % point so a crash is safe.

## 7. Anti-cheat

Detect (process / service names): FACEIT (`faceitclient`, `faceit`), Riot Vanguard (`vgc`, `vgtray`, service
`vgc`/`vgk`), EasyAntiCheat (`EasyAntiCheat*`), BattlEye (`BEService`, `BEDaisy`), Ricochet, Javelin,
Easy Anti-Cheat EOS. While any is running: the deep-hardware bridge and every SMBus/PawnIO path are stopped
**and not started**, and resume automatically after the anti-cheat has been gone for 30 s. Default mode loads
no kernel driver. The LHM bridge must use a LibreHardwareMonitorLib build that uses PawnIO, not WinRing0.

## 8. Windows service layout

- `ctrlmated.exe --install-service` (elevated) copies the daemon to `%ProgramFiles%\Ctrlmate\` (admin-write
  only), registers the service pointing there, sets recovery actions, creates the firewall rule
  (private+domain profiles, `remoteip=localsubnet`, TCP 9487 + UDP 5353 inbound), creates ACLs of §2.
  `--upgrade-service` stops, replaces binary + bridge, restarts. `--uninstall-service` reverses it.
- The desktop app detects "service missing / older than app" and offers a one-click **elevated** install /
  upgrade (UAC). The Velopack installer itself stays per-user, no admin.
- The service reports `StartPending`/`Running`/`StopPending` correctly.

## 9. Linux service layout

`ctrlmated.service` runs as root only if needed for I²C/hidraw; otherwise as user `ctrlmate`
(`StateDirectory=ctrlmate`, `ConfigurationDirectory=ctrlmate`, `RuntimeDirectory=ctrlmate`). Session features
go through the agent (XDG autostart + a `systemd --user` unit). udev rules scoped per exact VID:PID, no blanket
vendor access.

## 10. Home Assistant integration

- TOFU pinning: the config flow opens a raw TLS connection, reads the **peer certificate**, computes SHA-256,
  compares with the full 64-hex `fp` from mDNS when present, otherwise asks the user to confirm the
  fingerprint shown in the desktop app. Pairing and every later connection use `aiohttp.Fingerprint`.
- Errors: command `ok=false` ⇒ `HomeAssistantError` with translation key per `code`; reauth flow on
  `auth_error` / revoked token (`ConfigEntryAuthFailed`); reconfigure flow; `AbortFlow` never swallowed.
- Entity names via `translation_key` + `strings.json`/`translations/{en,fr}.json`.
- Update platform: no fake version; pending count is its own sensor; titles + KB ids in attributes;
  `release_summary` ≤ 255 chars; `UpdateEntityFeature.PROGRESS`.
- `hacs.json` / `manifest.json` metadata correct for the Marketplace, `diagnostics.py`, `icons.json`,
  `quality_scale`-style hygiene, hassfest + HACS validation in CI.

## 11. UI

Tauri v2 + React 19 + TypeScript + Tailwind v4. Clean, minimal, calm (Linear / Raycast / Apple), light + dark
following the system with a manual override persisted, no neon, no "gaming" look. Fonts bundled offline.
FR + EN complete with a language switch (default: system language). °C/°F everywhere through one formatter.
WCAG AA contrast, keyboard navigable, `aria-live` for toasts, text alternatives for charts. All daemon data is
real; empty/unavailable states are explicit.

## 12. Versioning and quality gates

- One version: `[workspace.package].version`. Everything else is derived or generated by `scripts/sync-version`.
- `Cargo.lock` is committed. CI: `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`,
  `cargo test --workspace`, `cargo audit`, front `tsc` + `eslint` + `vitest` + `vite build`, hassfest + HACS
  validation, packaging smoke builds.

## 13. Release engineering

- Version: `[workspace.package].version` only. `scripts/sync_version.py` derives `package.json` (root and desktop),
  the HA `manifest.json` and the PKGBUILD `pkgver`; `tauri.conf.json` has no version (Tauri uses Cargo's);
  `flake.nix` reads `Cargo.toml`; the packagers use `scripts/sync_version.py --print`. CI runs `--check`.
- Binaries expected by packaging in `target/release`: `ctrlmated`, `ctrlmate-agent`, `ctrlmate-desktop`
  (`.exe` on Windows) plus the optional `ctrlmate-lhm-bridge.exe` (dotnet publish) placed next to `ctrlmated.exe`.
- Linux: service `ctrlmated.service` runs `ctrlmated --data-dir /var/lib/ctrlmate` as `ctrlmate:ctrlmate`; udev rules
  are per exact VID:PID and are checked against the Rust tables by `scripts/check_udev_rules.py`.
- Windows: Velopack, per-user, channel `win`; the service step is `ctrlmated --install-service | --upgrade-service |
  --uninstall-service` (service name `Ctrlmate`), wrapped by `packaging/windows/install-service.ps1`.
- Velopack channels: `win`, `linux` (x86_64), `linux-arm64`.
- Release: tag `vX.Y.Z` == workspace version, gated by CI, artifacts signed (Authenticode if configured, cosign
  keyless, optional GPG), `SHA256SUMS`, build-provenance attestation, HA integration zip.
