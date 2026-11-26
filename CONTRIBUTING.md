# Contributing to Ctrlmate

Thanks for helping. The binding design contract is [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): read it first, and
change it in the same pull request if you must deviate.

## Setup

- Rust: the toolchain is pinned in `rust-toolchain.toml` (rustup installs it automatically).
- Front end: Bun 1.4+ (`bun install --cwd apps/desktop --frozen-lockfile`).
- Python 3.11+ for the helper scripts and the Home Assistant tests (`pip install -r tests/ha/requirements_test.txt`).
- Linux build dependencies: `libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev libayatana-appindicator3-dev librsvg2-dev libudev-dev libhidapi-dev libdbus-1-dev pkg-config`.
- Optional Windows bridge: .NET 10 SDK.

## Quality gates (CI runs all of them)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny --locked check && cargo audit

cd apps/desktop && bun run typecheck && bun run lint && bun run test && bun run build
ruff check custom_components tests/ha && pytest tests/ha
python scripts/sync_version.py --check
python scripts/check_udev_rules.py
```

## Rules of the road

- Rust never sends human-readable strings to a UI: use the stable result codes (ARCHITECTURE section 4).
- Telemetry is `Option`; a missing sensor is `null` with a `SensorStatus` reason. Never fabricate a default.
- Do not weaken the cooling safety rules or the IPC security rules (ARCHITECTURE sections 2 and 6).
- No kernel driver in the default Windows mode. Anything touching SMBus/PawnIO must respect anti-cheat auto-pause.
- Adding a supported USB device: add its VID:PID to the Rust table **and** to `packaging/linux/99-ctrlmate.rules`
  (exact `idVendor` + `idProduct`, group `ctrlmate`). `scripts/check_udev_rules.py` fails otherwise.
- UI strings go through the FR + EN dictionaries, temperatures through the single formatter.

## Versioning and releases

The version lives only in `[workspace.package].version` in `Cargo.toml`. After bumping it run
`python scripts/sync_version.py` (updates `package.json` files, the HA `manifest.json` and the PKGBUILD) and add a
`## [x.y.z] - YYYY-MM-DD` section to `CHANGELOG.md`. Then run `updpkgsums` in `packaging/linux` after the tag
archive exists. Tag `vX.Y.Z` on the commit: the release workflow verifies the tag equals the workspace version,
builds Windows and Linux packages, signs and attests them and publishes the release.

## Commits and pull requests

Use conventional commit prefixes (`feat:`, `fix:`, `docs:`, `ci:`, ...). Keep pull requests focused and describe how
you tested on real hardware when relevant. By contributing you agree your work is licensed under AGPL-3.0-or-later.
