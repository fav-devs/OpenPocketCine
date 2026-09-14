# Desktop operator UI handoff

## Goal

Turn `opc-monitor` from a keyboard-driven technical viewfinder into a polished,
mouse-and-touch-first desktop control surface for a directly paired Osmo Pocket. Keep
the live UDP pump, decoder, and renderer independent of UI work.

The intended visual direction is DJI Black: restrained translucent dark plates, clear
red record state, large touch targets, and a clean image centre. This is a desktop
operator surface, not a phone UI scaled up.

## Current working state

- Windows BLE pairing completes, reads the camera SoftAP credentials, creates a manual
  Windows WLAN profile, joins it, waits for a `192.168.2.x` DHCP address, then launches
  the viewfinder in a fresh process. A fresh process is required because winit/eframe
  permits one event loop per process on Windows.
- Direct UDP handshake, telemetry, video ingest, AVC/HEVC codec selection, FFmpeg
  decoding, and Vulkan display all work against a physical Pocket 3.
- This Pocket 3 sends AVC despite the desktop's original HEVC assumption. The first
  Annex-B units start `67` (SPS), `65` (IDR), and `61` (P-slice); decoder selection is
  now content-driven in `view.rs`.
- The first on-screen rail is in tree: `- ZOOM`, `+ ZOOM`, `REC`/`STOP`, and
  `RECENTER`. It is intentionally only a functional first step, not the finished UI.

## Known live-path defects

1. The feed can freeze after roughly 10–20 seconds, then recover. Logs showed watchdog
   escalation and Windows UDP errors `997` and `10022`. `session.rs` now treats these
   as empty polls and turns desktop `FullRejoin` into a fresh UDP sequence while the
   Wi-Fi path is still alive. This needs a physical retest.
2. There is a bottom-edge pixel corruption artifact. `OwnedPicture` already compacts
   FFmpeg plane stride correctly, so investigate the Vulkan plane upload/presentation
   path before changing decoder row packing.

## Architecture and files

| File | Responsibility |
| --- | --- |
| `Apps/Desktop/crates/opc-monitor/src/view.rs` | winit events, decoder selection, renderer presentation, carries shell intents to `Link` |
| `Apps/Desktop/crates/opc-monitor/src/shell.rs` | Pure operator state machine: input, hit testing, tracking, controls, HUD invalidation |
| `Apps/Desktop/crates/opc-monitor/src/link.rs` | Camera worker thread; keep it UI-free |
| `Apps/Desktop/crates/opc-ui/src/hud.rs` | CPU-rasterised RGBA chrome composited over the feed |
| `Apps/Desktop/crates/opc-ui/src/canvas.rs` | Testable overlay drawing primitives |
| `Apps/Desktop/crates/opc-ui/src/controls.rs` | Keyboard actions, zoom state, gimbal stick mapping |
| `Apps/Desktop/crates/opc-camera/src/command.rs` | Existing typed camera commands; do not invent wire bytes in UI code |

The hard split is: `Shell` decides every user-visible interaction and emits
`Intent::Send(Command)`; `view.rs` only turns native events into shell input and sends
intents through `Link`. Keep this split. It makes the controls unit-testable without a
window or camera.

## Required reading

- `AGENTS.md`
- `docs/desktop-viewfinder.md`
- `docs/desktop-camera-link.md`
- `docs/live-session.md`
- `docs/feed-watchdog.md`
- `docs/PARITY.md`
- `docs/UX.md`

The direct session is live now, so desktop is operator-visible work. Record any desktop
exception in `docs/PARITY.md`; do not silently drift from mobile control semantics.

## UI plan

### Pass 1: complete primary controls

1. Replace the text-only bottom rail with button plates and icons, preserving its
   current typed commands.
2. Add a right-side gimbal pad. Pointer-down starts an axis command, movement maps to
   `Command::GimbalStick`, and pointer-up/cancel sends the centred stick immediately.
   It must be a hold control, never a tap-to-nudge approximation.
3. Add a zoom rail with labelled physical stops when the camera reports them; until
   then retain safe 0.5x steps. Add a prominent 1x reset.
4. Add a still button and selfie flip/recenter actions.
5. Controls win hit-testing. A drag that begins on the unobstructed fitted image starts
   tracking; a drag/tap that begins in a control never sends tracking.

### Pass 2: compact chrome and sheets

1. Top leading camera-truth chips: ISO, shutter, EV, WB, zoom.
2. Top trailing recording tally and connection state.
3. Bottom sheets, not permanent screen clutter, for format, exposure, white balance,
   assists, and display options.
4. Large 44 px minimum pointer targets; keyboard shortcuts remain visible as tooltips.
5. Respect `H`: clean view hides all operator chrome, not only labels.

### Pass 3: input and resilience

1. Route mouse and touchscreen through the same hit-test/action code.
2. Add controller mapping only after pointer controls pass physical testing.
3. Disable controls during failed/recovering links and show an explicit state, never a
   fake optimistic camera state.
4. Add tests for every hit rectangle, gesture priority, gimbal release-to-rest, record
   behaviour, and resize layout.

## Important protocol safety

- Do not write raw DUML bytes from UI code; use `opc_camera::Command`.
- All commands serialize through the camera worker/ACK pump.
- Live enable (`0x09/0xa8`) remains enable-once; only the watchdog may resend it.
- Gimbal must send a rest command on mouse/touch cancel, focus loss, and window close.
- Do not make controls depend on incoming telemetry being perfect. Camera truth updates
  chips, but actions should not freeze the renderer or block the ACK worker.

## Verification

1. `cargo test -p opc-camera --lib` currently passes 50 tests.
2. Use `build-swift-core.bat` on this Windows workstation; it configures Swift, FFmpeg,
   Vulkan, builds `opc-monitor`, and stages all required DLLs. The debug executable is
   `Apps/Desktop/target/debug/opc-monitor.exe`.
3. A generic `cargo check` from a normal shell fails unless `FFMPEG_DIR` is configured;
   that is an environment limitation, not necessarily a source failure.
4. Physically test with the Pocket: record/stop, zoom, gimbal start and immediate stop,
   touch tracking, resize, long-run feed stability, and bottom-edge image integrity.
5. Never log Wi-Fi passwords or raw BLE pairing payloads containing them.
