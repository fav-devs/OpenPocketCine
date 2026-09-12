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

## The session

`CameraSession` in `opc-camera` owns the socket and the clock. Everything it sends is
decided elsewhere:

- **`Sequencer`** says *what is due*. It is free of sockets and of the core, so the two
  rules easiest to get wrong are testable against a fake clock: the pump fires at 40 Hz
  and never bursts a backlog after a stall, and live view is enabled **exactly once** per
  session. Repeats belong to the watchdog, which is not built yet.
- **`AckPump`** says what an acknowledgement carries.
- **The core** says what every byte is.

Sequence bookkeeping is the one thing the session decides for itself, transcribed from
the iOS driver: the DUML frame sequence advances by one per command, the transport
sequence by eight, and the command counter by one. A command datagram is
`transportHeader(0x05) + routingHeader + Duml.encode(frame)`.

One detail worth writing down because it is easy to get backwards: the reassembler takes
the **whole datagram**, not the payload. It reads the packet type at byte 6 and the
fragment index at bytes 16 to 18, and the encoded body only starts at byte 20. Handing it
`datagram[8..]` produces no pictures at all, silently.

### The fake camera

`crates/opc-camera/tests/session.rs` runs the datalink against a camera that is not
there: a UDP peer on loopback that answers the handshake, streams video packets, replies
to commands, and records everything it was sent. It is what makes the black-picture
failure something a test can catch — the session's cadence, its enable-once, and its
command delivery are all asserted from the camera's side of the wire.

It also asserts the datalink never binds the camera's own port.

## What the camera says about itself

The body describes its state in a stream of pushes, and `CameraStatusDecoder` already
knows how to read every one — including which model encodes a colour mode which way. The
desktop session folds them in and the shell reads the result.

Two kinds arrive. Ordinary status frames carry battery, recording state, elapsed time,
ISO, shutter, white balance, format and zoom. `0x00/0x99` subscription pushes carry
timecode and, more usefully, **the lists of values this body actually offers** — shutter
speeds, ISO indices, video formats, colour modes. A picker that invents its own list
offers an operator settings the camera will refuse, so the session subscribes as soon as
it opens.

Two habits carry through the whole record: `-1` means the camera has not said, and a
field that can legitimately be negative — exposure compensation, white-balance tint —
gets its own `has_` flag instead. A camera reporting ISO 0 is not a camera that has said
nothing.

`set_model` tells the decoder which body this is. Without it the model-specific
encodings fall back to what every Osmo shares, which reads some colour modes wrong on a
Pocket 3 or a Nano.

## When the feed stops

A frozen feed does not announce itself. The socket stays open, telemetry keeps arriving,
commands keep working, and the picture simply stops — so nothing in a log says anything
is wrong.

`FeedWatchdog` in the core owns the answer, and both phones already run it. The desktop
session supplies observations and carries out the rung it is handed:

| Rung | Who acts |
| --- | --- |
| Resend the live-view enable | The session |
| Rebuild the decoder | The shell — it owns the decoder |
| Reopen the datalink | The session: reset reassembly, handshake again |
| Full rejoin | The shell — it owns Bluetooth |

The resend deliberately bypasses the sequencer. The connect path enables live view
exactly once, and every repeat after that is the watchdog's, which is the contract in
[`feed-watchdog.md`](feed-watchdog.md). Reopening the datalink restarts the sequencer,
so the new session gets its own single enable — that is not the same as the connect path
sending two.

`FeedHealth` remembers when things last happened. It is pure, so the bookkeeping is
tested on its own: a negative age means *never seen* and zero means *just now*, and
collapsing the two would quietly disarm the whole ladder. A rebuild forgets the feed but
keeps the fact that there was one, because the watchdog treats a feed that never started
differently from one that stopped.

The fake camera covers the whole story end to end: it keeps answering and stops sending
pictures, and the test asserts the session notices, asks for the feed again, and leaves a
healthy feed alone.

## Pairing

The exchange that ends with the camera's Wi-Fi credentials, from the protocol notes:

