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
  drag it to zoom, the label under it is the truth. The body's own chip stops are marked
  on it (Pocket 4 Pro 1× / 3× / 6× / 12×; Pocket 4 and Pocket 3 1× / 2× / 4×, 2× at most
  for a Pocket 3 in 4K; 1× only in slow motion, timelapse and low light; Nano 1×), and
  `+` / `-` step between them. A body with only 1× greys the ruler. D-Log2 rejects every
  zoom, so a zoom off 1× first hops the colour to D-Log, waits for the body to report
  it, then zooms; parking back at 1× puts D-Log2 back. Rolling in D-Log2 locks the zoom,
  and the top bar says so.
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
- **Settings** (`⋮`, or `Tab`) — three tabs. **Camera:** focus mode, focus-track mode (Default / Product Showcase / Subject Lock / Registered Priority), white balance
  presets, colour profile (from the body's own list), field of view, gimbal mode,
  speed and **ramp** (Off / Soft / Medium, the phones' first-order ease on the stick,
  applied to the arrow keys and the on-screen pad alike). **Audio:** channel, vocal
  boost, wind noise reduction and directional audio (All / Front / Front+back). The
  last two live in one DSP blob (`@2` of the `0x02/0xA0` GET reply): the tab reads
  the blob when it opens, the rows stay greyed until it has answered, and a pick
  sends the body's own 26 bytes back with `@2` patched (`0x02/0x9F`) followed by a
  fresh read, so the chips show what took rather than what was asked. **Assist:** thirds grid, overexposure alert (zebra),
  focus peaking, the **LUT** row (Off, the core's official Rec.709 cubes, then every
  `.cube` the operator dropped into the LUT folder the row names), mirror, the timecode
  in the top bar, and the `T` countdown length (3, 5 or 10 s).

- **Assist toolbar** (`ASSIST` in the top bar, or `A`) — the phones' fifteen-tool
  strip under the top bar: `LUT PEAK FALSE | ZEBRA WAVE PARADE | HISTO VECTOR LIGHTS
  ND | GUIDES GRID CROSS | MIRROR | AUDIO`. A tap flips the tool; a long press or a
  right click opens its options as a sheet. **False colour** paints the core's
  CineStop, EL Zone, IRE or Limits lattices for the body's colour mode and ISO (the
  same two cubes the phones sample), with a reference key along the bottom of the
  picture. **Peaking** has the phones' sensitivity (Low / Med / High) and stroke
  colour. **Zebra** has highlight and midtone bands, each with its level (in IRE, or
  read as 0–255 codes on the feed, which the core converts) and stripe colour; the
  thresholds land on the feed's own axis per colour mode. **Grid** draws thirds, the
  phi grid and dotted diagonals in any mix; **Guides** draws the Film or Social
  aspect frames (several at once) with an optional mask outside them; **Cross** is
  the centre crosshair. The **scopes** are movable plates over the picture, sized as
  on the phones, read from the decoded picture on the CPU at about 15 Hz: **WAVE**
  (luma or RGB overlay, with the clip / crush / middle-grey guides), **PARADE** (RGB
  or YRGB lanes), **HISTO** (RGB fills and the luma line on the waveform's axis,
  the clip zone at 95), **VECTOR** (chroma trace on the 75% graticule with the 123°
  skin line; trace zoom 1× / 2× / 4×), **LIGHTS** (three lamps, clip and crush per
  channel, with the crush / clip compensation), **ND** (the suggested screw-on
  filter in stops, factor or density) and **AUDIO** (the body's own meters with
  peak hold). The axis each plate plots on — where 0, 100 and 18% grey fall for the
  body's colour mode and ISO — is the core's `ScopeDisplayScale`, and the lights and
  the ND reading are the core's from the histograms; the desktop only samples and
  draws. Drag a plate anywhere to park it somewhere else.

A chip lights up when the camera confirms the value, not when it is tapped; a setting
the body never reports (audio channel, field of view, gimbal speed) is kept as last
commanded.

## Programmed moves

