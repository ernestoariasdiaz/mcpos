#!/usr/bin/env bash
# Boot the latest ISO in QEMU/KVM with a virtio-gpu (3D if the host supports it).
#   ./iso/run-vm.sh            -> GL accelerated (virtio-vga-gl)
#   NOGL=1 ./iso/run-vm.sh     -> plain virtio, sway uses pixman
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ISO="${ISO:-$(ls -t "$ROOT"/out/mcpos-*.iso | head -1)}"
DISK="${DISK:-$ROOT/out/scratch.qcow2}"
[[ -f $DISK ]] || qemu-img create -f qcow2 "$DISK" 20G >/dev/null

if [[ -n ${NOGL:-} ]]; then GPU=(-vga none -device virtio-vga -display gtk)
else GPU=(-vga none -device virtio-vga-gl -display gtk,gl=on); fi

exec qemu-system-x86_64 \
    -enable-kvm -cpu host -smp "${CPUS:-4}" -m "${MEM:-6G}" \
    -machine q35 \
    "${GPU[@]}" \
    -device virtio-keyboard-pci -device virtio-tablet-pci \
    -nic user,model=virtio-net-pci,hostfwd=tcp::2222-:22 \
    -drive file="$DISK",if=virtio \
    -cdrom "$ISO" -boot d
