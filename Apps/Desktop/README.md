# OpenPocketCine desktop watcher

A PC second screen for a feed an iPhone is already hosting. Windows first; macOS and
Linux build from the same sources.

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
cargo run -p opc-watcher -- list
cargo run -p opc-watcher -- join "Studio iPhone" --seconds 30 --dump feed.h265 --still first.png
cargo run -p opc-watcher -- decode feed.h265 --out stills --look Contrast
```

Join the camera's Wi-Fi first, and turn Sharing on in the host's Operator Setup. Hosts
are advertised only on that network — there is no peer-to-peer discovery.

`feed.h265` is a plain Annex-B elementary stream, so `ffplay feed.h265` works.

## Layout

| Crate | Owns |
| --- | --- |
| `opc-core-sys` | Raw FFI to the Swift facade, plus the `#[repr(C)]` layout guards |
| `opc-relay` | Receive buffer, mDNS discovery, TCP transport, the join state machine |
| `opc-decode` | HEVC and AVC decoding over libavcodec |
| `opc-render` | The Vulkan feed pipeline, the cube upload, and PNG stills |
| `opc-watcher` | The command-line shell |

No relay decision is made in Rust, and no `.cube` is parsed here. Framing, payload
limits, join rules, the retry ladder, the delivery-delay guard, every JSON shape, and
the colour cube come from `OpenPocketViewCore`. The grade runs the Android shell's own
shaders, compiled from where they live.

## Status

Discovery, join, telemetry, picture ingest, decoding, and the graded feed pipeline —
output as PNG stills. There is no window yet, and none of it has been run against a live
host.
