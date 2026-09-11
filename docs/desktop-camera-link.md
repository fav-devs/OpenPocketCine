# Desktop camera link

How a PC talks to the camera itself, rather than to a phone that is already talking to
it. This is the v1 the desktop shell is being built toward: the laptop as the
viewfinder, with gimbal, zoom, recording and tracking.

The relay watcher ([`DESKTOP.md`](DESKTOP.md)) stays useful — it is a second screen for
a feed a phone hosts — but it is a different thing from this.

## The spine

Same five steps as the phones, from [`ARCHITECTURE.md`](ARCHITECTURE.md):

1. **Bluetooth.** Scan, pair, and talk GATT service FFF0.
2. **Credentials.** Ask the camera for its own Wi-Fi name and password over that link.
3. **Wi-Fi.** Join the camera's SoftAP. On-path only once DHCP has handed out an address
   in `192.168.2.2…254`.
4. **UDP.** Bind an **ephemeral local port** and send to `192.168.2.1:9004`. Binding
   local `:9004` is a known way to get telemetry while every video packet is dropped.
5. **Live view.** One `0x09/0xa8`, after the path and the display are both ready. It is
   enable-**once**: every later enable belongs to the watchdog, not to the connect path.

Steps 1 and 3 are the parts a PC does differently from a phone. Everything after that is
the same protocol the phones speak, and the desktop shell reaches it through the same
core.

## What the core owns, and what the shell owns

| The core decides | The shell does |
| --- | --- |
| Every opcode and payload (`Commands`) | Opens the socket, sends the bytes |
| Frame encoding and CRC (`Duml`) | Reads datagrams off the wire |
| Transport, routing and handshake headers (`DumlTransport`) | Runs the 40 Hz clock |
| Which acknowledgement cursor may move, and when (`AckWindows`) | Feeds it every datagram |
| Retry ladders, stall deadlines, first-picture gates | Bluetooth, Wi-Fi, permissions, UI |

`Sources/OpenPocketCineDesktopFacade/DesktopCameraABI.swift` and
`DesktopTransportABI.swift` are the seam. `Apps/Desktop/crates/opc-camera/` is the typed
Rust side of it. No opcode, payload byte, or cursor rule is written twice.

## The acknowledgement pump

This is the part most likely to go wrong, and it fails quietly.

Forty times a second the app sends a pktType-`0x04` datagram carrying three window
cursors. Get them wrong and the camera keeps sending telemetry and keeps answering
commands while it stops sending pictures — so the symptom is a live HUD over a black
frame, not an error message.

| Group | Follows | Rule |
| --- | --- | --- |
| 0 | latest pktType-`0x02` (video) | Telemetry may **seed** it before the first picture and must never move it after |
| 1 | latest pktType-`0x03` (command replies) | Telemetry must not rewind it once a reply has been seen |
| 2 | telemetry | Follows telemetry throughout |

Two details that look like details and are not:

- **Zero is a real cursor.** Sequence `0` is a valid 8-aligned value. "Never seen" and
  "seen, and it was zero" have to stay distinguishable, or the pump sends the handshake
  base when it should send zero.
- **Group 0 is the shell's.** `AckWindows.advancing` handles groups 1 and 2 only. Video
  is tracked separately because it is the transport sequence of the last video packet,
  which only the thing reading the socket knows.

`AckPump` in `opc-camera` holds all of this. Its tests in `crates/opc-camera/tests/wire.rs`
assert each rule directly, including that telemetry cannot pull group 0 backwards after a
picture has arrived.

Every UDP write — acknowledgements, camera SETs, gimbal stick, parameter GETs — has to
serialise on one queue. On iOS, interleaving a main-thread send with the 40 Hz pump
starved the window acknowledgement. See [`live-session.md`](live-session.md).

## Bluetooth, per platform

The shell's job, and the least portable piece in the port.

| Platform | Stack | Notes |
| --- | --- | --- |
| Windows | WinRT `Windows.Devices.Bluetooth` | The primary target. Pairing state is owned by the OS, so a camera paired once may need no prompt again. |
| Linux | BlueZ over D-Bus | Closest to what CI can exercise. |
| macOS | CoreBluetooth | Same stack the iOS shell uses. |

`btleplug` covers all three behind one API, which is the main reason the host is Rust.
Only credential reading needs Bluetooth; once the Wi-Fi is joined the camera session is
pure UDP.

## Wi-Fi, per platform

Joining the camera's SoftAP means leaving whatever network the laptop was on, because
the camera's network has no internet.

A desktop handles this better than a phone: plug in Ethernet, keep Wi-Fi for the camera,
and set the Wi-Fi interface metric higher so routing prefers the wire. That is a real
advantage of this port, not a workaround.

| Platform | Join | Notes |
| --- | --- | --- |
| Windows | WLAN API, or a temporary profile via `netsh` | Needs the profile removed on disconnect so the laptop goes back to its normal network. |
| Linux | NetworkManager (`nmcli`) or wpa_supplicant | |
| macOS | CoreWLAN `CWInterface.associate` | No entitlement dance, unlike iOS. |

A local VPN that did not opt out of the process can take the route even after the join.
`LocalVPNFilter` in the core carries the operator-facing hint for that case.

## v1 feature map

What the operator asked for, and where each piece stands.

| Feature | Core support | Facade | Shell |
| --- | --- | --- | --- |
| Live picture | — | — | **Done** (decode, grade, window) |
| Record start/stop | `Commands.recordStart` / `recordStop` | **Done** | Not wired |
| Record timer | none needed — a countdown then a start | n/a | Not wired |
| Zoom | `setZoom`, `setZoomSlew`, `setZoomStop` | **Done** | Not wired |
| Gimbal | `gimbalStick`, `gimbalRecenter`, `gimbalFlip`, speed, tilt lock | **Done** | Not wired |
| Tracking | `setTrackingBox`, `clearTrackingBox`, `pollTracking` | **Done** | Not wired |
| Frame rate and resolution | `setVideoFormat` | **Done** | Not wired |
| ISO, shutter, EV, white balance | `setIsoIndex`, `setShutter`, `setEv`, `setWhiteBalance` | **Done** | Not wired |
| Bluetooth pairing | `getWifiSsid`, `getWifiPassword` | **Done** | Not started |
| The UDP session itself | `DumlTransport`, `AckWindows` | **Done** | Not started |

The commands exist; the session that carries them does not yet.

## What is not covered yet

- **The SET mailbox.** `CameraSetMailbox` in the core owns retransmit and settle timing —
  a missed acknowledgement must not revert what the operator sees. Until it is exposed,
  desktop SETs are fire-and-forget and a dropped one is a control that silently did not
  take.
- **Status decode.** `CameraStatus` turns telemetry into battery, format, ISO and REC
  state. The HUD needs it.
- **The DJI frame marker.** The camera's own stream carries a private per-frame NAL that
  `Hevc.stripDjiMarker` removes. The relay's re-encode is already clean, so the watcher
  never needed it; a direct session does.

## Verification

`just desktop-check` builds the facade and runs the Rust suite. The wire tests run only
when the Swift core is linked, which is what the `desktop` job in
[`ci.yml`](../.github/workflows/ci.yml) is for.

None of the link has run against a camera. Nothing here should be described as working
until it has.
