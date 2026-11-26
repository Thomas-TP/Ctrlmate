# Ctrlmate

Open-source (AGPL-3.0-or-later) PC control suite for **Windows and Linux**: hardware telemetry, cooling policy
with a safety watchdog, RGB / Kelvin lighting, session control (audio, lock, notifications, scripts) and a native
**Home Assistant** integration.

> Status: 0.6 is a major rework (see [CHANGELOG](CHANGELOG.md)). The design contract is in
> [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## What it is

| Component | Binary | Runs as | Role |
|---|---|---|---|
| Daemon | `ctrlmated` | Windows service (SYSTEM) / systemd service (user `ctrlmate`) | Hardware access, cooling engine, lighting engine, Home Assistant transports (TLS 1.3 WebSocket, mDNS, optional MQTT), pairing, local IPC |
| Session agent | `ctrlmate-agent` | the logged-in user | Everything bound to your session: lock, screens off, volume / output device, toast notifications, foreground app and gaming detection, mic / webcam in use, allow-listed scripts |
| Desktop app | `ctrlmate-desktop` | the logged-in user | UI (Tauri v2, React 19). Talks to the daemon through Rust commands; the IPC token never reaches the web view |
| HA integration | `custom_components/ctrlmate` | Home Assistant | Entities, services, update platform, config flow with certificate pinning |
| Deep-hardware bridge (optional, Windows) | `ctrlmate-lhm-bridge` | child of the daemon | CPU temperature and SMBus access through LibreHardwareMonitor + PawnIO |

## What works, and what does not

Honest summary. A sensor that is not available is reported as unavailable with a reason, never faked.

Works out of the box (no kernel driver on Windows):

- NVIDIA GPU telemetry and fan control through NVML.
- NZXT Kraken (X3, Z3, 2023, Elite 2023/2024) liquid temperature, pump and fan telemetry and fan/pump curves over USB HID.
- NZXT RGB / Smart Device controllers and Gigabyte RGB Fusion (ITE IT5702 / IT8297 / IT5711) lighting over USB HID.
- OpenRGB SDK server (`127.0.0.1:6742`) as a lighting fallback for devices Ctrlmate does not drive itself.
- NVMe temperature / SMART, storage and system metrics.
- Session features through the agent: volume, mute, output device, lock, display off, notifications, foreground app, gaming mode, mic / webcam in use, scripts you allow-listed.
- Windows Update (WUA) and Linux package updates (PackageKit) status.
- Home Assistant control and telemetry with certificate pinning, pairing PIN and revocable tokens.

Needs the optional deep-hardware mode (Windows, installs no driver by itself; uses PawnIO):

- **CPU temperature on Windows.** Impossible without a kernel driver, so in the default mode it is reported as `requires_deep_hardware`. On Linux it comes from `hwmon`.
- SMBus DRAM RGB (G.Skill ENE) on Windows. On Linux it uses `/dev/i2c-*` directly (see Linux notes).

Known limitations:

- Samsung 990 PRO heatsink LED: only read-only detection and diagnostics. Samsung's vendor-specific NVMe admin commands are undocumented and are deliberately never sent to your boot SSD.
- NZXT hardware is exclusive: close NZXT CAM, it holds the USB device open.
- Linux GPU fan control and Windows Update install actions depend on driver / privilege; unsupported cases return `not_supported`, not success.
- Windows on ARM is not supported. Linux packages are built for x86_64 and aarch64.

## Requirements

- Windows 10 / 11 x64 with WebView2 (installed by the setup if missing), or a Linux distribution with systemd and WebKitGTK 4.1 (Debian 12+, Ubuntu 22.04+, Fedora 39+, Arch, NixOS).
- Home Assistant at least the version declared in `hacs.json` for the integration (discovery uses `zeroconf`).

## Install

### Windows

1. Download `Ctrlmate-win-Setup.exe` from the [latest release](https://github.com/Thomas-TP/Ctrlmate/releases/latest). The installer is **per-user, no administrator rights**, and updates itself with delta packages (Velopack).
2. Releases are Authenticode-signed when the maintainers' signing secrets are configured; unsigned builds make SmartScreen warn. Verify with `SHA256SUMS` and the build attestation (`gh attestation verify <file> -R Thomas-TP/Ctrlmate`).
3. For full hardware access, boot-time operation and Windows Update, install the service. The app offers a one-click elevated install (UAC), or run it yourself:
   ```powershell
   # from the install folder (%LocalAppData%\Ctrlmate\current)
   .\install-service.ps1            # install   (-Action upgrade | uninstall)
   ```
   This copies the daemon to `%ProgramFiles%\Ctrlmate`, registers the `Ctrlmate` service with recovery actions, adds a firewall rule for private/domain networks and the local subnet only (TCP 9487, UDP 5353) and locks down `%ProgramData%\Ctrlmate`.
4. `ctrlmate-agent` starts at login through `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, created by the app/agent. Remove that value to disable it.

### Linux

All Linux packages install the service, the udev rules and the `ctrlmate` system user. The daemon runs unprivileged as `ctrlmate`; your user must be in the `ctrlmate` group to read the IPC token:

```sh
sudo usermod -aG ctrlmate "$USER"   # then log out and back in
```

- **Debian / Ubuntu / Mint**: `sudo apt install ./ctrlmate_<version>_amd64.deb` (or `arm64`). The postinst adds the invoking `sudo` user to the group.
- **Fedora / RHEL / openSUSE**: `sudo dnf install ./ctrlmate-<version>-1.x86_64.rpm` (or `aarch64`).
- **Arch Linux**: build with [`packaging/linux/PKGBUILD`](packaging/linux/PKGBUILD). Run `updpkgsums` first, the committed checksum is a placeholder.
- **NixOS**: the flake at the repository root provides `ctrlmated`, `ctrlmate-agent` and a NixOS module (`services.ctrlmate.enable = true`). The desktop app is not packaged for Nix yet.
- **AppImage** (Velopack, auto-updating): bundles the app, agent and daemon. It does **not** install the service, udev rules or user. Use it for the UI, or install the deb/rpm for hardware access.

The session agent autostarts through `/etc/xdg/autostart/ctrlmate-agent.desktop`. Alternatively use the user unit and disable the autostart entry: `systemctl --user enable --now ctrlmate-agent`.

udev rules grant the `ctrlmate` group access to the **exact VID:PID pairs** listed in
[`packaging/linux/99-ctrlmate.rules`](packaging/linux/99-ctrlmate.rules) (NZXT, Gigabyte ITE) and to the motherboard SMBus
adapters (PIIX4 / I801) only. `i2c-dev` is loaded through `modules-load.d`. Nothing is opened to all users.

### Home Assistant

From Home Assistant 2026.11, HACS is part of Home Assistant itself as the **Marketplace** ([docs](https://www.home-assistant.io/integrations/marketplace/)): same catalog, same repositories, same `hacs.json`. Nothing to download.

1. Settings, **Marketplace**, **Add from link** (connect your GitHub account the first time, it only reads public data), enter `Thomas-TP/Ctrlmate`, then **Add**.
2. Open **Ctrlmate** in the Marketplace, **Install**, and restart Home Assistant if a repair asks for it.
3. Settings, Devices & services: **Ctrlmate** is discovered automatically through mDNS (or add it and enter the host). Confirm the certificate fingerprint when asked (it must match the one shown in the desktop app) and enter the 6-digit PIN shown there.

On Home Assistant 2026.10 or older, install [HACS](https://hacs.xyz) and add the repository there as a custom repository of type **Integration**.
Manual install: unzip `ctrlmate-homeassistant-<version>.zip` from the release into `config/custom_components/ctrlmate`.
A Hue / Govee sync blueprint is in [`blueprints/automation/`](blueprints/automation/ctrlmate_sync_hue_govee.yaml).

## Security model (summary)

Details and threat model: [SECURITY.md](SECURITY.md).

- The local IPC (`127.0.0.1:9488`) requires a bearer token, rejects any request with an `Origin` header and any wrong `Host` (DNS rebinding). The token never reaches the web view.
- LAN access (`:9487`) is TLS 1.3 only with a self-signed certificate that Home Assistant pins (trust on first use against the fingerprint), PIN pairing with per-IP throttling, revocable tokens.
- The cooling safety rules (pump never below 50 %, critical temperature forces 100 %, missing sensor data is treated as unsafe, firmware control restored on exit) cannot be bypassed from any path.
- Scripts run only from an allow-list edited over authenticated local IPC (never from Home Assistant), with an absolute executable path, argument vector (no shell), 60 s timeout.

## Anti-cheat FAQ

- **Does Ctrlmate load a kernel driver?** Not in the default mode. The default Windows mode uses NVML, USB HID and user-mode APIs only.
- **What about deep-hardware mode?** It uses PawnIO (not WinRing0) through LibreHardwareMonitorLib. While FACEIT, Riot Vanguard, EasyAntiCheat, BattlEye, Ricochet or Javelin is detected, the bridge and every SMBus path are stopped and not started; they resume 30 seconds after the anti-cheat is gone.
- **Is it guaranteed safe with every anti-cheat?** No. Detection is by process and service name and anti-cheat vendors can change behaviour at any time. If you play competitive games, leave deep-hardware mode off.

## FAQ

- **Why is there no CPU temperature on Windows by default?** See above: reading it needs a driver. Enable deep-hardware mode or accept the explicit "requires deep hardware" state.
- **Can Home Assistant run commands on my PC?** Only the categories you enable (power control, custom scripts, ...). Editing the script list is impossible from Home Assistant.
- **Where is my data?** Windows service: `%ProgramData%\Ctrlmate`. Linux service: `/var/lib/ctrlmate`. Nothing is sent to the internet except the update check against GitHub Releases.

## Build from source

Prerequisites: Rust (the toolchain is pinned by [`rust-toolchain.toml`](rust-toolchain.toml)), [Bun](https://bun.sh) 1.4+, Python 3.11+. Linux needs WebKitGTK 4.1, GTK 3, libsoup 3, libudev and hidapi development packages. The optional bridge needs the .NET 8 SDK.

```sh
bun install --cwd apps/desktop --frozen-lockfile
bun run --cwd apps/desktop build
cargo build --release --locked -p ctrlmate-daemon -p ctrlmate-agent -p ctrlmate-desktop
cargo test --workspace --locked

# packages
pwsh packaging/windows/pack-velopack.ps1                 # Windows, needs `dotnet tool install -g vpk`
packaging/linux/pack-linux.sh --formats deb,rpm          # Linux
```

`Cargo.lock` is committed. The workspace version in `Cargo.toml` is the single source; run
`python scripts/sync_version.py` after changing it (CI runs `--check`).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Security issues: [SECURITY.md](SECURITY.md).

## License

GNU Affero General Public License v3.0 or later. See [LICENSE](LICENSE).
