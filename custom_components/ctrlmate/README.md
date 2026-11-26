# Ctrlmate for Home Assistant

Local push integration for a PC running the Ctrlmate desktop app / service.
Requires Home Assistant 2026.10 or newer (child devices). No cloud, no polling.

## Install

- Marketplace (Home Assistant 2026.11+, it replaces HACS): *Settings > Marketplace > Add from link*, enter
  `Thomas-TP/Ctrlmate`, then install and restart if asked. On 2026.10 or older use HACS with a custom
  repository of type *Integration*.
- Manual: copy `custom_components/ctrlmate` into your `config/custom_components/`.
- Blueprints are **not** installed by the Marketplace or HACS. Copy `blueprints/automation/ctrlmate_sync_hue_govee.yaml`
  to `config/blueprints/automation/ctrlmate/` (or import its raw URL under
  *Settings > Automations & Scenes > Blueprints*).

## Set up

1. Start pairing in the Ctrlmate desktop app (Home Assistant page): a 6-digit PIN and the
   certificate fingerprint are shown.
2. In Home Assistant the PC is discovered automatically (or add it manually with host and port, default 9487).
3. When discovery does not provide the fingerprint, Home Assistant shows the certificate fingerprint it
   received. **Compare it with the one in the desktop app** and confirm only if they are identical.
4. Enter the PIN. From then on every connection is pinned to that certificate (fail closed).

If the token is revoked or the certificate changes, Home Assistant opens a re-authentication flow.
The address can be changed under *Reconfigure*.

## What you get

- A PC device with child devices (CPU, GPU, memory, drives, cooler, motherboard) that inherit the PC's area.
- Telemetry sensors; a sensor the hardware or mode cannot provide is *unavailable*, never 0.
- Lights for the master zone and every real zone (brightness, RGB, Kelvin 2000-6500), cooling mode select
  (auto, silent, balanced, performance, gaming), audio volume / mute / output, power and script buttons,
  desktop notifications.
- OS updates: an `update` entity (KB ids and titles in attributes), a pending-count sensor and a
  reboot-required binary sensor.
- **Wake PC** button (Wake-on-LAN) that works while the PC is off. Options: broadcast address (default: the
  subnet broadcast of the PC) and UDP port.
- Actions that need the user session (lock, displays, volume, notifications, scripts) report a translated
  error and a repair issue when the Ctrlmate agent is not running.

## Troubleshooting

Download diagnostics from the device page (token, fingerprint, MAC, host and addresses are redacted).
