# Changelog

All notable changes are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and the project uses [Semantic Versioning](https://semver.org/).

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
