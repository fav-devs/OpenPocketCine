//! Connection and pairing screen (eframe/egui).
//!
//! Runs before the viewfinder. Scans BLE, drives the pairing handshake, and returns
//! the Wi-Fi credentials so main can join the camera network and open the viewfinder.

use std::io::Write;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, FontId, Layout, RichText, Stroke, Vec2};
use opc_camera::{
    pair_approval_ack, pair_set_pin, pair_wake_access_point, status_string, BleTransport,
    Discovered, GattMap, NotificationAssembler,
};

use crate::ble_impl::BtleplugTransport;

// ── palette ──────────────────────────────────────────────────────────────────

const BG: Color32 = Color32::from_rgb(14, 14, 14);
const SURFACE: Color32 = Color32::from_rgb(26, 26, 26);
const BORDER: Color32 = Color32::from_rgb(50, 50, 50);
const ACCENT: Color32 = Color32::from_rgb(255, 140, 0);
const TEXT: Color32 = Color32::from_rgb(230, 230, 230);
const DIM: Color32 = Color32::from_rgb(130, 130, 130);
const SUCCESS: Color32 = Color32::from_rgb(80, 200, 120);
const ERR: Color32 = Color32::from_rgb(220, 70, 70);

// ── channels ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
enum BleCmd {
    Scan,
    Pair(String),
    Cancel,
}

#[derive(Debug)]
enum BleEvent {
    Scanning,
    Found(Vec<Discovered>),
    Step(String),
    AwaitingApproval,
    Credentials { ssid: String, password: String },
    Error(String),
}

// ── public result ─────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ConnectOutcome {
    Quit,
    Connected {
        ssid: String,
        password: String,
        model_id: Option<i32>,
    },
    Skip,
}

// ── screens ──────────────────────────────────────────────────────────────────

enum Screen {
    Scanning,
    Choose(Vec<Discovered>),
    Pairing {
        label: String,
        camera: String,
    },
    AwaitingApproval {
        camera: String,
    },
    WifiReady {
        #[allow(dead_code)]
        camera: String,
        ssid: String,
        password: String,
        model_id: Option<i32>,
        show_pass: bool,
    },
    Failed(String),
}

// ── app ───────────────────────────────────────────────────────────────────────

struct ConnectApp {
    screen: Screen,
    tx: mpsc::Sender<BleCmd>,
    rx: mpsc::Receiver<BleEvent>,
    ble_available: bool,
    camera_name: String,
    model_id: Option<i32>,
    outcome: Arc<Mutex<Option<ConnectOutcome>>>,
}

impl ConnectApp {
    fn new(
        tx: mpsc::Sender<BleCmd>,
        rx: mpsc::Receiver<BleEvent>,
        ble_available: bool,
        outcome: Arc<Mutex<Option<ConnectOutcome>>>,
    ) -> Self {
        if ble_available {
            let _ = tx.send(BleCmd::Scan);
        }
        Self {
            screen: Screen::Scanning,
            tx,
            rx,
            ble_available,
            camera_name: String::new(),
            model_id: None,
            outcome,
        }
    }

