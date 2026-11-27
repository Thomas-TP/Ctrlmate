# Changelog

All notable changes are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and the project uses [Semantic Versioning](https://semver.org/).

## [0.6.3] - 2026-11-27

### Fixed
- Desktop app: with no service installed yet (so no IPC token), the "Service and agent" block showed an
  error ("no access token found") instead of the install button. A missing token is now treated as
  "service not installed", so the one-click elevated install is offered.

## [0.6.2] - 2026-11-27

Patch release found by running 0.6.1 on real hardware.

### Fixed
- Cooling: a daemon without permission to control the GPU fans (NVML "no permission" or "not supported":
  non-administrator or non-root run, GPUs without fan control) no longer latches the failsafe. The first
  watchdog tick used to "hand the fans back to the firmware" although they were never taken, the write
  failed three times and the engine entered the failsafe with reason `write_failed` for good, with an
  error logged every second. Fans are now written only when the engine really forces them (failsafe), an
  "unavailable" answer is recorded once and never counts as a write failure, and shutdown only restores
  fans that were forced.
- The daemon writes its logs to stderr, so `ctrlmated --print-telemetry` prints pure JSON on stdout.

## [0.6.1] - 2026-11-26

Maintenance release: dependencies, build and release fixes on top of 0.6.0.

### Fixed
- Release: the Windows packaging step now expects Velopack's real package name (`Ctrlmate-<version>-full.nupkg`),
  so the installer, the portable zip and the 0.5.0 to 0.6.x delta are published.
- Linux and Nix builds, the deb/rpm packages (systemd unit paths) and the Home Assistant CI job.
- The version script also keeps the LibreHardwareMonitor bridge version in sync.

### Changed
- Dependencies brought up to date, replacing the Dependabot pull requests:
  - Rust: `rand` 0.10 (CSPRNG unchanged: PIN and IPC token still come from `ThreadRng`), `windows` 0.62,
    `nvml-wrapper` 0.13, `mdns-sd` 0.22, `tokio-tungstenite` 0.30, `rcgen` 0.14, `sha2` 0.11,
    `libloading` 0.9, `winreg` 0.56 and the lockfile.
  - Front end: TypeScript 7 (with the TypeScript 6 compatibility package for `typescript-eslint`), Vite 8,
    Vitest 5, ESLint 10, `eslint-plugin-react-hooks` 7 (state derived while rendering instead of in effects),
    `lucide-react` 1.x, `jest-dom` 8, `jsdom` 31.
  - The LibreHardwareMonitor bridge targets .NET 10 LTS (.NET 8 is out of support); the Velopack runtime
    dependency is now `net10-x64-desktop`.
  - GitHub Actions: `checkout` 7, `upload-artifact` 7, `download-artifact` 8, `cache` 6, `setup-python` 7,
    `setup-dotnet` 6, `attest-build-provenance` 4, `cosign-installer` 4, `azure/login` 3, `install-nix-action` 32.
- Dependabot: Python requirements are read from the repository root, minor/patch updates are grouped per
  ecosystem, and `System.*` NuGet majors are held to the target framework.
- Not updated yet: `sysinfo` (0.40 changes the disk and constructor APIs) and `zbus` (6; the Linux PackageKit
  code and `tauri-plugin-single-instance` still use 5).

## [0.6.0] - 2026-11-25

Major rework: security, correctness, honesty of sensors, UI and release engineering. Contract:
`docs/ARCHITECTURE.md`.

### Added
- `ctrlmate-agent`: headless per-session agent for lock, screens off, notifications, audio, foreground app and gaming
  detection, mic/webcam sensors and allow-listed script execution; autostarted at login (Windows `HKCU\...\Run`,
  Linux XDG autostart or `systemd --user` unit). The daemon never runs session actions itself.
- Hardened local IPC: bearer token, `Origin` rejection, `Host` check, authenticated WebSocket frames; per-IP PIN
  throttling and TLS 1.3 only on the LAN side.
- Cooling policy owned by the daemon with a non-bypassable safety watchdog and stable result codes.
- Honest sensors: unavailable values are `null` with a reason (for example `requires_deep_hardware`).
- Home Assistant: certificate pinning from the peer certificate, reauth/reconfigure flows, translated entities,
  diagnostics, update platform without fake versions, hassfest and HACS validation in CI.
- Windows service layout: install to `%ProgramFiles%`, recovery actions, firewall rule, ACLs; elevated install/upgrade from
  the app or `install-service.ps1`.
- Linux: unprivileged `ctrlmate` user, hardened `ctrlmated.service`, udev rules per exact VID:PID, SMBus-only I2C rule,
  `modules-load.d` for `i2c-dev`, user unit and XDG autostart for the agent.
- Packaging: deb and rpm with complete maintainer scripts, reproducible builds, aarch64 packages, Velopack AppImage,
  PKGBUILD with install hooks, root `flake.nix` with a NixOS module.
- Release engineering: single version source (`scripts/sync_version.py --check`), tracked `Cargo.lock`, pinned
  toolchain, `cargo deny` and `cargo audit`, CI on Windows and Linux, tag-driven release with Velopack deltas,
  Authenticode signing hook, `SHA256SUMS`, build-provenance attestation, cosign bundles, Home Assistant zip asset.

### Changed
- Redesigned desktop UI (Tauri v2, React 19, Tailwind v4): light/dark, FR + EN, offline fonts, accessible.
- Preset `full_speed` removed from user presets (it is the emergency state only); `gaming` preset added.
- `tauri.conf.json` no longer carries its own version; Tauri uses the Cargo workspace version.
- The Home Assistant install instructions now describe the Marketplace built into Home Assistant 2026.11
  (the successor of HACS: same catalog and `hacs.json`), with HACS kept for 2026.10 and older.

### Removed
- Blanket vendor-wide udev rules (ASUS, Corsair, Lian Li, MSI, Aquacomputer, `uaccess` tags) for devices Ctrlmate
  does not drive.
- Tracked `apps/desktop/tsconfig.tsbuildinfo`.

### Fixed
- Stale versions (0.4.0 default in the Linux packager, 0.5.0 in several files).
- `ctrlmated.service` ran with `CAP_NET_BIND_SERVICE` it never needed and without a service user.

[0.6.0]: https://github.com/Thomas-TP/Ctrlmate/releases/tag/v0.6.0
