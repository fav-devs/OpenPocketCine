#!/usr/bin/env bash
# Dumps everything the OS knows about a connected DJI camera's USB descriptors.
# Run once per USB mode the body offers (Webcam, Transfer File, anything else),
# naming the mode:  tools/usb-descriptor-dump.sh webcam
# Output lands in usb-descriptors/<mode>-<host>/; paste or commit the folder.
set -euo pipefail

mode="${1:-unknown}"
vendor="2ca3"   # DJI
out="usb-descriptors/${mode}-$(uname -s | tr '[:upper:]' '[:lower:]')"
mkdir -p "$out"
echo "mode: $mode" > "$out/README.txt"
echo "host: $(uname -a)" >> "$out/README.txt"
date -u +"when: %Y-%m-%dT%H:%M:%SZ" >> "$out/README.txt"

case "$(uname -s)" in
  Linux)
    if ! command -v lsusb >/dev/null; then
      echo "lsusb is missing: install usbutils (apt install usbutils / dnf install usbutils)" >&2
      exit 1
    fi
    lsusb > "$out/lsusb.txt"
    if ! grep -qi "ID $vendor:" "$out/lsusb.txt"; then
      echo "no DJI device (vendor $vendor) on the bus — is the camera plugged in and in a USB mode?" >&2
      exit 2
    fi
    # Full descriptors need root for the strings; try without first.
    (sudo -n true 2>/dev/null && sudo lsusb -v -d "$vendor:" || lsusb -v -d "$vendor:") > "$out/lsusb-verbose.txt" 2>&1 || true
    lsusb -t > "$out/lsusb-tree.txt" 2>&1 || true
    # The kernel's view: which driver claimed each interface, and the raw descriptors.
    for dev in /sys/bus/usb/devices/*; do
      [[ -f "$dev/idVendor" ]] || continue
      [[ "$(cat "$dev/idVendor")" == "$vendor" ]] || continue
      name="$(basename "$dev")"
      {
        echo "device $name"
        for f in idVendor idProduct bcdDevice manufacturer product serial bNumInterfaces bDeviceClass bDeviceSubClass bDeviceProtocol bcdUSB speed; do
          [[ -f "$dev/$f" ]] && printf '  %s: %s\n' "$f" "$(cat "$dev/$f" 2>/dev/null)"
        done
        for iface in "$dev"/"$name":*; do
          [[ -d "$iface" ]] || continue
          printf '  interface %s class %s/%s/%s driver %s\n' "$(basename "$iface")" \
            "$(cat "$iface/bInterfaceClass")" "$(cat "$iface/bInterfaceSubClass")" "$(cat "$iface/bInterfaceProtocol")" \
            "$(basename "$(readlink -f "$iface/driver" 2>/dev/null || echo none)")"
        done
      } >> "$out/sysfs.txt"
      cp "$dev/descriptors" "$out/descriptors-$name.bin" 2>/dev/null || true
    done
    dmesg 2>/dev/null | grep -i -E "usb|uvc|dji" | tail -60 > "$out/dmesg.txt" || true
    ls -l /dev/video* > "$out/video-devices.txt" 2>&1 || true
    if command -v v4l2-ctl >/dev/null; then
      for v in /dev/video*; do
        { echo "== $v"; v4l2-ctl -d "$v" --all; v4l2-ctl -d "$v" --list-formats-ext; } >> "$out/v4l2.txt" 2>&1 || true
      done
    fi
    ;;
  Darwin)
    system_profiler SPUSBDataType -json > "$out/system_profiler.json" 2>/dev/null || true
    system_profiler SPUSBDataType > "$out/system_profiler.txt" 2>/dev/null || true
    if ! grep -qi "0x$vendor" "$out/system_profiler.txt"; then
      echo "no DJI device (vendor 0x$vendor) on the bus — is the camera plugged in and in a USB mode?" >&2
      exit 2
    fi
    # ioreg carries the interface classes and which driver matched each one.
    ioreg -p IOUSB -l -w0 > "$out/ioreg-iousb.txt" 2>/dev/null || true
    ioreg -r -c IOUSBHostInterface -l -w0 > "$out/ioreg-interfaces.txt" 2>/dev/null || true
    if command -v lsusb >/dev/null; then lsusb -v -d "$vendor:" > "$out/lsusb-verbose.txt" 2>&1 || true; fi
    if command -v cyme >/dev/null; then cyme --vidpid "$vendor" --verbose 3 > "$out/cyme.txt" 2>&1 || true; fi
    log show --last 10m --predicate 'subsystem contains "usb" or process contains "usb"' > "$out/log-usb.txt" 2>/dev/null || true
    ;;
  *)
    echo "on Windows run tools/usb-descriptor-dump.ps1" >&2
    exit 1
    ;;
esac

echo "wrote $out:"
ls -1 "$out"
