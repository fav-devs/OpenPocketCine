# OpenPocketCine desktop watcher

A PC second screen for a feed an iPhone is already hosting. Windows first; macOS and
Linux build from the same sources.

Full notes: [`docs/DESKTOP.md`](../../docs/DESKTOP.md).

## Quick start

```sh
just desktop-core                       # build the Swift core as a shared library
cd Apps/Desktop
cargo run -p opc-watcher -- list
cargo run -p opc-watcher -- join "Studio iPhone" --seconds 30 --dump feed.h265
```

Join the camera's Wi-Fi first, and turn Sharing on in the host's Operator Setup. Hosts
are advertised only on that network — there is no peer-to-peer discovery.

`feed.h265` is a plain Annex-B elementary stream, so `ffplay feed.h265` works.

## Layout

| Crate | Owns |
| --- | --- |
| `opc-core-sys` | Raw FFI to the Swift facade, plus the `#[repr(C)]` layout guards |
| `opc-relay` | Receive buffer, mDNS discovery, TCP transport, the join state machine |
| `opc-watcher` | The command-line shell |

No relay decision is made in Rust. Framing, payload limits, join rules, the retry
ladder, the delivery-delay guard, and every JSON shape come from `OpenPocketViewCore`.

## Status

Milestone 1: discovery, join, telemetry, and picture ingest. There is no decoder and
nothing on screen yet, and none of it has been run against a live host.
