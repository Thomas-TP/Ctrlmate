# Ctrlmate

Modern, open-source (**AGPLv3**) PC telemetry, lighting, cooling & system control suite for **Windows & Linux**, built natively for **Home Assistant 2026.11+**.

## Installation

### Windows (Velopack Installer & Delta Auto-Updates)
1. Download **`Ctrlmate-win-Setup.exe`** from [GitHub Releases](https://github.com/Thomas-TP/Ctrlmate/releases/latest) and run it (one-click install, Start Menu & Desktop shortcuts, fast delta updates via **Velopack**).
2. *(Optional — recommended for NZXT Kraken write access, Windows Updates & PawnIO)* Install `ctrlmated` as a native auto-start Windows Service from an elevated terminal:
   ```powershell
   ctrlmated.exe --install-service
   ```

### Linux (All Distributions)
- **Universal AppImage (Velopack auto-updates)**: download the Linux AppImage from [GitHub Releases](https://github.com/Thomas-TP/Ctrlmate/releases/latest).
- **Debian / Ubuntu / Mint / Pop!_OS**: `sudo apt install ./ctrlmate_<ver>_amd64.deb`
- **Fedora / RHEL / openSUSE**: `sudo dnf install ./ctrlmate-<ver>-1.x86_64.rpm`
- **Arch Linux (AUR)**: see [`packaging/linux/PKGBUILD`](packaging/linux/PKGBUILD)
- **NixOS**: see [`packaging/linux/flake.nix`](packaging/linux/flake.nix)

### Home Assistant 2026.11 Integration (1-Click Marketplace)
1. Open Home Assistant → **Settings → Marketplace → Custom repositories**
2. Add repository URL: `https://github.com/Thomas-TP/Ctrlmate`
3. Category: **Integration**
4. Click **Install**, then configure **Ctrlmate** with the 6-digit PIN shown in the Ctrlmate desktop app (only asks for a single Area for the PC; all Child Devices inherit it automatically).

> A ready-to-use Philips Hue / Govee sync blueprint (Brightness + RGB + calibrated Kelvin `2000K–6500K`) is included in [`blueprints/automation/ctrlmate_sync_hue_govee.yaml`](blueprints/automation/ctrlmate_sync_hue_govee.yaml).

## Architecture

- **`crates/ctrlmate-protocol`** — Shared strongly-typed wire protocol (Parent PC + Home Assistant 2026.11 Child Devices, entities, commands, 6-digit PIN pairing, MQTT config & IPC).
- **`crates/ctrlmate-hal`** — Cross-platform Hardware & OS Abstraction Layer (**Zero-Kernel / Anti-Cheat Safe** by default: NVIDIA NVML telemetry & fan control, **NZXT Kraken 360/Z3/X3 USB HID** liquid temp + pump/fan telemetry & curves, NVMe SMART IOCTL + product model / Linux `hwmon`, USB HID controller discovery, conflict detection, Smart Adaptive Polling, WASAPI/PipeWire volume & audio output switching, Windows Update WUA / Linux PackageKit, active app & gaming mode detection, privacy mic/webcam sensors, native desktop notifications & allowlisted scripts).
- **`crates/ctrlmate-rgb`** — Perceptually calibrated Kelvin (`2000K–6500K`) + RGB lighting engine with 60 FPS smooth transitions and hardware backends:
  - **Gigabyte IT5702/IT8297 USB HID** (native Zero-Kernel)
  - **NZXT RGB Controller `0x2012` / HUE2 USB HID** (native multi-hub Zero-Kernel)
  - **G.Skill Trident Z5 / ENE SMBus DRAM (`0x70–0x77`)** (native Linux `/dev/i2c-*` + opt-in Windows PawnIO `SmbusPIIX4`/`SmbusI801` with automatic FACEIT Anti-Cheat Auto-Pause)
  - **OpenRGB SDK TCP** (`127.0.0.1:6742`) fallback for 1000+ peripherals
  - **Samsung 990 PRO Heatsink NVMe LED**: safe read-only detection & transparent diagnostics (Samsung's closed NVMe Vendor-Specific Admin opcodes are never guessed against your boot SSD).
- **`crates/ctrlmate-lhm-bridge`** — Optional Windows-only Deep Hardware micro-bridge (`LibreHardwareMonitorLib` + PawnIO) with automatic **FACEIT Anti-Cheat Auto-Pause**.
- **`crates/ctrlmate-cooling`** — Preset-based cooling engine with non-bypassable Thermal Safety Watchdog (AIO pump hard floor `≥ 50%`, automatic `100%` thermal emergency override).
- **`crates/ctrlmate-daemon` (`ctrlmated`)** — Lightweight Rust background daemon + native Windows Service (`--service`, `--install-service`, `--uninstall-service`), TLS 1.3 TOFU WebSocket Local Push, mDNS/Zeroconf discovery, 6-digit PIN pairing, secondary **Home Assistant 2026.11 MQTT Device + Child Device Discovery**, and local IPC.
- **`apps/desktop`** — Minimalist desktop app built with **Tauri v2 + React 19 + TypeScript + Tailwind CSS v4 + Velopack** (auto-updates from GitHub Releases).
- **`custom_components/ctrlmate`** — Native **Home Assistant 2026.11 Marketplace** integration (`local_push`, Zeroconf, `ChildDeviceInfo`, Wake-on-LAN).

## License

Licensed under the **GNU Affero General Public License v3.0 or later (AGPL-3.0-or-later)**. See [`LICENSE`](LICENSE).
