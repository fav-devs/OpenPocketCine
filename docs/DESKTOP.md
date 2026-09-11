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
| `opc-decode` | HEVC and AVC decoding over libavcodec, and Annex-B replay of a dump |
| `opc-render` | The Vulkan feed pipeline, the cube upload, and PNG stills |
| `opc-watcher` | The command-line shell |

## What the shell owns

Sockets, Bonjour, decoding, GPU, windowing, credential storage, and the command line.
That is the same split the iOS and Android shells follow — see
[`ARCHITECTURE.md`](ARCHITECTURE.md).

## The feed pipeline

Three passes, in the phones' order:

1. `ycbcr.frag` converts the decoder's planes to RGB **at the source raster**.
2. `feed.frag` grades that RGB through the colour cube, still at the source raster.
3. `blit.frag` stretches the graded picture to the display raster.

Cube at the feed raster, *then* stretch. Cubing after the upsample blotched D-Log2 on
Android ([`../ANDROID.md`](../ANDROID.md)) and would here too.

Only the first pass is new. Passes 2 and 3 are
`Apps/Android/app/src/main/cpp/shaders/`, compiled from where they live rather than
copied, so the grade a PC shows cannot drift from the grade a phone shows. Android
converts with a `VkSamplerYcbcrConversion` over the decoder's AHardwareBuffer; software
decode hands over three separate planes, so that conversion is written out in
`crates/opc-render/shaders/ycbcr.frag` using the same BT.709 limited-range matrix.

`feed.frag` samples five bindings unconditionally. The ones an operator has turned off
get a 1×1 texture and a zeroed `*On` flag, which keeps the shared shader untouched.
Scopes, peaking, false colour, and zebra are wired in the shader but not yet driven from
the desktop shell.

## What the core owns

`WatcherRelayProtocol`, `WatcherRelayFraming`, `WatcherRelayMessages`,
`WatcherRelayFrameBlob`, `WatcherRelayRecovery`, `WatcherRelayFrameFreshness`, and
`WatcherFocusPoint`. The ABI is defined in
[`opc_desktop_types.h`](../Sources/COpcDesktop/include/opc_desktop_types.h). Its record
layout is asserted from both sides — `DesktopAbiLayoutTests` in Swift and the `layout`
tests in `opc-core-sys` — so a field added on one side fails the other.

## Build and run

Prerequisites: a Swift toolchain, FFmpeg development libraries, a Vulkan loader, and a
GLSL compiler (`glslc` from the Vulkan SDK, or `glslangValidator` from glslang).

```sh
# Debian or Ubuntu
sudo apt install libavcodec-dev libavutil-dev libswscale-dev libvulkan-dev glslang-tools

just desktop-core     # build the Swift core as a shared library
just desktop-build    # build the Rust host against it
just desktop-check    # fmt, clippy, and the full Rust test suite
```

On Windows, point `FFMPEG_DIR` at a prebuilt shared FFmpeg (its `include/` and `lib/`)
and install the Vulkan SDK for `glslc`.

The shell links the Swift core, so `opc-watcher` needs `just desktop-core` first. The
library crates build and test without it; `build.rs` emits link flags only when the core
is present and says so when it is not.

Then, with the PC on the camera's Wi-Fi and Sharing on in the host's Operator Setup:

```sh
cd Apps/Desktop
cargo run -p opc-watcher -- list
cargo run -p opc-watcher -- join "Studio iPhone" --seconds 30 --dump feed.h265 --still first.png
cargo run -p opc-watcher -- decode feed.h265 --out stills --look Contrast --frames 10
```

`--dump` writes the received access units straight to disk. The host emits Annex-B with
parameter sets inline on every keyframe, so that file plays in `ffplay` or VLC with no
container — which is how you confirm the transport works.

`--still` decodes the live feed and writes the first picture through the whole pipeline,
and `decode` replays a dump through the same decoder and grade. Between them the path is
checked end to end before there is a window to draw in.

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
| 2a | Decode and grade: libavcodec, the Vulkan feed pipeline, the cube, PNG stills | In tree, **not physically verified** |
| 2b | A window: swapchain present, resize, and the assists the shader already carries | Not started |
| 3 | Direct camera session: BLE credential read, SoftAP join, UDP datalink, the ACK pump | Not started |

Milestone 2b is the swapchain and the operator chrome. The pipeline renders into an image
today; presenting it means a surface, a swapchain, in-flight frames, and resize handling
on top of what is here. `blit.frag` already takes the `uvMode` rotation the phones use.

Milestone 3 is where [`live-session.md`](live-session.md) becomes required reading. The
40 Hz window-ACK discipline is the thing most likely to produce a session that connects,
shows telemetry, and never shows a picture. `Hevc.stripDjiMarker` in the core becomes
relevant there too: the relay's re-encode is clean, but the camera's own stream carries
DJI's private per-frame marker.

## Verification

`just desktop-check` is the desktop gate. It is **not** proof of operator-visible
behavior: per [`AGENTS.md`](../AGENTS.md), that needs a real camera, a real host phone,
and a real PC.

What is actually checked today: the decoder against a committed synthetic HEVC stream,
the Annex-B splitter, the relay's receive buffer and discovery filtering, the ABI record
layout from both sides, and the feed pipeline rendering real decoded pictures — black and
white landing where limited range says they should, chroma moving hue the way BT.709
says, mirroring reflecting the picture, raster changes rebuilding cleanly, and a frame
surviving decode, grade, and PNG encode. The pipeline tests run on whatever Vulkan device
is present, including a software one; they skip rather than fail where there is none.

What is not checked: nothing has been run against a live host or a camera, and the Swift
facade has no Swift toolchain in the authoring environment, so its own tests and the
Rust tests that call it are written but unrun. The grade-matches-the-core comparison in
`crates/opc-render/tests/lut_grade.rs` is the one to run first on a machine with Swift.
