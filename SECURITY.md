# Security policy

## Reporting a vulnerability

Please do **not** open a public issue. Use GitHub's private vulnerability reporting
(<https://github.com/Thomas-TP/Ctrlmate/security/advisories/new>). Include the version, platform, reproduction steps
and impact. You will get an acknowledgement within a few days; fixes for confirmed issues are released as soon as
practical with a credit unless you prefer otherwise.

Supported versions: the latest release only.

## Threat model (summary of docs/ARCHITECTURE.md section 2)

In scope (Ctrlmate defends against):

- **Web pages in the user's browser** reaching the local IPC: every `/ipc/*` route needs a bearer token (constant-time
  compare); requests with an `Origin` header are rejected; `Host` must be `127.0.0.1:<port>` or `localhost:<port>`
  (DNS rebinding); no CORS layer; WebSockets authenticate with a first frame within 5 s.
- **Other local users**: the Windows data directory is SYSTEM + Administrators only (only `ipc.token` is readable by
  Users); on Linux the data directory is `0750` and secrets `0600`, the token `0640` group `ctrlmate`. The Linux
  daemon runs as an unprivileged user with a hardened systemd unit and udev rules limited to exact VID:PID pairs.
- **LAN attackers**: TLS 1.3 only, trust-on-first-use certificate pinning by Home Assistant, PIN pairing with
  per-IP throttling, handshake/connection limits, revocable tokens that close live sockets immediately,
  `/api/v1/info` reveals no MAC/hostname before pairing. The firewall rule is limited to the local subnet and
  private/domain profiles.
- **Remote command abuse**: Home Assistant can only trigger enabled permission categories; the script allow-list can
  only be edited over authenticated local IPC; scripts use an absolute path, an argument vector (no shell) and a
  timeout. The agent never accepts an executable path from a request.
- **Unsafe hardware states**: the cooling safety rules are non-bypassable and treat missing data as unsafe.

Out of scope:

- Code already running as the same user: it can read the IPC token, so `/ipc/*` is not a privilege boundary against it.
- Physical access, a compromised Home Assistant instance holding a valid paired token (revoke it from the app), and a
  compromised administrator account.
- Third-party components: PawnIO and LibreHardwareMonitor in the optional deep-hardware mode, vendor drivers, OpenRGB.

## Supply chain

`Cargo.lock` is committed and CI builds with `--locked`. `cargo deny` (advisories, licenses, sources) and `cargo audit`
run on every change. Releases carry `SHA256SUMS`, GitHub build-provenance attestations
(`gh attestation verify <file> -R Thomas-TP/Ctrlmate`), cosign keyless bundles for Linux packages, and Authenticode
signatures for Windows builds when signing is configured (an unsigned build prints a warning in the release log).
