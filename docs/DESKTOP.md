# Desktop — a third shell, Windows first

`Apps/Desktop/` is a PC shell for OpenPocketCine. Windows is the target platform;
macOS and Linux come along because nothing in the stack is Windows-specific.

It starts as a **watcher**: a second screen for a feed an iPhone is already hosting.
That ordering is deliberate. A watcher never opens BLE and never joins the camera's
SoftAP on its own, so the first desktop milestone skips the two hardest things to port
and still ships something an operator can use. Direct camera control comes after.

Operator-visible behavior lives in [`PARITY.md`](PARITY.md). The relay contract lives in
[`watcher-relay.md`](watcher-relay.md). This file is the desktop build and I/O notes.

## How the desktop build is structured

1. **Portable Swift core** — `Sources/OpenPocketViewCore/` is already Foundation-only, so
   it compiles for a desktop triple with no changes.
2. **C-ABI facade** — `Sources/OpenPocketCineDesktopFacade/` exports the relay surface
   through `@_cdecl` entry points, the same trick the Android facade uses for JNI.
   `Sources/COpcDesktop/` holds the fixed-layout C records both sides agree on.
3. **Rust host** — `Apps/Desktop/` owns sockets, discovery, decoding, drawing, and the
   command line. Rust is here for one reason: it is the only ecosystem with a single
   Bluetooth API that covers Windows, macOS, and Linux, which is what milestone 3 needs.
4. **One implementation of the protocol.** The host calls the core for framing, kind
   validation, payload limits, join rules, the retry ladder, the delivery-delay guard,
   focus fitting, and every JSON shape. Nothing about the wire format is written twice.

```
OpenPocketViewCore (Swift)  →  OpenPocketCineDesktopFacade (@_cdecl)  →  opc-core-sys
                                                                            ↓
                                                        opc-relay  →  opc-watcher
```

| Crate | Owns |
| --- | --- |
| `opc-core-sys` | Raw FFI declarations and the `#[repr(C)]` mirrors of the shared header |
| `opc-relay` | Receive buffer, Bonjour discovery, TCP transport, the join state machine |
| `opc-watcher` | The command-line shell for milestone 1 |

## What the shell owns

Sockets, Bonjour, decoding, GPU, windowing, credential storage, and the command line.
That is the same split the iOS and Android shells follow — see
[`ARCHITECTURE.md`](ARCHITECTURE.md).

## What the core owns

`WatcherRelayProtocol`, `WatcherRelayFraming`, `WatcherRelayMessages`,
`WatcherRelayFrameBlob`, `WatcherRelayRecovery`, `WatcherRelayFrameFreshness`, and
`WatcherFocusPoint`. The ABI is defined in
[`opc_desktop_types.h`](../Sources/COpcDesktop/include/opc_desktop_types.h). Its record
layout is asserted from both sides — `DesktopAbiLayoutTests` in Swift and the `layout`
tests in `opc-core-sys` — so a field added on one side fails the other.

## Build and run

```sh
just desktop-core     # build the Swift core as a shared library
just desktop-build    # build the Rust host against it
just desktop-check    # fmt, clippy, and the full Rust test suite
```

Then, with the PC on the camera's Wi-Fi and Sharing on in the host's Operator Setup:

```sh
cd Apps/Desktop
cargo run -p opc-watcher -- list
cargo run -p opc-watcher -- join "Studio iPhone" --seconds 30 --dump feed.h265
```

`--dump` writes the received access units straight to disk. The host emits Annex-B with
parameter sets inline on every keyframe, so that file plays in `ffplay` or VLC with no
container — which is how you confirm the transport works before there is a renderer.

Without the Swift library the Rust workspace still type-checks and its pure-Rust tests
still run; `build.rs` only emits link flags when it finds the library, and says so
otherwise. Anything that calls the core fails at link rather than running a stub.

## Windows notes

- **Wi-Fi is an advantage here.** The camera's SoftAP carries no internet. A desktop with
  Ethernet for the network and a Wi-Fi adapter for the camera avoids the trade-off a
  phone cannot escape. Set the Wi-Fi interface metric higher than Ethernet so routing
  prefers the wire.
- **DLL placement.** Windows resolves DLLs beside the executable rather than by rpath, so
  `OpenPocketCineDesktop.dll` has to sit next to `opc-watcher.exe`. `OPC_CORE_LIB_DIR`
  covers the link step, not the run step.
- **Bonjour.** Discovery is pure Rust mDNS; it does not need Apple's Bonjour service
  installed. A firewall prompt on first run is expected — UDP 5353 inbound.

## Milestones

| # | Scope | State |
| --- | --- | --- |
| 1 | Discovery, join, telemetry, picture ingest, HEVC dump | In tree, **not physically verified** |
| 2 | Decode and draw: FFmpeg plus a GPU present path with the 3D LUT cube and assists | Not started |
| 3 | Direct camera session: BLE credential read, SoftAP join, UDP datalink, the ACK pump | Not started |

Milestone 2 should port `Apps/Android/app/src/main/cpp/opc_vulkan.cpp` rather than write a
third renderer — it is already Vulkan, and only its `AHardwareBuffer` import and
`VK_KHR_android_surface` swapchain are Android-specific.

Milestone 3 is where [`live-session.md`](live-session.md) becomes required reading. The
40 Hz window-ACK discipline is the thing most likely to produce a session that connects,
shows telemetry, and never shows a picture.

## Verification

`just desktop-check` is the desktop gate. It is **not** proof of operator-visible
behavior: per [`AGENTS.md`](../AGENTS.md), that needs a real camera, a real host phone,
and a real PC. Milestone 1 has passing unit and ABI tests and has not been run against a
live host. Do not describe it as working until it has been.
