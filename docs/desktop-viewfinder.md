# The desktop viewfinder

`opc-monitor` is the laptop as a viewfinder: a window on the camera itself, not on a
phone's shared feed. That is the difference between it and `opc-watcher`, and it is the
whole reason the crate exists.

```
opc-monitor view [--camera HOST:PORT] [--look NAME | --lut FILE] [--model ID]
                 [--still PATH]
opc-monitor keys
```

Join the camera's Wi-Fi first. Bluetooth pairing will do that from here once the platform
BLE transport lands; the state machine behind it is already in `opc-camera`.

## What it looks like

The picture fills the window, keeping its proportions — a 16:9 feed in a window the
operator dragged square gets bars, not narrow faces. Framing is what a viewfinder is for.

The chrome is one strip along the top, one along the bottom, and nothing in the middle
unless something is wrong:

- **Top left** — ISO, shutter, EV, white balance, zoom.
- **Top right** — a red lamp and the running time, while the body is rolling.
- **Bottom left** — the frame rate the body is shooting, the rate actually reaching the
  screen, battery, storage, and which assists are on.
- **Middle** — only a phase message (`WAITING FOR LIVE VIEW`, `APPROVE ON THE CAMERA`,
  `RECOVERING FEED`), or a countdown, or a failure.

`H` hides all of it.

## Keys

| | | | |
| --- | --- | --- | --- |
| `Space` | start recording | `T` | 3-second countdown, or cancel it |
| `R` | stop recording | `S` | write a still |
| Arrows | pan and tilt | `C` | recentre the gimbal |
| `+` / `-` | zoom in and out | `F` | flip to selfie and back |
| `0` | back to wide | `Esc` | close |
| Drag | track what you drew around | `X` | stop tracking |
| `[` / `]` | step resolution / frame rate | `H` | hide the chrome |
| `Z` | zebra | `P` | peaking |
| `L` | colour cube | `M` | mirror |

Two arrows at once pan diagonally, and two opposite arrows rest the stick — the gimbal
takes one position, not a stream of presses, so the held directions are added up and sent
as one. The stick is re-sent every 200 ms while held, which is a keepalive rather than
the thing that makes it move, and it rests the moment the key comes up.

`[` and `]` walk the body's **own** list of format pairs rather than a ladder this shell
invented. Resolution and frame rate are not independent — a body that shoots 4K may only
offer 24, 25 and 30 there while 1080p goes to 120 — so stepping resolution keeps the
frame rate when both shoot it and falls to that resolution's first when they do not. A
format the body did not list steps nowhere: guessing at a neighbour would change the shot
to something nobody asked for.

## How it is put together

```
opc-monitor (bin)
├── link.rs   the datalink on its own thread
├── view.rs   winit: events in, intents out
└── shell.rs  (in the library) every decision
```

`Shell` holds the whole operator-facing behaviour and has no window, no GPU and no camera
in it. Presses, drags and clock ticks go in; `Intent::Send`, `Intent::Still` and
`Intent::Quit` come out. That is what makes the viewfinder testable: 26 tests drive it
with a fake clock, and five more drive it together with the renderer on a software Vulkan
device, because chrome that is decided correctly and then never reaches the glass is a
failure neither side's own tests would catch.

`link.rs` is a thread because the datalink has a 40 Hz ACK pump to keep and a 5 ms read
timeout to sit on, and the window has a swapchain to feed. Neither can wait for the
other. Two channels, and nothing shared but the messages.

## Things that are decided here, not guessed

- **Tracking maps through the letterbox.** A drag is normalised against exactly the
  rectangle the blit drew into, and mirroring is undone before the box reaches the
  camera: the operator points at what they see, and the camera is told where that is on
  its own sensor. A drag smaller than 2% of a side is a click, and a click sends nothing —
  clearing what the camera is following by accident is worse than doing nothing.
- **A committed box stops being drawn after 1.5 seconds.** The camera does not report
  where the subject moved to, so a box left on screen would stop being where the subject
  is, and the operator would believe it.
- **The zoom follows the body.** Somebody may have turned the ring; the next `+` steps
  from where the lens actually is.
- **Presented frames drive the watchdog**, not arrived ones. A decoder quietly producing
  nothing looks exactly like a healthy feed otherwise — the black-picture-with-live-HUD
  failure this whole port has been written around.
- **A backlog skips to a keyframe**, never drops predicted pictures. Only the first coded
  slice of an access unit is read to decide that; the parameter sets ahead of it say
  nothing about whether it refreshes.

## What has not been run

No camera, and no window. `link.rs` and `view.rs` reach the Swift core, so they compile
only where there is one to link — `just desktop-check-gated` type-checks them anywhere by
forcing the flag under `cargo check`, but type-checking is not running. The first things
to try on a real machine, in order:

1. `opc-monitor view` on the camera's Wi-Fi — does a picture arrive at all.
2. `Space`, then `R` — does the body roll and stop.
3. The arrows — does the gimbal move and, more importantly, does it **stop**.
4. A drag — does the camera follow the thing that was drawn around, not near it.
