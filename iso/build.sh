#!/usr/bin/env bash
# Build the mcpOS live ISO on an Arch host:  sudo ./iso/build.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(dirname "$HERE")"
WORK="${WORK:-/tmp/mcpos-work}"
OUT="${OUT:-$ROOT/out}"
PROFILE="$WORK/profile"
BUILDER="${SUDO_USER:-root}"

[[ $EUID -eq 0 ]] || { echo "mkarchiso necesita root: sudo $0"; exit 1; }
pacman -Qi archiso >/dev/null 2>&1 || pacman -S --needed --noconfirm archiso

echo "==> cargo build --release (como $BUILDER)"
sudo -u "$BUILDER" -H bash -lc "cd '$ROOT' && cargo build --release"

echo "==> perfil releng + overlay mcpOS"
rm -rf "$WORK"
mkdir -p "$WORK"
cp -r /usr/share/archiso/configs/releng "$PROFILE"
grep -v '^#' "$HERE/packages.extra" | sed '/^$/d' >> "$PROFILE/packages.x86_64"
sort -u -o "$PROFILE/packages.x86_64" "$PROFILE/packages.x86_64"
cp -a "$HERE/airootfs/." "$PROFILE/airootfs/"
# files may come from a Windows checkout: strip CRLF from our overlay
( cd "$HERE/airootfs" && find . -type f -print0 ) | while IFS= read -r -d '' f; do
    sed -i 's/\r$//' "$PROFILE/airootfs/$f"
done
# enable units here (symlinks don't survive Windows/zip round-trips)
WANTS="$PROFILE/airootfs/etc/systemd/system/multi-user.target.wants"
mkdir -p "$WANTS"
for u in /etc/systemd/system/mcpos-setup-user.service \
         /usr/lib/systemd/system/vmtoolsd.service \
         /usr/lib/systemd/system/vmware-vmblock-fuse.service; do
    ln -sf "$u" "$WANTS/$(basename "$u")"
done
UWANTS="$PROFILE/airootfs/etc/systemd/user/default.target.wants"
mkdir -p "$UWANTS"
ln -sf /etc/systemd/user/mcpos-ydotoold.service "$UWANTS/mcpos-ydotoold.service"
for b in mcpd mcp-shell mcp-journal mcp-desktop mcp-a11y; do
    install -Dm755 "$ROOT/target/release/$b" "$PROFILE/airootfs/usr/local/bin/$b"
done

sed -i \
    -e 's/^iso_name=.*/iso_name="mcpos"/' \
    -e 's/^iso_label=.*/iso_label="MCPOS_$(date --date="@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y%m)"/' \
    -e 's/^iso_publisher=.*/iso_publisher="mcpOS <https:\/\/github.com\/ernestoariasdiaz>"/' \
    -e 's/^iso_application=.*/iso_application="mcpOS — MCP-native Linux (test build)"/' \
    "$PROFILE/profiledef.sh"
cat >> "$PROFILE/profiledef.sh" <<'PERMS'
file_permissions+=(
  ["/etc/sudoers.d/10-mcp"]="0:0:0440"
  ["/usr/local/sbin/mcpos-setup-user"]="0:0:755"
  ["/usr/local/sbin/mcpos-install"]="0:0:755"
  ["/usr/local/bin/mcpos-agent"]="0:0:755"
  ["/usr/local/bin/mcpd"]="0:0:755"
  ["/usr/local/bin/mcp-shell"]="0:0:755"
  ["/usr/local/bin/mcp-journal"]="0:0:755"
  ["/usr/local/bin/mcp-desktop"]="0:0:755"
  ["/usr/local/bin/mcp-a11y"]="0:0:755"
  ["/usr/local/sbin/mcpos-update"]="0:0:755"
)
PERMS

# bigger RAM overlay so npm installs and coredumps fit in the live session
for f in "$PROFILE"/grub/*.cfg "$PROFILE"/syslinux/*.cfg "$PROFILE"/efiboot/loader/entries/*.conf; do
    [[ -f $f ]] && sed -i 's/archisobasedir=%INSTALL_DIR%/& cow_spacesize=4G/' "$f"
done

echo "==> mkarchiso"
mkdir -p "$OUT"
mkarchiso -v -w "$WORK/build" -o "$OUT" "$PROFILE"
chown -R "$BUILDER" "$OUT" 2>/dev/null || true
ls -lh "$OUT"/*.iso
