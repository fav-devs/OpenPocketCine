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

The chrome is a DJI Mimo replica laid out for a landscape laptop, rendered by Slint
(`opc-chrome`, Outfit type, Tabler icons) into a transparent overlay the shell composites
over the picture:

- **Top bar** — menu, gimbal follow `ON`/`OFF`, the format chip (`1080P·60`), the
  exposure mode chip (`AUTO`/`M`), the link state in the middle with a red `REC` badge
  and running time while the body is rolling, and exit at the far right.
- **Zoom ruler** — a dotted ruler under the top bar that slides beneath a fixed ring;
  drag it to zoom, the label under it is the truth.
- **Exposure plate** (left) — shutter, `ISO`, `EV` and `WB` readouts. A field the camera
  has not reported is absent rather than guessed.
- **Status plate** (right) — Wi-Fi, battery (red at 20 %), card time left, and the rate
  actually reaching the screen.
- **Bottom bar** — gallery, flip and orientation next to the joystick on the left; the
  record button in the middle (a red disc, a red square while rolling, white in photo
  mode); `CTR`, `FOLLOW`, `STILL` and fullscreen on the right; and the mode strip
  (`TIMELAPSE · SLOWMOTION · LOW-LIGHT · VIDEO · PHOTO · PANO · LIVESTREAM`) with the
  active mode in Mimo yellow and Pano / Livestream greyed out.
- **Middle** — only a phase message (`WAITING FOR LIVE VIEW`, `APPROVE ON THE CAMERA`,
  `RECOVERING FEED`), the take countdown, or a failure.

Three sheets open over the picture and close on `Esc`, the `×`, or a tap outside:

- **Format** (the format chip) — the resolutions and frame rates the body listed, and
  nothing else. Picking a size keeps the rate when that size offers it.
- **Exposure** (`AUTO`/`M` chip, or `E`) — `Auto`/`Manual`, then ISO and shutter for
  manual, ISO max and EV for auto. The rows the mode does not use are drawn greyed, the
  way Mimo shows them.
- **Settings** (`⋮`, or `Tab`) — three tabs. **Camera:** focus mode, white balance
  presets, colour profile (from the body's own list), field of view, gimbal mode and
  speed. **Audio:** channel and vocal boost; wind and directional audio are shown greyed
  because they live in a DSP blob the desktop cannot read yet. **Assist:** thirds grid,
  overexposure alert (zebra), focus peaking, LUT, mirror, and the timecode in the top bar.

A chip lights up when the camera confirms the value, not when it is tapped; a setting
the body never reports (audio channel, field of view, gimbal speed) is kept as last
commanded.

## The library

The gallery button, or `G`, opens the camera's card over the picture: a grid of
thumbnails with `ALL · VIDEOS · PHOTOS · FAVORITES` tabs, a sort chip
(`Newest · Oldest · Name · Rating`) and Refresh. Tapping a clip fills the bar along the
bottom with its name, duration, size and resolution, and the actions:

- **PLAY** fetches the 720p `.LRF` proxy to the cache and opens the player on it; the
  original is the fallback when there is no proxy. **VIEW** does the same for a still.
- **DOWNLOAD** fetches the original into the library folder, with a progress bar.
- **STAR** flips the favourite locally and tells the camera when the record carries a
  handle to tell it with.
- **DELETE** arms on the first tap and sends on the second. Only a handle the core's
  base + step fit vouched for ever goes out; a shared or unfitted handle greys the
  button. A delete is irreversible.

Listing follows the phones' sequence and the Osmosis notes for the bodies that need
them: enter playback (`0x02/0x0c`, three tries), fall through to the Pocket 3's
`0x01/0x01` entry at 20 Hz when the body refuses, wait 1.7 s for the store to mount,
then list the internal store, the trigger, and the card, and collect until the camera
goes quiet. Older pages walk the oldest video handle down while playback holds; a body
that never enters still lists its newest page. The catalogue itself is decoded by the
Swift core through the facade — no manifest byte is read in Rust — and the last list
and the local stars are kept per camera under the platform cache folder, so the
library opens instantly next time.

Closing the library exits playback until the body's playback bit clears and then asks
for live view again, the same loop the phones run.

## The player

The proxy plays through the feed pipeline, so the LUT, zebra, peaking and mirror keys
work on it exactly as on live view and the chrome says which are on. `Space` pauses,
the bar scrubs, `Esc` goes back to the library. A still is converted to the same 4:2:0
path, so it is graded too. Playback is from the file on disk, never streamed from
`/v2`: the camera parks `moov` at the end and serves no extension, which no player
copes with.

Every button carries its key hint in small type, so a keyboard operator learns the
bindings from the screen. When the window is wider than the picture, the two plates park
in the black gutters and leave the shot clean.

Each cluster is a self-contained Slint component with its own anchor, so a later edit
mode can move them without touching their internals.

`H` hides all of it. To look at the chrome without a camera or a window:

```
cargo run -p opc-chrome --example snapshot -- <dir>
```

writes PNGs of the finding, live, recording, failed and wide-window states.

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
| `Tab` | settings | `E` | exposure sheet |
| `G` | the library | `R` | refresh the list (library) |
| `F11` | fullscreen (button) | `Esc` | close a sheet first |
| `Z` | zebra | `P` | peaking |
| `L` | colour cube | `M` | mirror |

Two arrows at once pan diagonally, and two opposite arrows rest the stick — the gimbal
takes one position, not a stream of presses, so the held directions are added up and sent
as one. The stick is re-sent every 200 ms while held, which is a keepalive rather than
the thing that makes it move, and it rests the moment the key comes up.

## Pointer controls

The bars are a desktop operator surface, not scaled-up phone chrome. The record button,
`STILL`, flip, `CTR` and the mode strip carry the existing typed commands. The gimbal
follow chip and `FOLLOW` button send the same SET frames as the mobile gimbal sheet
(`Follow` → `Tilt locked` → `FPV`). The format chip opens the format sheet. The joystick is a **hold** control: pressing or moving it
sends the matching stick axes and release, cancellation, focus loss, and window close
send a centred stick immediately. Controls are at least 44 px, and they are greyed and
disabled while the link is recovering or failed.

A finger drags a tracking box on the unobstructed fitted image, exactly as the mouse
does. A press that starts in a control stays a control — it can never become tracking.
Keyboard shortcuts remain available.

Touch is handled explicitly rather than left to the system: once winit registers a window
for touch, Windows stops synthesising mouse clicks from taps, so without this a finger on
the picture would do nothing at all.

One finger owns the box. A second finger, or a palm steadying the laptop, is ignored
until the first lifts — but a finger that lands on a letterbox bar never claims the drag
it did not start, so it cannot lock out the next one that does land on the shot. A
cancelled touch — the system claiming the gesture, a palm rejected — abandons the box
rather than committing it: pointing the camera at whatever a finger happened to be over
is worse than not tracking at all.

The rule lives in `Shell::touch`, not in the window, so all of that is pinned by tests on
a machine with no touchscreen. What is **not** tested is whether the events arrive at all;
that needs a real touchscreen.

## Formats

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
5. The same drag with a finger, on a touchscreen — does the event arrive at all.
