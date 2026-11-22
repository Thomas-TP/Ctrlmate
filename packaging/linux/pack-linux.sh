#!/usr/bin/env bash
set -euo pipefail

VERSION="${1:-0.4.0}"
OUT_DIR="${2:-dist-velopack/linux}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

STAGE_DIR="$REPO_ROOT/dist-stage/linux-x64"
rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR" "$OUT_DIR"

cp target/release/ctrlmate-desktop "$STAGE_DIR/"
cp target/release/ctrlmated "$STAGE_DIR/"
cp LICENSE "$STAGE_DIR/"
chmod +x "$STAGE_DIR/ctrlmate-desktop" "$STAGE_DIR/ctrlmated"

# 1. Velopack Linux AppImage + delta feed (releases.linux.json)
vpk "[linux]" pack \
  --packId Ctrlmate \
  --packVersion "$VERSION" \
  --packTitle "Ctrlmate" \
  --packAuthors "Ctrlmate Contributors" \
  --packDir "$STAGE_DIR" \
  --mainExe ctrlmate-desktop \
  --runtime linux-x64 \
  --icon apps/desktop/src-tauri/icons/icon.png \
  --outputDir "$OUT_DIR" \
  --channel linux

# 2. Native .deb package (Debian / Ubuntu / Mint / Pop!_OS)
DEB_ROOT="$REPO_ROOT/dist-stage/deb"
rm -rf "$DEB_ROOT"
mkdir -p \
  "$DEB_ROOT/DEBIAN" \
  "$DEB_ROOT/usr/bin" \
  "$DEB_ROOT/usr/lib/systemd/system" \
  "$DEB_ROOT/usr/lib/udev/rules.d" \
  "$DEB_ROOT/usr/share/applications" \
  "$DEB_ROOT/usr/share/icons/hicolor/256x256/apps" \
  "$DEB_ROOT/usr/share/licenses/ctrlmate"

cp target/release/ctrlmate-desktop "$DEB_ROOT/usr/bin/ctrlmate-desktop"
cp target/release/ctrlmated "$DEB_ROOT/usr/bin/ctrlmated"
cp packaging/linux/ctrlmated.service "$DEB_ROOT/usr/lib/systemd/system/ctrlmated.service"
cp packaging/linux/99-ctrlmate.rules "$DEB_ROOT/usr/lib/udev/rules.d/99-ctrlmate.rules"
cp apps/desktop/src-tauri/icons/icon.png "$DEB_ROOT/usr/share/icons/hicolor/256x256/apps/ctrlmate.png"
cp LICENSE "$DEB_ROOT/usr/share/licenses/ctrlmate/LICENSE"

cat > "$DEB_ROOT/usr/share/applications/ctrlmate.desktop" <<'EOF'
[Desktop Entry]
Name=Ctrlmate
Comment=Modern PC telemetry, lighting, cooling and Home Assistant control
Exec=/usr/bin/ctrlmate-desktop
Icon=ctrlmate
Terminal=false
Type=Application
Categories=System;Utility;
EOF

cat > "$DEB_ROOT/DEBIAN/control" <<EOF
Package: ctrlmate
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: amd64
Depends: libgtk-3-0, libwebkit2gtk-4.1-0 | libwebkit2gtk-4.0-37, libhidapi-hidraw0
Maintainer: Ctrlmate Contributors <https://github.com/Thomas-TP/Ctrlmate>
Description: Modern PC telemetry, lighting, cooling and system control for Home Assistant 2026.11
EOF

cat > "$DEB_ROOT/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if command -v udevadm >/dev/null 2>&1; then
  udevadm control --reload-rules || true
  udevadm trigger || true
fi
if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload || true
  systemctl enable --now ctrlmated.service || true
fi
EOF
chmod 755 "$DEB_ROOT/DEBIAN/postinst"

dpkg-deb --build --root-owner-group "$DEB_ROOT" "$OUT_DIR/ctrlmate_${VERSION}_amd64.deb"

# 3. Native .rpm package (Fedora / RHEL / openSUSE) when fpm or rpmbuild is available
if command -v rpmbuild >/dev/null 2>&1; then
  RPM_TOP="$REPO_ROOT/dist-stage/rpmbuild"
  mkdir -p "$RPM_TOP"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
  cat > "$RPM_TOP/SPECS/ctrlmate.spec" <<EOF
Name:           ctrlmate
Version:        ${VERSION}
Release:        1%{?dist}
Summary:        Modern PC telemetry, lighting, cooling and Home Assistant 2026.11 suite
License:        AGPL-3.0-or-later
URL:            https://github.com/Thomas-TP/Ctrlmate

%description
Open-source cross-platform PC telemetry, lighting (RGB & Kelvin), cooling and OS control for Home Assistant 2026.11.

%install
mkdir -p %{buildroot}/usr/bin %{buildroot}/usr/lib/systemd/system %{buildroot}/usr/lib/udev/rules.d %{buildroot}/usr/share/applications %{buildroot}/usr/share/icons/hicolor/256x256/apps
install -m 755 $REPO_ROOT/target/release/ctrlmate-desktop %{buildroot}/usr/bin/ctrlmate-desktop
install -m 755 $REPO_ROOT/target/release/ctrlmated %{buildroot}/usr/bin/ctrlmated
install -m 644 $REPO_ROOT/packaging/linux/ctrlmated.service %{buildroot}/usr/lib/systemd/system/ctrlmated.service
install -m 644 $REPO_ROOT/packaging/linux/99-ctrlmate.rules %{buildroot}/usr/lib/udev/rules.d/99-ctrlmate.rules
install -m 644 $DEB_ROOT/usr/share/applications/ctrlmate.desktop %{buildroot}/usr/share/applications/ctrlmate.desktop
install -m 644 $REPO_ROOT/apps/desktop/src-tauri/icons/icon.png %{buildroot}/usr/share/icons/hicolor/256x256/apps/ctrlmate.png

%files
/usr/bin/ctrlmate-desktop
/usr/bin/ctrlmated
/usr/lib/systemd/system/ctrlmated.service
/usr/lib/udev/rules.d/99-ctrlmate.rules
/usr/share/applications/ctrlmate.desktop
/usr/share/icons/hicolor/256x256/apps/ctrlmate.png
EOF
  rpmbuild --define "_topdir $RPM_TOP" -bb "$RPM_TOP/SPECS/ctrlmate.spec"
  cp "$RPM_TOP"/RPMS/*/*.rpm "$OUT_DIR/"
fi

ls -lh "$OUT_DIR"