    fn finish(&self, ctx: &egui::Context, o: ConnectOutcome) {
        *self.outcome.lock().unwrap() = Some(o);
        let _ = self.tx.send(BleCmd::Cancel);
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn draw_screen(&mut self, ui: &mut egui::Ui) -> Option<ConnectOutcome> {
        match &mut self.screen {
            Screen::Scanning => {
                ui.label(
                    RichText::new("Scanning for cameras\u{2026}")
                        .color(DIM)
                        .font(FontId::proportional(15.0)),
                );
                ui.add_space(16.0);
                ui.add(egui::Spinner::new().size(32.0).color(ACCENT));
                ui.add_space(32.0);
                skip_button(ui)
            }

            Screen::Choose(cameras) => {
                ui.label(
                    RichText::new("Select a camera")
                        .color(TEXT)
                        .font(FontId::proportional(16.0)),
                );
                ui.add_space(16.0);

                let list = cameras.clone();
                let mut chosen: Option<(String, String, Option<i32>)> = None;
                for cam in &list {
                    let model = cam
                        .advert
                        .as_ref()
                        .and_then(|a| a.model_id)
                        .map(|id| format!("Model {id}"))
                        .unwrap_or_else(|| "Osmo Pocket".into());
                    if camera_card(ui, &cam.name, &model).clicked() {
                        chosen = Some((
                            cam.address.clone(),
                            cam.name.clone(),
                            cam.advert.as_ref().and_then(|a| a.model_id),
                        ));
                    }
                }

                if let Some((addr, name, mid)) = chosen {
                    self.camera_name = name.clone();
                    self.model_id = mid;
                    let _ = self.tx.send(BleCmd::Pair(addr));
                    self.screen = Screen::Pairing {
                        label: "Connecting\u{2026}".into(),
                        camera: name,
                    };
                    return None;
                }

                ui.add_space(24.0);
                if accent_button(ui, "Scan Again").clicked() {
                    let _ = self.tx.send(BleCmd::Scan);
                    self.screen = Screen::Scanning;
                    return None;
                }
                ui.add_space(8.0);
                skip_button(ui)
            }

            Screen::Pairing { label, camera } => {
                let (label, camera) = (label.clone(), camera.clone());
                ui.label(
                    RichText::new(format!("Pairing with {camera}"))
                        .color(TEXT)
                        .font(FontId::proportional(16.0)),
                );
                ui.add_space(12.0);
                ui.label(
                    RichText::new(label)
                        .color(DIM)
                        .font(FontId::proportional(14.0)),
                );
                ui.add_space(16.0);
                ui.add(egui::Spinner::new().size(24.0).color(ACCENT));
                ui.add_space(24.0);
                if ghost_button(ui, "Cancel").clicked() {
                    let _ = self.tx.send(BleCmd::Cancel);
                    let _ = self.tx.send(BleCmd::Scan);
                    self.screen = Screen::Scanning;
                }
                None
            }

            Screen::AwaitingApproval { camera } => {
                let camera = camera.clone();
                ui.label(
                    RichText::new(format!("Approve on {camera}"))
                        .color(TEXT)
                        .font(FontId::proportional(16.0)),
                );
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Press the pairing button on the camera body.")
                        .color(DIM)
                        .font(FontId::proportional(14.0)),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Waiting\u{2026}")
                        .color(ACCENT)
                        .font(FontId::proportional(14.0)),
                );
                ui.add_space(24.0);
                if ghost_button(ui, "Cancel").clicked() {
                    let _ = self.tx.send(BleCmd::Cancel);
                    let _ = self.tx.send(BleCmd::Scan);
                    self.screen = Screen::Scanning;
                }
                None
            }

            Screen::WifiReady {
                ssid,
                password,
                model_id,
                show_pass,
                ..
            } => {
                let (ssid, password, mid, show) =
                    (ssid.clone(), password.clone(), *model_id, *show_pass);

                ui.label(
                    RichText::new("\u{2713}  Pairing complete")
                        .color(SUCCESS)
                        .font(FontId::proportional(18.0)),
                );
                ui.add_space(24.0);
                ui.label(
                    RichText::new("OpenPocketCine will join this network on your PC.")
                        .color(TEXT)
                        .font(FontId::proportional(14.0)),
                );
                ui.add_space(16.0);
                cred_row(ui, "Network", &ssid);
                ui.add_space(8.0);
                let display = if show {
                    password.clone()
                } else {
                    "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}".into()
                };
                ui.horizontal(|ui| {
                    cred_row(ui, "Password", &display);
                    if ui
                        .small_button(if show { "hide" } else { "show" })
                        .clicked()
                    {
                        if let Screen::WifiReady { show_pass, .. } = &mut self.screen {
                            *show_pass = !*show_pass;
                        }
                    }
                });
                ui.add_space(32.0);
                if accent_button(ui, "Join & Connect  \u{2192}").clicked() {
                    return Some(ConnectOutcome::Connected {
                        ssid,
                        password,
                        model_id: mid,
                    });
                }
                None
            }

            Screen::Failed(msg) => {
                let msg = msg.clone();
                ui.label(
                    RichText::new("\u{26a0}  Could not connect")
                        .color(ERR)
                        .font(FontId::proportional(16.0)),
                );
                ui.add_space(12.0);
                ui.label(
                    RichText::new(msg)
                        .color(DIM)
                        .font(FontId::proportional(13.0)),
                );
                ui.add_space(24.0);
                if self.ble_available {
                    if accent_button(ui, "Try Again").clicked() {
                        let _ = self.tx.send(BleCmd::Scan);
                        self.screen = Screen::Scanning;
                        return None;
                    }
                    ui.add_space(8.0);
                }
                skip_button(ui)
            }
        }
    }
}

