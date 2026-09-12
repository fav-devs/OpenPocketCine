# OpenPocketCine desktop shell

Two programs, Windows first; macOS and Linux build from the same sources.

- **`opc-monitor`** — the laptop as a viewfinder, talking to the camera directly:
  gimbal, zoom, recording, a timer, tracking, and the frame rate and resolution.
  [`docs/desktop-viewfinder.md`](../../docs/desktop-viewfinder.md).
- **`opc-watcher`** — a PC second screen for a feed an iPhone is already hosting.

Full notes: [`docs/DESKTOP.md`](../../docs/DESKTOP.md).

## Quick start

Needs a Swift toolchain, FFmpeg development libraries, a Vulkan loader, and a GLSL
compiler:

```sh
sudo apt install libavcodec-dev libavutil-dev libswscale-dev libvulkan-dev glslang-tools
```

```sh
just desktop-core                       # build the Swift core as a shared library
cd Apps/Desktop

# The viewfinder. Join the camera's Wi-Fi first.
cargo run -p opc-monitor -- view
cargo run -p opc-monitor -- keys         # every key it listens for

# The second screen.
cargo run -p opc-watcher -- list
cargo run -p opc-watcher -- watch "Studio iPhone" --look Contrast
cargo run -p opc-watcher -- join "Studio iPhone" --seconds 30 --dump feed.h265 --still first.png
cargo run -p opc-watcher -- decode feed.h265 --out stills --look Contrast
```

`watch` opens a window on a shared feed. Keys: **L** cube, **Z** zebra, **P** peaking,
**M** mirror, **S** still, **Esc** quit. `opc-monitor keys` lists the viewfinder's, which
are a superset.

For `opc-watcher`, join the camera's Wi-Fi first and turn Sharing on in the host's
Operator Setup. Hosts are advertised only on that network — there is no peer-to-peer
discovery.

`feed.h265` is a plain Annex-B elementary stream, so `ffplay feed.h265` works.

## Layout

| Crate | Owns |
| --- | --- |
| `opc-core-sys` | Raw FFI to the Swift facade, plus the `#[repr(C)]` layout guards |
| `opc-relay` | Receive buffer, mDNS discovery, TCP transport, the join state machine |
| `opc-decode` | HEVC and AVC decoding over libavcodec |
| `opc-camera` | Camera commands, the DUML datalink, the ACK pump, pairing, and the watchdog |
| `opc-render` | The Vulkan feed pipeline, the swapchain, the cube upload, the chrome composite, and PNG stills |
| `opc-ui` | The viewfinder's chrome: a bitmap font, a CPU canvas, the key map, the format ladder, the tracking arithmetic |
| `opc-monitor` | The viewfinder |
| `opc-watcher` | The command-line shell and the second-screen window |

No relay decision is made in Rust, and no `.cube` is parsed here. Framing, payload
limits, join rules, the retry ladder, the delivery-delay guard, every JSON shape, and
the colour cube come from `OpenPocketViewCore`. The grade runs the Android shell's own
shaders, compiled from where they live.

## Status

Discovery, join, telemetry, picture ingest, decoding, the graded feed pipeline with zebra
and peaking, the camera datalink, and both windows. The swapchain is tested through a
headless surface, so no real window has been opened yet, and none of it has been run
against a live host or a camera. Bluetooth pairing has its state machine but no platform
transport behind it yet, so the Wi-Fi has to be joined by hand.