`K` opens the Moves sheet: **Set here** captures the body's live pose into A, B or an
optional C (the sheet shows the live pan and tilt as the camera reports them), the
`A → B` and `B → C` rows pick each leg's duration, and **Start** counts 3-2-1, closes
the sheet and runs the take with a readout in the top bar. **Stop**, any arrow key,
or the pad cancels it with a native stop.

The engine is the core's `GimbalMoveEngine` transcribed (`opc-monitor/moves.rs`) minus
smoothing at B and pause / resume: the approach to A in steps under 120° at 120°/s,
a two-second hold, one native timed target (`0x04/0x14`) per exact leg, legs over 180°
or 25.5 s split into native parts along the reachable arc, every target checked
against attitude no older than 300 ms so the firmware can never take the route
through the missing sector, and a final check that the camera stopped within 0.15°.
A late boundary dispatch (over 40 ms, the window's draw loop being no scheduler)
stops the take. Attitude reaches the desktop as the `0x04/0x05` yaw, display tilt and
native pitch the facade now reads out with every status. Timing and positional
accuracy are unqualified on a body; see `docs/programmed-moves.md`.

## The library

The gallery button, or `G`, opens Mimo's album over the picture: **Device** (the card)
or **Local** (only what is on this machine), `All · Photos · Videos · Favorites` pills,
a sort chip (`Newest · Oldest · Name · Rating`) and Refresh. Tiles are grouped under
day headers (`Today`, then the date), carry Mimo's download mark until the original is
on disk, the clip length, and a star for favourites; there are no file names on the
grid. Tapping a tile fills the bar along the bottom with its name, duration, size and
resolution, and the actions:

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
work on it exactly as on live view. The page is Mimo's: back, an info button that
shows the clip's name and figures, the rendition as the title (`Low-Res` for the
proxy), a download button for the original; below, the time pill, a filmstrip scrubber
of eight frames decoded from the clip with the playhead over it, the tools
(Screenshot writes the graded frame with `S`; LUT, Zebra and Peaking toggle), and
heart · pause · trash. `Space` pauses, `Esc` goes back to the library; trash arms on
the first tap and deletes on the second. A still is converted to the same 4:2:0
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
| `+` / `-` | the next / previous zoom stop | `F` | flip to selfie and back |
| `0` | back to wide | `Esc` | close |
| Drag | track what you drew around | `X` | stop tracking |
| `[` / `]` | step resolution / frame rate | `H` | hide the chrome |
| `Tab` | settings | `E` | exposure sheet |
| `G` | the library | `R` | refresh the list (library) |
| `K` | programmed moves | `A` | the assist toolbar |
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

A click (or a tap) that is not a drag is **tap-to-focus**: Mimo's four-write burst
(`0x22` spot, `0x30` region, `0x68` hint, `0x32` commit) at that point on the sensor,
mirroring undone, with a bracketed reticle and the AE spot marked at its corner for
1.5 s. A body already following something is told to stop first. The Nano takes no
tap focus, so a click on one sends nothing. A drag's box is then **polled** on the
phones' cadence (`0x02/0xA5` every 0.5 s): the box stays as long as the body says it
has the subject, moves to the subject's rectangle when the body sends one, and comes
off at the first idle after a lock or after six idle answers with no lock. The AF-C
face bracket is not here: the phones detect faces on-device, and the desktop has no
detector yet.

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
  to the stop above where the lens actually is. Which stops the body has is the core's
  answer (`CameraModel.activeZoomStops`), asked again whenever the model, format or
  shooting mode moves.
- **Every live-control SET goes through the phones' mailbox.** The core's
  `CameraSetMailbox` decides, per opcode, what may go on the wire: one generation at a
  time, latest wins (a wheel or a slider replaces its pending step rather than queuing),
  the zoom slider pipelined at 20 Hz. The datalink keeps its clock the way the phones
  do — retransmit once after 300 ms of silence, settle at 2 s, accept a late ACK for
  the open generation, drop a superseded one. A FORMAT just sent is pinned on its chip
  until the body confirms it or the settle window passes; a SET nobody answered puts
  "no answer from the camera" in the top bar.
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
