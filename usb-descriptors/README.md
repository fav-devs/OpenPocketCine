# DJI Osmo Pocket 3 – USB Descriptor Captures

Captured on Windows 11 from a DJI Osmo Pocket 3 (VID 2CA3) connected over USB-C.
Each sub-folder holds the output for one USB mode offered by the camera body.

## Modes

| Folder | Camera screen label | PID | Interfaces |
|--------|--------------------|----|------------|
| `webcam-windows` | Webcam | 0023 | MI_00 Camera (UVC) + MI_02 MEDIA (UAC audio) |
| `transfer-windows` | Transfer File | 0020 | MI_00 RNDIS + MI_02 MassStorage (UMS) + MI_03–07 BULK vendor (Class FF/SubClass 43) |
| `chargeonly-windows` | Charge Only | — | No USB interfaces; power only |
| `prompt-windows` | (USB-mode prompt on screen, nothing selected) | — | No USB interfaces |

## Key observations

- **Webcam (PID 0023)**: Standard UVC+UAC composite — recognized as a webcam on Windows with no
  driver installation. Class 0x0E (video) + Class 0x01 (audio).
- **Transfer (PID 0020)**: Composite device with 8 interfaces. Notable:
  - `MI_00`: RNDIS (Class E0/01/03) — a USB-Ethernet network interface.
  - `MI_02`: USB Mass Storage (Class 08/06/50, USBSTOR) — mounts the SD card as two LUNs
    (`Linux File-Stor Gadget`).
  - `MI_03–07`: Five vendor-specific BULK interfaces (Class FF / SubClass 43 / Protocol 01) —
    DJI's proprietary DUML/control channels. Windows shows these as "BULK Interface" with no
    driver (Error status); they need a custom driver or WinUSB to talk to.
- **Charge Only / Prompt**: Camera does not place any device on the USB bus.

## Files per folder

- `README.txt` — mode, host OS, capture timestamp
- `pnp-devices.txt` — per-device PnP properties (HardwareIds, CompatibleIds, Service, etc.)
- `pnp-table.txt` — summary table of all DJI-associated PnP nodes
- `pnputil.txt` — `pnputil /enum-devices /connected` full output
- `win32-pnpentity.txt` — Win32_PnPEntity query filtered to VID_2CA3
- `raw-descriptors.txt` — extended PnP properties including USB descriptor bytes where available
- `NOTE-no-device.txt` — present instead of the above when no device enumerated

## Capture tool

`tools/usb-descriptor-dump.ps1` — run once per mode, pass the mode name as the first argument.
Requires Windows; optionally uses UsbTreeView.exe for raw descriptor trees.
