#!/usr/bin/env bash
# Build Ctrlmate Linux packages from ALREADY BUILT release binaries (target/release):
#   deb, rpm, and (optionally) a Velopack AppImage.
#
#   packaging/linux/pack-linux.sh [--arch x86_64|aarch64] [--out DIR] [--version X.Y.Z]
#                                 [--formats deb,rpm,appimage] [--build]
#
# The version comes from [workspace.package].version (scripts/sync_version.py --print).
# Output is reproducible: mtimes and archive metadata are clamped to the last commit time
# (SOURCE_DATE_EPOCH). Required tools: python3, dpkg-deb (deb), rpmbuild (rpm), vpk (appimage).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

ARCH="$(uname -m)"
OUT="$ROOT/dist-release/linux"
VERSION=""
FORMATS="deb,rpm"
BUILD=0

while [ $# -gt 0 ]; do
    case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --formats) FORMATS="$2"; shift 2 ;;
    --build) BUILD=1; shift ;;
    -h | --help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

WS_VERSION="$(python3 scripts/sync_version.py --print)"
VERSION="${VERSION:-$WS_VERSION}"
if [ "$VERSION" != "$WS_VERSION" ]; then
    echo "error: --version $VERSION differs from workspace version $WS_VERSION" >&2
    exit 1
fi

case "$ARCH" in
x86_64 | amd64) ARCH=x86_64; DEB_ARCH=amd64; RUNTIME=linux-x64; CHANNEL=linux ;;
aarch64 | arm64) ARCH=aarch64; DEB_ARCH=arm64; RUNTIME=linux-arm64; CHANNEL=linux-arm64 ;;
*) echo "unsupported architecture: $ARCH (supported: x86_64, aarch64)" >&2; exit 2 ;;
esac
if [ "$(uname -m)" != "$ARCH" ]; then
    echo "error: binaries in target/release are built for $(uname -m); build on a $ARCH host" >&2
    exit 1
fi

if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
    SOURCE_DATE_EPOCH="$(git log -1 --format=%ct 2>/dev/null || date +%s)"
fi
export SOURCE_DATE_EPOCH

if [ "$BUILD" = 1 ]; then
    (cd apps/desktop && bun install --frozen-lockfile && bun run build)
    cargo build --release --locked -p ctrlmate-daemon -p ctrlmate-agent -p ctrlmate-desktop
fi

BINS=(ctrlmated ctrlmate-agent ctrlmate-desktop)
for b in "${BINS[@]}"; do
    [ -x "target/release/$b" ] || { echo "error: target/release/$b missing (cargo build --release --locked, or pass --build)" >&2; exit 1; }
done

mkdir -p "$OUT"
STAGE="$ROOT/dist-stage/linux-$ARCH"
rm -rf "$STAGE"
mkdir -p "$STAGE"

# Install the common file tree into a root. Used by the deb and as the reference for the rpm/PKGBUILD.
install_tree() {
    local d="$1"
    install -d "$d"
    for b in "${BINS[@]}"; do install -Dm755 "target/release/$b" "$d/usr/bin/$b"; done
    install -Dm644 packaging/linux/ctrlmated.service "$d/usr/lib/systemd/system/ctrlmated.service"
    install -Dm644 packaging/linux/ctrlmate-agent.service "$d/usr/lib/systemd/user/ctrlmate-agent.service"
    install -Dm644 packaging/linux/99-ctrlmate.rules "$d/usr/lib/udev/rules.d/99-ctrlmate.rules"
    install -Dm644 packaging/linux/ctrlmate-i2c.conf "$d/usr/lib/modules-load.d/ctrlmate-i2c.conf"
    install -Dm644 packaging/linux/ctrlmate.desktop "$d/usr/share/applications/ctrlmate.desktop"
    install -Dm644 packaging/linux/ctrlmate-agent.desktop "$d/etc/xdg/autostart/ctrlmate-agent.desktop"
    install -Dm644 apps/desktop/src-tauri/icons/icon.png "$d/usr/share/icons/hicolor/256x256/apps/ctrlmate.png"
    install -Dm644 LICENSE "$d/usr/share/doc/ctrlmate/copyright"
    install -Dm644 LICENSE "$d/usr/share/licenses/ctrlmate/LICENSE"
}

