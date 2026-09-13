//! The camera on its own thread.
//!
//! The datalink has a 40 Hz pump to keep and a 5 ms read timeout to sit on; the window
//! has a swapchain to feed. Neither can wait for the other, so they are two threads with
//! two channels between them, and nothing shared but the messages.

use std::fs::OpenOptions;
use std::io::Write;
use std::net::SocketAddr;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

use opc_camera::{CameraSession, Command, Recovery, SessionEvent, Status};

/// What the camera thread tells the window.
#[derive(Debug)]
pub enum FromCamera {
    /// The session opened.
    Opened,
    /// A complete access unit, ready for the decoder.
    Picture(Vec<u8>),
    /// The camera said something about itself.
    Status(Box<Status>),
    /// The feed stalled and the watchdog is working on it.
    Recovering(Recovery),
    /// The camera never answered, or the link died.
    Lost(String),
}

/// What the window tells the camera thread.
#[derive(Debug)]
enum ToCamera {
    Send(Command),
    /// A picture reached the screen. The watchdog counts presented frames, not arrived
    /// ones — a decoder quietly producing nothing looks like a healthy feed otherwise.
    Presented,
    /// The decoder is wedged, or has just been rebuilt.
    DecoderFailed(bool),
    Stop,
}

/// A running camera link.
#[derive(Debug)]
pub struct Link {
    commands: Sender<ToCamera>,
    events: Receiver<FromCamera>,
    worker: Option<JoinHandle<()>>,
}

impl Link {
    /// Opens the datalink on its own thread and starts pumping.
    ///
    /// `session_id` and `base_seq` must be fresh per connect; a fixed base sequence can
    /// wedge the camera. `model_id` tells the status decoder which body this is, so it
    /// reads that body's own colour-mode encodings rather than another's.
    pub fn open(
        remote: Option<SocketAddr>,
        session_id: u16,
        base_seq: u16,
        model_id: Option<i32>,
    ) -> Self {
        let (commands, command_rx) = channel();
        let (event_tx, events) = channel();
        let worker = std::thread::Builder::new()
            .name("opc-camera".to_string())
            .spawn(move || {
                run(
                    remote,
                    session_id,
                    base_seq,
                    model_id,
                    &command_rx,
                    &event_tx,
                )
            })
            .ok();
        Self {
            commands,
            events,
            worker,
        }
    }

    /// Queues a command. A closed link drops it rather than failing the caller: the
    /// window has already been told the link is gone.
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(ToCamera::Send(command));
    }

    pub fn note_presented(&self) {
        let _ = self.commands.send(ToCamera::Presented);
    }

    pub fn note_decoder_failed(&self, failed: bool) {
        let _ = self.commands.send(ToCamera::DecoderFailed(failed));
    }

    /// Everything waiting from the camera.
    pub fn drain(&self) -> Vec<FromCamera> {
        self.events.try_iter().collect()
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        let _ = self.commands.send(ToCamera::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run(
    remote: Option<SocketAddr>,
    session_id: u16,
    base_seq: u16,
    model_id: Option<i32>,
    commands: &Receiver<ToCamera>,
    events: &Sender<FromCamera>,
) {
    // The window is normally launched without a terminal on Windows. Keep the network
    // state beside the executable, but never log BLE credentials or camera payloads.
    let mut log = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("opc-monitor.log")))
        .and_then(|path| OpenOptions::new().create(true).append(true).open(path).ok());
    macro_rules! link_log {
        ($($t:tt)*) => {
            if let Some(ref mut file) = log {
                let _ = writeln!(file, "link: {}", format_args!($($t)*));
            }
        };
    }

    link_log!(
        "starting UDP session remote={:?} model={model_id:?}",
        remote
    );
    let opened = match remote {
        Some(address) => CameraSession::connect_to(address, session_id, base_seq),
        None => CameraSession::connect(session_id, base_seq),
    };
    let mut session = match opened {
        Ok(session) => session,
        Err(error) => {
            link_log!("could not open UDP socket: {error}");
            let _ = events.send(FromCamera::Lost(error.to_string()));
            return;
        }
    };
    link_log!(
        "UDP socket bound local_port={} remote={} phase={:?}",
        session.local_port(),
        session.remote(),
        session.phase()
    );
    if let Some(model_id) = model_id {
        session.set_model(model_id);
    }
    let mut saw_picture = false;
    let mut logged_access_units = 0usize;

    loop {
        loop {
            match commands.try_recv() {
                Ok(ToCamera::Send(command)) => session.send(command),
                Ok(ToCamera::Presented) => session.note_presented(),
                Ok(ToCamera::DecoderFailed(failed)) => session.set_decoder_failed(failed),
                Ok(ToCamera::Stop) | Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => break,
            }
        }

        let polled = match session.poll() {
            Ok(polled) => polled,
            Err(error) => {
                link_log!("UDP poll failed: {error}");
                let _ = events.send(FromCamera::Lost(error.to_string()));
                return;
            }
        };
        for event in polled {
            let message = match event {
                SessionEvent::Opened => {
                    link_log!("camera accepted session; phase={:?}", session.phase());
                    FromCamera::Opened
                }
                SessionEvent::Picture(bytes) => {
                    if !saw_picture {
                        link_log!("received first picture ({} bytes)", bytes.len());
                        saw_picture = true;
                    }
                    if logged_access_units < 3 {
                        let head = bytes
                            .iter()
                            .take(32)
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        link_log!(
                            "access unit #{} bytes={} head={head}",
                            logged_access_units + 1,
                            bytes.len()
                        );
                        logged_access_units += 1;
                    }
                    FromCamera::Picture(bytes)
                }
                SessionEvent::StatusChanged => {
                    link_log!("received camera status");
                    FromCamera::Status(Box::new(session.status()))
                }
                SessionEvent::Recovering(recovery) => {
                    link_log!("feed watchdog: {recovery:?}");
                    FromCamera::Recovering(recovery)
                }
                SessionEvent::Unreachable => {
                    link_log!("camera did not answer its UDP session handshake");
                    let _ = events.send(FromCamera::Lost(
                        "the camera did not answer — is this machine on its Wi-Fi?".to_string(),
                    ));
                    return;
                }
                // Command replies are already folded into the status decoder.
                SessionEvent::Frame(_) => continue,
            };
            if events.send(message).is_err() {
                return;
            }
        }
    }
}
