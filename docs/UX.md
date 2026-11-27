# Ctrlmate UX (v0.7)

The interface was redesigned from scratch in 0.7. This page records the decisions so they stay coherent.

## Principles

1. **One home, no maze.** Everything you need day to day is visible on one dashboard. Details open in a
   side panel on top of it (never a different page), so you never lose your place.
2. **Quiet by default.** Sober light/dark themes, one accent colour, generous space, short animations
   (disabled by `prefers-reduced-motion`). No neon, no "gaming" look.
3. **Honest.** An unavailable sensor says so and why. Cooling changes are confirmed by the service, never
   assumed. Dangerous actions (shutdown, restart, sleep, hibernate, revoking a client) ask first.
4. **The keyboard is first class.** `Ctrl+K` opens a command palette that reaches every section and the
   common actions (presets, lights, lock, rescan, pairing, theme, language).
5. **Ctrlmate never closes another program.** When NZXT CAM, Gigabyte Control Center or an anti-cheat gets in
   the way, it explains and lets the user decide.

## Surfaces

| Surface | Window | Purpose |
| --- | --- | --- |
| Dashboard | `main` | State of the PC at a glance: headline, alerts, cooling, temperatures, lighting, PC actions, Home Assistant, updates. Every tile has its main control inline. |
| Detail panels | `main` (side sheet) | Cooling, temperatures, lighting, PC, Home Assistant, updates, settings. One at a time, `Esc` closes. |
| Command palette | `main` | `Ctrl+K`. |
| Onboarding | `main` | First run (and replayable from Settings > General). |
| Quick panel | `tray` | Opened by the notification-area icon: temperatures, preset, master light, lock / screens off / sleep, "Open Ctrlmate". Closes when it loses focus. |

The two windows are the same React bundle: the `tray` window (or `?view=tray`) renders the quick panel.
The panel asks the main window to open a section with the `ui://open` event.

## Onboarding

1. **Installation**: what will be installed (service, agent, local network access), then one button.
   Windows asks for administrator approval **once**; that single elevated step installs the service,
   registers the agent autostart and the firewall rule, then the app connects by itself. If the service is
   already running this step is simply "ready". On Linux the package owns the service and the step shows the
   command.
2. **Hardware**: what was found, and which running software may hold the devices (with a rescan).
   Offers the optional deep-hardware mode only when a sensor needs it.
3. **Cooling**: pick a preset, then a thermal safety check computed from real data (sensors read, limits
   loaded, pump floor, watchdog).
4. **Home Assistant**: Marketplace instructions, the repository link to copy, a one-time PIN, and live
   detection of the pairing. Skippable.
5. **Preferences**: language, theme, unit, start with the system, close to tray.

"Set up later" leaves the onboarding; the dashboard then explains a missing service and offers the same
one-click repair.

## Installation model

`Ctrlmate-win-Setup.exe` is a per-user Velopack installer (no administrator needed to copy the app). The
service must live in `%ProgramFiles%` with restricted ACLs, so it needs one elevation: it is requested by the
first-run onboarding, with context, instead of a blind prompt in the middle of the installer. Updates keep
the same flow ("Upgrade service" appears when the installed service is older than the app).

## Design tokens

Defined in `apps/desktop/src/index.css` (`:root` and `:root[data-theme='dark']`), checked against WCAG AA by
`src/test/contrast.test.ts`. Accent `#4f46e5` (light) / `#908bff` (dark). Inter for text, JetBrains Mono only
for codes (pairing PIN, fingerprints, paths).
