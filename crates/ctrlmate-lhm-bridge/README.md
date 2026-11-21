# ctrlmate-lhm-bridge (Optional Windows Deep Hardware Helper)

Optional Windows-only sidecar helper scheduled for **Phase 3 (Deep Hardware Mode)**.

- **Not used on Linux** (Linux uses native kernel `/sys/class/hwmon`, `i2c-dev`, `hidraw`, `nvme`).
- **Not loaded in default Zero-Kernel / Anti-Cheat Safe mode** (100% compatible with Riot Vanguard, FACEIT, EAC, Ricochet).
- When explicitly enabled by a Windows user outside strict FACEIT sessions, bridges `LibreHardwareMonitorLib` + **PawnIO** (`namazso/PawnIO`) to expose motherboard Super I/O sensors/PWM headers (ITE, Nuvoton, Fintek, ASUS EC), fine Ryzen SMU/Intel MSR CPU temperatures, and DDR4/DDR5 SMBus temperatures.