1. Wake the session (`0x00/0x2B`).
2. Offer a pairing PIN (`0x07/0x45`). The camera answers `00 01` if it already knows
   this client, or `00 02` if a human has to approve on the body.
3. On approval it sends its own `0x07/0x46` **request** — answer that, echoing its
   sequence.
4. Ask it to wake its access point (`0x53/0x10`).
5. Read the Wi-Fi name (`0x07/0x07`), then the password (`0x07/0x0E`).

`Pairing` in `opc-camera` runs that with no radio attached: it decides the next step
from what the camera has said, and the driver turns steps into writes. So the branch that
needs a person standing at the camera is testable, as is the one where a step goes
unanswered and has to be retried, and the deadline that gives up rather than hanging.

A notification is not a frame. The camera splits replies across several, and the core's
`DumlNotificationAssembler` puts them back together — a reader that treats each
notification as complete sees truncated payloads rather than an error.

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

`BleTransport` is the interface a stack has to provide: scan, connect, write a frame,
hand over notifications, disconnect. It is deliberately small, because everything about
*what* to write is `Pairing`'s and everything about what the bytes mean is the core's.
**The `btleplug` implementation behind it is not written yet.**

## Wi-Fi, per platform

Joining the camera's SoftAP means leaving whatever network the laptop was on, because
the camera's network has no internet.

A desktop handles this better than a phone: plug in Ethernet, keep Wi-Fi for the camera,
and set the Wi-Fi interface metric higher so routing prefers the wire. That is a real
advantage of this port, not a workaround.

| Platform | Join | Notes |
| --- | --- | --- |
| Windows | `netsh wlan`: add a profile, then connect | The profile is written `connectionMode=manual`, so the laptop does not wander back onto the camera after the shoot. |
| Linux | `nmcli device wifi connect` | |
| macOS | `networksetup -setairportnetwork` | No entitlement dance, unlike iOS. |

`JoinPolicy` decides *whether to try again and when*, on the core's deadlines — 90
seconds in total, 10 between attempts, and no attempt started that cannot finish before
the deadline. `command_line` builds the argument list for each platform's tool, and
`profile_xml` builds the Windows document. Both are tested without running anything;
only the running is untested, and it is three lines behind the `WifiJoiner` trait.

One rule worth knowing: a platform that will not say which network it is on counts as
being on target. Refusing to proceed on that basis strands an operator who is in fact
connected.

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
| Bluetooth pairing | `getWifiSsid`, `getWifiPassword`, `BleAdvert`, the notification assembler | **Done** | Flow **done**; the radio behind it is not |
| Wi-Fi join | `CameraSoftAPSwitch` | **Done** | Policy and commands **done**; the runner is not |
| The UDP session itself | `DumlTransport`, `AckWindows`, `HevcDepacketizer` | **Done** | **Done** |
| Surviving a frozen feed | `FeedWatchdog` | **Done** | **Done** |
| HUD telemetry | `CameraStatusDecoder` | **Done** | **Done** |

The session runs and carries commands. What is missing before an operator sees anything
is Bluetooth pairing and the Wi-Fi join in front of it, and the UI behind it.

## What is not covered yet

- **The platform implementations.** `Pairing` and `JoinPolicy` decide what to do;
  `BleTransport` and `WifiJoiner` are the interfaces that would carry it out, and neither
  has an implementation yet. Until they do, the operator joins the Wi-Fi by hand.
- **The SET mailbox.** `CameraSetMailbox` owns retransmit and settle timing — a missed
  acknowledgement must not revert what the operator sees. Until it is exposed, desktop
  SETs are fire-and-forget and a dropped one is a control that silently did not take.
- **Reconnect.** `SessionRecovery` bounds the retry after a drop.

The DJI per-frame marker is already handled: `Hevc.stripDjiMarker` runs inside the
reassembler, so a direct session gets clean access units without the shell doing
anything.

## Verification

`just desktop-check` builds the facade and runs the Rust suite. The wire tests run only
when the Swift core is linked, which is what the `desktop` job in
[`ci.yml`](../.github/workflows/ci.yml) is for.

None of the link has run against a camera. Nothing here should be described as working
until it has.