impl eframe::App for ConnectApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain events from the worker.
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                BleEvent::Scanning => {
                    self.screen = Screen::Scanning;
                }
                BleEvent::Found(cameras) => {
                    if cameras.is_empty() {
                        self.screen = Screen::Failed(
                            "No cameras found. Power on your Osmo Pocket and try again.".into(),
                        );
                    } else {
                        self.screen = Screen::Choose(cameras);
                    }
                }
                BleEvent::Step(label) => {
                    let camera = self.camera_name.clone();
                    self.screen = Screen::Pairing { label, camera };
                }
                BleEvent::AwaitingApproval => {
                    let camera = self.camera_name.clone();
                    self.screen = Screen::AwaitingApproval { camera };
                }
                BleEvent::Credentials { ssid, password } => {
                    self.screen = Screen::WifiReady {
                        camera: self.camera_name.clone(),
                        ssid,
                        password,
                        model_id: self.model_id,
                        show_pass: false,
                    };
                }
                BleEvent::Error(msg) => {
                    self.screen = Screen::Failed(msg);
                }
            }
        }

        // Keep animating while busy.
        if matches!(
            self.screen,
            Screen::Scanning | Screen::Pairing { .. } | Screen::AwaitingApproval { .. }
        ) {
            ctx.request_repaint_after(Duration::from_millis(60));
        }

        // Apply dark palette.
        let mut vis = egui::Visuals::dark();
        vis.panel_fill = BG;
        vis.window_fill = BG;
        vis.widgets.noninteractive.bg_fill = SURFACE;
        vis.widgets.inactive.bg_fill = SURFACE;
        vis.widgets.hovered.bg_fill = Color32::from_rgb(40, 40, 40);
        vis.widgets.active.bg_fill = ACCENT;
        ctx.set_visuals(vis);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG))
            .show(ctx, |ui| {
                ui.with_layout(Layout::top_down(Align::Center), |ui| {
                    ui.add_space(48.0);
                    ui.label(
                        RichText::new("OpenPocketCine")
                            .font(FontId::proportional(28.0))
                            .color(TEXT),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Desktop")
                            .font(FontId::proportional(13.0))
                            .color(DIM),
                    );
                    ui.add_space(40.0);

                    let outcome = self.draw_screen(ui);
                    if let Some(o) = outcome {
                        self.finish(ctx, o);
                    }
                });
            });
    }
}

// ── widget helpers ────────────────────────────────────────────────────────────

fn camera_card(ui: &mut egui::Ui, name: &str, model: &str) -> egui::Response {
    let size = Vec2::new(320.0, 64.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let bg = if resp.hovered() {
            Color32::from_rgb(38, 38, 38)
        } else {
            SURFACE
        };
        ui.painter().rect(
            rect,
            egui::Rounding::same(8.0),
            bg,
            Stroke::new(1.0, BORDER),
        );
        let dot = egui::pos2(rect.left() + 20.0, rect.center().y);
        ui.painter().circle_filled(dot, 5.0, ACCENT);
        ui.painter().text(
            egui::pos2(rect.left() + 38.0, rect.center().y - 9.0),
            egui::Align2::LEFT_CENTER,
            name,
            FontId::proportional(14.0),
            TEXT,
        );
        ui.painter().text(
            egui::pos2(rect.left() + 38.0, rect.center().y + 9.0),
            egui::Align2::LEFT_CENTER,
            model,
            FontId::proportional(11.0),
            DIM,
        );
    }
    ui.add_space(8.0);
    resp
}

fn cred_row(ui: &mut egui::Ui, label: &str, value: &str) {
    egui::Frame::none()
        .fill(SURFACE)
        .rounding(egui::Rounding::same(6.0))
        .stroke(Stroke::new(1.0, BORDER))
        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
        .show(ui, |ui| {
            ui.set_min_width(320.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(label)
                        .color(DIM)
                        .font(FontId::proportional(12.0)),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(value)
                            .color(TEXT)
                            .font(FontId::monospace(13.0)),
                    );
                });
            });
        });
}

