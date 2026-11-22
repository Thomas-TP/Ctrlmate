# Ctrlmate

Modern, open-source (**AGPLv3**) PC telemetry, lighting, cooling & system control suite for **Windows & Linux**, built natively for **Home Assistant 2026.11+**.

## Home Assistant 2026.11 Installation (1-Click Marketplace)

1. Open Home Assistant → **Settings → Marketplace → Custom repositories**
2. Add repository URL: `https://github.com/Thomas-TP/Ctrlmate`
3. Category: **Integration**
4. Click **Install**, then configure **Ctrlmate** with the 6-digit PIN shown in the Ctrlmate desktop app.

> A ready-to-use Philips Hue / Govee sync blueprint (Brightness + RGB + calibrated Kelvin `2000K–6500K`) is included in [`blueprints/automation/ctrlmate_sync_hue_govee.yaml`](blueprints/automation/ctrlmate_sync_hue_govee.yaml).

## Architecture

- **`crates/ctrlmate-protocol`** — Shared strongly-typed wire protocol (Parent PC + Home Assistant 2026.11 Child Devices, entities, commands, 6-digit PIN pairing & IPC).
- **`crates/ctrlmate-hal`** — Cross-platform Hardware & OS Abstraction Layer (**Zero-Kernel / Anti-Cheat Safe** by default: NVIDIA NVML telemetry & fan control, **NZXT Kraken 360/Z3/X3 USB HID** liquid temp + pump/fan telemetry & curves, NVMe SMART IOCTL / Linux `hwmon`, USB HID controller discovery, conflict detection, Smart Adaptive Polling, WASAPI/PipeWire volume & audio output switching, Windows Update WUA / Linux PackageKit, active app & gaming mode detection, privacy mic/webcam sensors, native desktop notifications & allowlisted scripts).
- **`crates/ctrlmate-rgb`** — Perceptually calibrated Kelvin (`2000K–6500K`) + RGB lighting engine with 60 FPS smooth transitions and native hardware backends (**Gigabyte IT5702/IT8297 USB HID**, **NZXT RGB Controller `0x2012` / HUE2 USB HID**, **OpenRGB SDK TCP** for G.Skill DDR5 & 1000+ devices) for Philips Hue & Govee synchronization.
- **`crates/ctrlmate-lhm-bridge`** — Optional Windows-only Deep Hardware micro-bridge (`LibreHardwareMonitorLib` + PawnIO) with automatic **FACEIT Anti-Cheat Auto-Pause** (never used on Linux or in default Zero-Kernel Safe mode).
- **`crates/ctrlmate-cooling`** — Preset-based cooling engine with non-bypassable Thermal Safety Watchdog.
- **`crates/ctrlmate-daemon` (`ctrlmated`)** — Lightweight Rust background daemon (TLS 1.3 TOFU WebSocket Local Push, mDNS/Zeroconf discovery, 6-digit PIN pairing, local IPC).
- **`apps/desktop`** — Minimalist desktop app built with **Tauri v2 + React 19 + TypeScript + Tailwind CSS v4**.
- **`custom_components/ctrlmate`** — Native **Home Assistant 2026.11 Marketplace** integration (`local_push`, Zeroconf, Child Devices, Wake-on-LAN).

## License

Licensed under the **GNU Affero General Public License v3.0 or later (AGPL-3.0-or-later)**. See [`LICENSE`](LICENSE).
