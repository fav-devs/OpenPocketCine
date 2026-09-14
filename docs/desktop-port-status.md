# The PC port: what exists, and where to pick it up

Written as a handover. If you are coming to the desktop shell cold, read this first and
then the doc for whichever piece you are touching.

- [`DESKTOP.md`](DESKTOP.md) — how the build is structured and why Rust is in it
- [`desktop-camera-link.md`](desktop-camera-link.md) — the datalink, the ACK pump, pairing
- [`desktop-viewfinder.md`](desktop-viewfinder.md) — the window, the keys, the chrome

## What it does today

`opc-monitor view` opens a window on the camera itself — the laptop as a viewfinder, not
a second screen for a phone. The picture fills the window, a thin strip of chrome top and
bottom says what the body is set to, and the keyboard drives it.

| | Key | Where it lives |
| --- | --- | --- |
| Gimbal | Arrows, `C` recentre, `F` flip | `opc-ui/controls.rs`, `Stick` |
| Zoom | `+` `-` `0` step the body's own stops; D-Log2 hop | `opc-monitor/zoom.rs` |
| Record | `Space`, `R` | `opc-ui/controls.rs` |
| Timer | `T` — 3 seconds, cancellable | `opc-monitor/shell.rs`, `Countdown` |
| Tracking | Mouse or one-finger drag, polled at 0.5 s until lock or idle, `X` to clear; a click is tap-to-focus (Mimo's four-write burst) | `opc-ui/tracking.rs`, `Shell::touch`, `opc-camera/tracking.rs` |
| Frame rate, resolution | `[` `]` | `opc-ui/format.rs` |
| Assists | `A` opens the phones' toolbar (long press for options); `Z` `P` `L` `M`, `H` hides the chrome | `opc-monitor/assists.rs`, `shell.rs`, `Toggles` |
| Sheets | `Tab` settings, `E` exposure, the format chip | `opc-monitor/sheets.rs` |
| SET mailbox | Latest-wins per opcode, 300 ms retransmit, 2 s settle, FORMAT pin | `opc-camera/mailbox.rs`, the core's `CameraSetMailbox` |
| Library | `G`, then the grid; `Space` and `Esc` in the player | `opc-monitor/library.rs`, `media.rs`, `opc-media` |

The chrome is a Slint Mimo replica with every button live (`opc-chrome`); the media
library and player are `opc-media` (paging, HTTP, cache) driven by `opc-monitor/media.rs`.

`opc-watcher` is the other program and the older one: a PC second screen for a feed an
iPhone is already hosting. It shares the decoder, the renderer and the core facade, and
nothing else.

## The shape

```
OpenPocketViewCore (Swift, Foundation-only)
        ↓  @_cdecl
OpenPocketCineDesktopFacade  +  COpcDesktop (fixed-layout C records)
        ↓
opc-core-sys  →  opc-camera  ┐
              →  opc-relay   ├→  opc-monitor  (the viewfinder)
opc-decode    →  opc-render  ┘   opc-watcher  (the second screen)
              →  opc-ui
```

| Crate | Lines | Tests written | Owns |
| --- | --- | --- | --- |
| `opc-core-sys` | 810 | 8 | Raw FFI and the `#[repr(C)]` layout guards |
| `opc-camera` | 4089 | 85 | Commands, DUML transport, the ACK pump, pairing, the watchdog |
| `opc-relay` | 1592 | 26 | Receive buffer, mDNS discovery, TCP, the join state machine |
| `opc-decode` | 659 | 15 | HEVC and AVC over libavcodec, Annex-B replay |
| `opc-render` | 4324 | 40 | The Vulkan feed pipeline, the swapchain, the cube, the chrome composite |
| `opc-ui` | 1618 | 61 | Bitmap font, CPU canvas, key map, format ladder, tracking arithmetic |
| `opc-monitor` | 1987 | 41 | The viewfinder: the shell, the window, the camera thread |
| `opc-watcher` | 947 | — | The command-line shell and the second-screen window |

Plus 1,584 lines of Swift facade and a 328-line shared header.

276 tests are written; **218 run here**. The other 58 are gated behind `opc_core_linked`
and need a Swift toolchain — see *What has never been run*.

**No protocol decision is made in Rust.** Framing, opcodes, payload limits, join rules,
the retry ladder, focus fitting, the colour cube and every JSON shape come from the core
through the facade. The Rust side owns sockets, the clock, the GPU and the typing.

## The one pattern worth knowing

Every piece is a **pure state machine plus a thin platform layer**. `Sequencer`,
`Pairing`, `JoinPolicy`, `FeedHealth`, `Controls`, `Stick`, `Fit`, `Drag`, `Canvas`,
`Hud` and `Shell` all decide things with a clock passed in and no I/O. The socket, the
window and the GPU are the parts that cannot be tested here, and they are kept as small
as they will go.

That is why `Shell::touch` arbitrates between fingers rather than `view.rs` doing it: the
window cannot be run in this environment, so anything decided there is unverifiable by
construction. If you add behaviour, put it on the pure side of that line.

## Decisions that are easy to reverse by accident

These are all encoded in tests. If a test with a long name starts failing, it is probably
one of these.

**The picture is letterboxed, never stretched.** A 16:9 feed in a window dragged square
gets bars. `opc_render::letterbox` is public because `Shell::fit` must map a click back
through exactly the rectangle the blit drew into — one formula, asserted against the
pixels it produces. Two implementations would silently drift and tracking would land
*near* the subject.

**The chrome gets its own render pass.** A HUD through the colour cube tells the operator
the wrong thing about exposure, so `overlay.frag` composites after `blit.frag`. Opacity
fades the chrome only; multiplying the composite would dim the picture with it.

**Live view is enable-once.** `0x09/0xa8` is sent once on connect. Every later enable
belongs to the watchdog. This is in the core and the session honours it.

**The ACK pump runs at 40 Hz with three cursor groups**, and group 0 is the shell's —
`AckWindows::advancing()` handles only 1 and 2. Getting this wrong produces a session
that connects, shows a live HUD, and never shows a picture. `live-session.md` is required
reading before touching it.

**The watchdog counts presented frames, not arrived ones.** A decoder quietly producing
nothing looks exactly like a healthy feed otherwise.

**A backlog skips to a keyframe, never thins.** Predicted pictures need the ones before
them.

**A tap or click sends nothing.** Clearing what the camera is following by accident is
worse than doing nothing. A cancelled touch abandons its box rather than committing it.

**A committed tracking box stops being drawn after 1.5 s.** The camera never reports
where the subject moved to, so a box left on screen would stop being where the subject is
and the operator would believe it.

**`[` and `]` walk the body's own format list.** A format the body did not list steps
nowhere rather than guessing at a neighbour.

## What is actually verified

218 tests pass, on a machine with no camera, no phone, no display and no Swift toolchain.
`just desktop-check` is the gate.

- **Pure logic** — the sequencer, pairing, the join policy, feed health, the key map, the
  stick, the format ladder, the tracking arithmetic, the canvas and the HUD, all driven
  with a fake clock.
- **The fake camera** (`opc-camera/tests/session.rs`) — a loopback UDP peer that answers
  handshakes, streams video, replies to commands, records what it was sent, and can
  `freeze()`. This is what makes the black-picture class of failure catchable.
- **The feed pipeline on a software Vulkan device** (lavapipe) — limited-range black and
  white landing where they should, chroma moving hue the way BT.709 says, zebra striping
  rather than flooding, peaking finding an edge in the right column, the letterbox to the
  row, and chrome compositing over the picture without tinting it.
- **The swapchain through `VK_EXT_headless_surface`** — acquire, submit, present,
  recreate on resize, a minimised window, and past the frames-in-flight count. Same code
  a real window drives.
- **The decoder against a committed synthetic HEVC stream.**
- **ABI record layout asserted from both sides** of the facade.

## What has never been run

Be honest about this when planning; most of it is one machine away.

1. **No camera.** Nothing in this port has met a Pocket. The fake camera answers the way
   the captures say a real one does, which is not the same thing.
2. **No window.** The swapchain is exercised headless, so winit, the platform surface
   extensions and both event loops (`opc-monitor/src/view.rs`,
   `opc-watcher/src/watch.rs`) are unrun.
3. **No touchscreen.** `Shell::touch` is tested; whether the events arrive is not.
4. **The Swift facade has never been compiled.** There is no Swift toolchain in the
   authoring environment — `download.swift.org` is blocked by policy here, which is not
   retryable. 1,584 lines, plus **58 tests** gated behind `opc_core_linked` that have
   never executed: 24 in `opc-camera/tests/wire.rs`, 11 in `opc-camera/tests/session.rs`,
   17 in `opc-relay/tests/core_round_trip.rs`, 6 in `opc-render/tests/lut_grade.rs`.
   `just desktop-check-gated` forces the flag under `cargo check`, so all of it is known
   to *compile* — that is the most that can be claimed from here.
5. **GitHub Actions is disabled on this repository.** The `desktop` CI job is committed
   and would do all of the above — it installs Swift, lavapipe and FFmpeg, builds the
   core and runs the whole suite. Turning Actions on (Settings → Actions → General) is
   the single highest-value thing available, because it converts item 4 from unknown to
   known without anyone owning a camera.

The first thing to run on a machine with Swift is
`Apps/Desktop/crates/opc-render/tests/lut_grade.rs` — it compares the GPU grade against
the core's own, which is the claim most likely to be quietly wrong.

Then, with a camera, in this order: does a picture arrive at all; does `Space` roll and
`R` stop; does the gimbal move and, more importantly, **stop**; does a drag track the
thing that was drawn around rather than near it.

## What is next, and why in this order

1. **The platform Bluetooth transport.** `BleTransport` is an interface with nothing
   behind it, so `Pairing` — written and tested — cannot reach a camera. Until this
   lands, the camera's Wi-Fi has to be joined by hand, which is the one part of the
   original ask that is not met. btleplug is the reason Rust is in this port at all.
2. **On-screen controls**, if wanted. The architecture takes it — `Hud` already computes
   its layout and would need to hand back the rectangles it drew so `Shell` can hit-test
   a tap before falling through to a tracking drag. Deliberately left until someone has
   held the laptop, because where the controls should sit is not guessable.
3. **The scopes** (milestone 2d). False colour is in: the core bakes the paint and
   weight lattices (`FalseColorCube`) and the facade hands them over as cube handles.
   The scopes need the decoded picture read back on the CPU each frame and a place to
   draw the plates.

## Building it

```sh
sudo apt install libavcodec-dev libavutil-dev libswscale-dev libvulkan-dev glslang-tools
just desktop-core      # build the Swift core as a shared library
just desktop-build
just desktop-check     # format, lint, test — the gate
just desktop-check-gated   # type-check everything behind opc_core_linked, anywhere
```

Without a Swift toolchain the workspace still builds and 218 tests still run; the
viewfinder binary builds too and says what is missing when asked to open a camera.