fn accent_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(label)
                .color(Color32::BLACK)
                .font(FontId::proportional(15.0)),
        )
        .fill(ACCENT)
        .min_size(Vec2::new(180.0, 40.0))
        .rounding(egui::Rounding::same(6.0)),
    )
}

fn ghost_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(label)
                .color(DIM)
                .font(FontId::proportional(13.0)),
        )
        .fill(Color32::TRANSPARENT),
    )
}

fn skip_button(ui: &mut egui::Ui) -> Option<ConnectOutcome> {
    if ghost_button(ui, "Already on camera Wi-Fi  \u{2192}").clicked() {
        return Some(ConnectOutcome::Skip);
    }
    None
}

// ── BLE worker ────────────────────────────────────────────────────────────────

fn ble_worker(gatt: Option<GattMap>, rx: mpsc::Receiver<BleCmd>, tx: mpsc::Sender<BleEvent>) {
    let gatt = match gatt {
        Some(g) => g,
        None => {
            let _ = tx.send(BleEvent::Error("Cannot read GATT map from core.".into()));
            return;
        }
    };

    let mut transport = match BtleplugTransport::new(&gatt) {
        Ok(t) => t,
        Err(e) => {
            let _ = tx.send(BleEvent::Error(format!("Bluetooth unavailable: {e}")));
            return;
        }
    };

    loop {
        let cmd = match rx.recv() {
            Ok(c) => c,
            Err(_) => break,
        };
        match cmd {
            BleCmd::Scan => {
                let _ = tx.send(BleEvent::Scanning);
                match transport.scan(5.0) {
                    Ok(cameras) => {
                        let _ = tx.send(BleEvent::Found(cameras));
                    }
                    Err(e) => {
                        let _ = tx.send(BleEvent::Error(format!("Scan failed: {e}")));
                    }
                }
            }

            BleCmd::Pair(address) => {
                if let Err(e) = transport.connect(&address) {
                    let _ = tx.send(BleEvent::Error(format!("Connect failed: {e}")));
                    continue;
                }

                // Append to BLE log so we can trace the pairing exchange.
                let log_path = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.join("opc-ble.log")));
                let mut log = log_path.as_deref().and_then(|p| {
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(p)
                        .ok()
                });
                macro_rules! plog {
                    ($($t:tt)*) => {
                        if let Some(ref mut f) = log {
                            let _ = writeln!(f, $($t)*);
                            let _ = f.flush();
                        }
                    };
                }

                let mut assembler = NotificationAssembler::new();

                // Drain notifications and pass complete frames to a closure.
                // Returns true if the closure signalled "done".
                macro_rules! poll_frames {
                    ($body:expr) => {{
                        let mut _done = false;
                        for raw in transport.take_notifications() {
                            plog!(
                                "notif {} bytes: {}",
                                raw.len(),
                                raw.iter()
                                    .map(|b| format!("{b:02x}"))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            );
                            for frame in assembler.append(&raw) {
                                plog!(
                                    "  {:02x}/{:02x} seq={} flags={:02x} payload={}",
                                    frame.cmd_set,
                                    frame.cmd_id,
                                    frame.seq,
                                    frame.flags,
                                    frame
                                        .payload
                                        .iter()
                                        .map(|b| format!("{b:02x}"))
                                        .collect::<Vec<_>>()
                                        .join(" ")
                                );
                                #[allow(clippy::redundant_closure_call)]
                                if ($body)(&frame) {
                                    _done = true;
                                }
                            }
                        }
                        _done
                    }};
                }

                macro_rules! cancel_check {
                    ($lbl:lifetime) => {
                        match rx.try_recv() {
                            Ok(BleCmd::Cancel) | Err(mpsc::TryRecvError::Disconnected) => {
                                break $lbl
                            }
                            _ => {}
                        }
                    };
                }

                // ── PHASE 1: Wake + SetPin sent together ───────────────────────────────
                // Android (PocketCameraSession.kt) sends both without waiting for a Wake
                // reply — the camera expects SetPin to arrive before it will ack.
                let _ = tx.send(BleEvent::Step("Opening session\u{2026}".into()));
                plog!("pair: Wake (seq=0x802B)");
                let ok = opc_camera::Command::SessionWake
                    .encode(0x802B)
                    .map_err(|e| e.to_string())
                    .and_then(|f| transport.write_frame(&f).map_err(|e| e.to_string()));
                if let Err(e) = ok {
                    let _ = tx.send(BleEvent::Error(format!("Wake: {e}")));
                    let _ = transport.disconnect();
                    continue;
                }
                // SetPin goes out 120 ms after Wake (write_frame pacing).
                plog!("pair: SetPin (token=osmo)");
                let ok = pair_set_pin("0000", Some("osmo"))
                    .map_err(|e| e.to_string())
                    .and_then(|f| transport.write_frame(&f).map_err(|e| e.to_string()));
                if let Err(e) = ok {
                    let _ = tx.send(BleEvent::Error(format!("SetPin: {e}")));
                    let _ = transport.disconnect();
                    continue;
                }

                // ── PHASE 2: Wait for SetPin reply (0x07/0x45) or approval (0x07/0x46) ─
                let _ = tx.send(BleEvent::Step("Authenticating\u{2026}".into()));
                let mut pin_accepted = false;
                let pin_deadline = Instant::now() + Duration::from_secs(90);

                'pin: loop {
                    cancel_check!('pin);
                    if Instant::now() > pin_deadline {
                        let _ = tx.send(BleEvent::Error(
                            "Pairing timed out. Hold the camera closer and try again.".into(),
                        ));
                        break 'pin;
                    }
                    if poll_frames!(|frame: &opc_camera::DumlFrame| {
                        match (frame.cmd_set, frame.cmd_id) {
                            (0x07, 0x45) => match frame.payload.get(1) {
                                Some(0x01) => {
                                    plog!("  known client");
                                    true
                                }
                                Some(0x02) => {
                                    plog!("  needs approval");
                                    let _ = tx.send(BleEvent::AwaitingApproval);
                                    false
                                }
                                other => {
                                    plog!("  pin refused: {other:?}");
                                    let _ = tx.send(BleEvent::Error(
                                        "Camera refused the pairing request.".into(),
                                    ));
                                    false
                                }
                            },
                            (0x07, 0x46) => {
                                plog!("  approval request seq={} — ACKing", frame.seq);
                                if let Ok(ack) = pair_approval_ack(frame.seq) {
                                    if let Err(e) = transport.write_frame(&ack) {
                                        plog!("  ACK write: {e}");
                                    }
                                }
                                true
                            }
                            _ => false,
                        }
                    }) {
                        pin_accepted = true;
                        break 'pin;
                    }
                    thread::sleep(Duration::from_millis(50));
                }

                if !pin_accepted {
                    let _ = transport.disconnect();
                    continue;
                }

                // ── PHASE 3: Wake access point ─────────────────────────────────────────
                let _ = tx.send(BleEvent::Step("Waking camera Wi-Fi\u{2026}".into()));
                thread::sleep(Duration::from_millis(200));
                plog!("pair: WakeAccessPoint");
                if let Ok(f) = pair_wake_access_point() {
                    if let Err(e) = transport.write_frame(&f) {
                        plog!("WakeAP write: {e}");
                    }
                }
                let ap_dl = Instant::now() + Duration::from_secs(5);
                'ap: loop {
                    cancel_check!('ap);
                    if Instant::now() > ap_dl {
                        break 'ap;
                    }
                    if poll_frames!(
                        |f: &opc_camera::DumlFrame| f.cmd_set == 0x53 && f.cmd_id == 0x10
                    ) {
                        plog!("  WakeAP ack");
                        break 'ap;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                thread::sleep(Duration::from_millis(600));

                // ── PHASE 4: Read SSID ─────────────────────────────────────────────────
                let _ = tx.send(BleEvent::Step("Reading Wi-Fi name\u{2026}".into()));
                plog!("pair: GetWifiSsid (seq=0x8007)");
                let ok = opc_camera::Command::GetWifiSsid
                    .encode(0x8007)
                    .map_err(|e| e.to_string())
                    .and_then(|f| transport.write_frame(&f).map_err(|e| e.to_string()));
                if let Err(e) = ok {
                    let _ = tx.send(BleEvent::Error(format!("GetSsid: {e}")));
                    let _ = transport.disconnect();
                    continue;
                }
                let mut ssid = String::new();
                let ssid_dl = Instant::now() + Duration::from_secs(10);
                'ssid: loop {
                    cancel_check!('ssid);
                    if Instant::now() > ssid_dl {
                        break 'ssid;
                    }
                    poll_frames!(|f: &opc_camera::DumlFrame| {
                        if f.cmd_set == 0x07 && f.cmd_id == 0x07 {
                            if let Ok(s) = status_string(&f.payload) {
                                if !s.is_empty() {
                                    ssid = s;
                                    return true;
                                }
                            }
                        }
                        false
                    });
                    if !ssid.is_empty() {
                        break 'ssid;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                if ssid.is_empty() {
                    let _ = tx.send(BleEvent::Error(
                        "Camera did not return a Wi-Fi name.".into(),
                    ));
                    let _ = transport.disconnect();
                    continue;
                }

                // ── PHASE 5: Read password ─────────────────────────────────────────────
                let _ = tx.send(BleEvent::Step("Reading password\u{2026}".into()));
                plog!("pair: GetWifiPassword (seq=0x800E)");
                let ok = opc_camera::Command::GetWifiPassword
                    .encode(0x800E)
                    .map_err(|e| e.to_string())
                    .and_then(|f| transport.write_frame(&f).map_err(|e| e.to_string()));
                if let Err(e) = ok {
                    let _ = tx.send(BleEvent::Error(format!("GetPass: {e}")));
                    let _ = transport.disconnect();
                    continue;
                }
                let mut password = String::new();
                let pass_dl = Instant::now() + Duration::from_secs(10);
                'pass: loop {
                    cancel_check!('pass);
                    if Instant::now() > pass_dl {
                        break 'pass;
                    }
                    poll_frames!(|f: &opc_camera::DumlFrame| {
                        if f.cmd_set == 0x07 && f.cmd_id == 0x0E {
                            if let Ok(s) = status_string(&f.payload) {
                                if !s.is_empty() {
                                    password = s;
                                    return true;
                                }
                            }
                        }
                        false
                    });
                    if !password.is_empty() {
                        break 'pass;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                if password.is_empty() {
                    let _ = tx.send(BleEvent::Error(
                        "Camera did not return a Wi-Fi password.".into(),
                    ));
                    let _ = transport.disconnect();
                    continue;
                }

                plog!("pair: done  ssid={ssid}");
                let _ = tx.send(BleEvent::Credentials { ssid, password });
                let _ = transport.disconnect();
            }

            BleCmd::Cancel => {
                let _ = transport.disconnect();
                break;
            }
        }
    }
    let _ = transport.disconnect();
}

// ── public entry point ────────────────────────────────────────────────────────

/// Opens the connection screen and returns when the user connects, skips, or quits.
#[cfg(opc_core_linked)]
pub fn run() -> ConnectOutcome {
    let gatt = GattMap::from_core();
    let ble_available = gatt.is_some();

    let (cmd_tx, cmd_rx) = mpsc::channel::<BleCmd>();
    let (evt_tx, evt_rx) = mpsc::channel::<BleEvent>();
    let outcome_arc: Arc<Mutex<Option<ConnectOutcome>>> = Arc::new(Mutex::new(None));
    let arc2 = Arc::clone(&outcome_arc);

    thread::spawn(move || ble_worker(gatt, cmd_rx, evt_tx));

    let app = ConnectApp::new(cmd_tx, evt_rx, ble_available, arc2);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("OpenPocketCine")
            .with_inner_size([520.0, 480.0])
            .with_resizable(false),
        ..Default::default()
    };

    let _ = eframe::run_native(
        "OpenPocketCine",
        options,
        Box::new(move |_cc| Ok(Box::new(app) as Box<dyn eframe::App>)),
    );

    Arc::try_unwrap(outcome_arc)
        .ok()
        .and_then(|m| m.into_inner().ok())
        .flatten()
        .unwrap_or(ConnectOutcome::Quit)
}