# ---- deb -------------------------------------------------------------------------------------
build_deb() {
    command -v dpkg-deb >/dev/null || { echo "error: dpkg-deb not found" >&2; exit 1; }
    local d="$STAGE/deb"
    install_tree "$d"
    mkdir -p "$d/DEBIAN"
    local size
    size="$(du -sk --exclude=DEBIAN "$d" | cut -f1)"
    cat > "$d/DEBIAN/control" <<EOF
Package: ctrlmate
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: ${DEB_ARCH}
Installed-Size: ${size}
Depends: libc6, libgtk-3-0, libwebkit2gtk-4.1-0, libsoup-3.0-0, libayatana-appindicator3-1 | libappindicator3-1, libudev1, libhidapi-hidraw0, adduser, systemd
Recommends: i2c-tools
Maintainer: Ctrlmate Contributors <noreply@github.com>
Homepage: https://github.com/Thomas-TP/Ctrlmate
Description: PC telemetry, lighting, cooling and Home Assistant control
 Ctrlmate: hardware telemetry, cooling policy with a safety watchdog, RGB and
 Kelvin lighting and a Home Assistant integration. Ships the ctrlmated system
 daemon (unprivileged "ctrlmate" user), the ctrlmate-agent session agent and the
 desktop application.
EOF
    echo "/etc/xdg/autostart/ctrlmate-agent.desktop" > "$d/DEBIAN/conffiles"
    for s in postinst prerm postrm; do install -m755 "packaging/linux/debian/$s" "$d/DEBIAN/$s"; done
    find "$d" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
    dpkg-deb -Zxz --root-owner-group --build "$d" "$OUT/ctrlmate_${VERSION}_${DEB_ARCH}.deb"
}

# ---- rpm -------------------------------------------------------------------------------------
build_rpm() {
    command -v rpmbuild >/dev/null || { echo "error: rpmbuild not found" >&2; exit 1; }
    local top="$STAGE/rpmbuild"
    mkdir -p "$top"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
    sed -e "s|@VERSION@|${VERSION}|g" -e "s|@ROOT@|${ROOT}|g" \
        packaging/linux/ctrlmate.spec.in > "$top/SPECS/ctrlmate.spec"
    rpmbuild -bb --target "${ARCH}-linux" \
        --define "_topdir $top" \
        --define "_buildhost reproducible" \
        --define "_binary_payload w19.xzdio" \
        "$top/SPECS/ctrlmate.spec"
    cp "$top"/RPMS/*/ctrlmate-*.rpm "$OUT/"
}

# ---- AppImage (Velopack) ---------------------------------------------------------------------
# The AppImage bundles the desktop app, the agent and the daemon, with Velopack delta updates.
# It does NOT install the system service, udev rules or the ctrlmate user: use the deb/rpm/AUR
# package (or run the daemon as your user) for hardware access.
build_appimage() {
    command -v vpk >/dev/null || { echo "error: vpk not found (dotnet tool install -g vpk)" >&2; exit 1; }
    local d="$STAGE/appimage"
    mkdir -p "$d"
    for b in "${BINS[@]}"; do install -m755 "target/release/$b" "$d/$b"; done
    install -m644 LICENSE "$d/LICENSE"
    vpk "[linux]" pack \
        --packId Ctrlmate \
        --packVersion "$VERSION" \
        --packTitle "Ctrlmate" \
        --packAuthors "Ctrlmate Contributors" \
        --packDir "$d" \
        --mainExe ctrlmate-desktop \
        --runtime "$RUNTIME" \
        --icon apps/desktop/src-tauri/icons/icon.png \
        --outputDir "$OUT" \
        --channel "$CHANNEL"
}

IFS=',' read -ra FMT <<< "$FORMATS"
for f in "${FMT[@]}"; do
    case "$f" in
    deb) build_deb ;;
    rpm) build_rpm ;;
    appimage) build_appimage ;;
    *) echo "unknown format: $f" >&2; exit 2 ;;
    esac
done

echo "Ctrlmate $VERSION ($ARCH) packages in $OUT:"
ls -l "$OUT"
