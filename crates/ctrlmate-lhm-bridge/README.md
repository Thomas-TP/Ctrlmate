# ctrlmate-lhm-bridge (optional Windows deep-hardware helper)

Windows-only sidecar started by `ctrlmated` **only in deep-hardware mode**. It wraps
`LibreHardwareMonitorLib` to expose what needs a driver: AMD `Tctl` and per-CCD temperatures, Super I/O
fan tachometers / PWM headers, VRM/chipset temperatures and DIMM temperatures.

- Not used on Linux (native `hwmon`, `i2c-dev`, `hidraw`).
- Never started in the default zero-kernel mode and never while an anti-cheat is running
  (architecture section 7).

## Driver: PawnIO, not WinRing0

The project pins `LibreHardwareMonitorLib` **0.9.6**. 0.9.5 (2026-01-07, "Swap WinRing0 to PawnIO") is the first
release that uses [PawnIO](https://pawnio.eu) instead of WinRing0; 0.9.6 (2026-02-14) updates the PawnIO modules to 2.2.
0.9.4, pinned previously, still used WinRing0. Do not downgrade. After `Computer.Open()` the bridge also refuses
to continue if a `winring0` module is loaded in its process.
PawnIO itself must be installed by the user; the bridge never installs a driver.

## Command line

| flag | effect |
|---|---|
| `--no-driver` | refuse to load anything: prints `{"type":"refused","reason":"no_driver_flag"}`, exit code 3 |
| `--chassis-id <sensor identifier>` | declare a control (LHM identifier, e.g. `/lpc/it8688e/0/control/1`) as a chassis fan; repeatable |

At start the bridge also checks the architecture section 7 anti-cheat list (processes `faceitclient`, `faceit`, `vgc`, `vgtray`,
`EasyAntiCheat*`, `BEService`, `BEDaisy`, `ricochet`, `javelin`, and services `vgc`, `vgk`, `FACEIT`, `EasyAntiCheat`,
`EasyAntiCheat_EOS`, `BEService`, `BEDaisy`). If any is present it prints `refused` / `anti_cheat_active` and exits with code 3 without
opening the driver. It re-checks every 2 s while running: on detection it emits `anti_cheat`, restores PWM, closes the driver and exits with code 4.

Exit codes: `0` clean, `3` refused, `4` anti-cheat appeared, `5` initialisation/poll error.

## Line protocol

One JSON object per line, UTF-8, every object has a `"type"`.

stdout (bridge -> daemon):

| type | when | fields |
|---|---|---|
| `hello` | once | `version`, `lhm_version`, `pid`, `driver:"pawnio"` |
| `heartbeat` | every second, from its own thread | `seq`, `ts_ms`, `poll_age_ms`, `polls` |
| `snapshot` | after each poll (~1 Hz) | see below |
| `ack` | per command | `cmd`, `ok`, `detail?` |
| `error` | malformed / unusable input | `error`, `detail?` |
| `anti_cheat` | anti-cheat appeared | `processes[]` |
| `stopped` | last line | `pwm_reset`, `verified` |

**Watchdog contract for the Rust side:** no line at all for more than ~3 s means the process is dead or blocked on stdout;
heartbeats continuing with `poll_age_ms` above ~5000 means the hardware poll is hung. In both cases the daemon should treat
the bridge sensors as lost (architecture section 6 safety rules) and kill/restart it.

`snapshot`: `cpu_temp_c` (Tctl, else Package, else first CPU temperature), `cpu_tctl_c`, `cpu_package_c`,
`cpu_ccd_temps_c[{name,value}]` (one per CCD), `cpu_power_w`, `motherboard_name`, `motherboard_vrm_temp_c`,
`motherboard_chipset_temp_c`, `ram_temp_c`, `case_fans[{id,name,rpm}]`, `controls[{id,name,kind,value_pct,mode}]`.
Missing sensors are `null`, never a fabricated default.

- Fan ids are the LibreHardwareMonitor sensor identifier (`/lpc/<chip>/0/fan/<n>`), stable across runs and
  independent of RPM: stopped fans (0 rpm) are still listed.
- `controls[].kind` is `cpu_fan`, `pump`, `chassis` or `unknown`; `mode` is `default` (firmware) or `software`.

stdin (daemon -> bridge), one JSON object per line; a malformed line produces an `error` object and never stops the loop:

| command | effect |
|---|---|
| `{"cmd":"set_all_pwm","percent":N}` | software duty for **chassis** controls only, clamped to **25..100 %** (documented floor) |
| `{"cmd":"set_pwm","id":"<control id>","percent":N}` | same for one chassis control; refused (`refused_not_chassis`) for anything else |
| `{"cmd":"reset_pwm"}` | back to firmware control, verified by reading the mode back |
| `{"cmd":"quit"}` / `{"cmd":"stop"}` | graceful stop (below) |

**CPU_FAN / CPU_OPT and AIO pump headers are never driven.** They are recognised by name (`cpu`, `pump`, `aio`);
the AIO pump and CPU fan stay under firmware / the Kraken's own control, in line with the pump >= 50 % rule. Generic
names such as `Fan #1` are classified `unknown` and are *not* driven either (on ITE boards `#1` is often the CPU header);
confirm the chassis headers of your board once with `--chassis-id`.

## Stop sequence

On `quit`/`stop`, stdin EOF (daemon gone) or anti-cheat detection the bridge, in this order:
1. sets every motherboard control back to automatic (`SetDefault`) and **verifies** by reading `ControlMode` back (one retry),
2. emits `{"type":"stopped","pwm_reset":true,"verified":<bool>}`,
3. closes the LibreHardwareMonitor `Computer` (releases PawnIO),
4. returns from `Main` (no kill race: the daemon should wait for the process to exit, and only then force-kill after a timeout).

## Build

```
dotnet build -c Release crates/ctrlmate-lhm-bridge
```

Requires the .NET 8 SDK (checked with 8.0.428). The output `ctrlmate-lhm-bridge.exe` needs the .NET 8 desktop runtime.
