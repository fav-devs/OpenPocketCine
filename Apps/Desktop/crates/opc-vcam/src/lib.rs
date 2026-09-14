//! The viewfinder as a camera for other apps.
//!
//! The window hands the graded picture here once per presented frame; a worker thread
//! pushes it into whichever [`Backend`] the operator chose:
//!
//! - [`Backend::Device`] — the platform's own camera: a `v4l2loopback` device on Linux;
//!   on Windows 11 a Media Foundation virtual camera whose source (`opc_vcam_win.dll`,
//!   registered once) the Frame Server loads and this crate feeds over a named pipe; on
//!   macOS the OpenPocketCine camera extension's sink stream, reached through the Swift
//!   facade. Every app that opens a webcam (Chrome, Zoom, Teams, OBS, ffmpeg) sees it.
//! - [`Backend::Stream`] — MJPEG over HTTP on the loopback interface, on every
//!   platform. OBS reads it as a Media Source and its own Virtual Camera hands it to the
//!   apps that need a camera on Windows and macOS, where there is no driver-free way to
//!   register one from a plain executable.
//!
//! Latest wins: a frame that arrives while the worker is still writing the last one
//! replaces the waiting one rather than queueing behind it, so the camera never runs
//! behind the window.

use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

mod convert;
pub mod install;
#[cfg(target_os = "macos")]
pub mod mac;
mod stream;
#[cfg(target_os = "linux")]
pub(crate) mod v4l2;
#[cfg(windows)]
pub mod win;
pub mod wire;

pub use convert::rgba_to_yuyv;
pub use install::{ComponentReport, ComponentState};
pub use stream::{StreamServer, DEFAULT_PORT};

/// The picture the camera carries: 1280 × 720, whatever the window is.
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;

/// One frame from the renderer: tightly packed RGBA8, `width × height × 4` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    /// Whether the buffer really is `width × height × 4`.
    pub fn is_sane(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.rgba.len() == (self.width as usize) * (self.height as usize) * 4
    }
}

/// Where the frames go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// The platform's camera: a `v4l2loopback` device on Linux, found by asking every
    /// `/dev/video*` for its driver; the Media Foundation virtual camera on Windows.
    Device,
    /// MJPEG over HTTP on `127.0.0.1:port`.
    Stream { port: u16 },
}

/// What the worker last did, for the settings readout.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// "Camera · /dev/video10 · 1280×720" or "Stream · http://127.0.0.1:8890/stream".
    pub where_: String,
    /// Frames the backend accepted.
    pub frames: u64,
    /// The last failure, or empty while it works.
    pub error: String,
}

impl Status {
    /// One line for a readout row.
    pub fn line(&self) -> String {
        if !self.error.is_empty() {
            return self.error.clone();
        }
        if self.where_.is_empty() {
            return "Starting".to_string();
        }
        format!("{} · {} frames", self.where_, self.frames)
    }
}

#[derive(Debug)]
pub enum Error {
    /// No `v4l2loopback` device on this machine.
    NoDevice,
    /// The platform has no driver-free way to register a camera.
    Unsupported,
    Io(std::io::Error),
    Frame(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => write!(
                f,
                "no v4l2loopback device — sudo modprobe v4l2loopback exclusive_caps=1 card_label=OpenPocketCine"
            ),
            Self::Unsupported => write!(
                f,
                "no camera device on this platform — use the stream with OBS's virtual camera"
            ),
            Self::Io(error) => write!(f, "{error}"),
            Self::Frame(what) => write!(f, "{what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// A backend the worker writes to. One per thread; never shared.
pub trait Sink: Send {
    /// Where the frames are going, for the readout.
    fn describe(&self) -> String;
    /// One frame. A sink that cannot keep the frame drops it and says why.
    fn push(&mut self, frame: &Frame) -> Result<(), Error>;
}

/// Opens the backend, or says why it cannot be.
pub fn open(backend: &Backend) -> Result<Box<dyn Sink>, Error> {
    match backend {
        Backend::Device => open_device(),
        Backend::Stream { port } => Ok(Box::new(StreamServer::bind(*port)?)),
    }
}

#[cfg(target_os = "linux")]
fn open_device() -> Result<Box<dyn Sink>, Error> {
    let path = v4l2::find_loopback().ok_or(Error::NoDevice)?;
    Ok(Box::new(v4l2::LoopbackSink::open(&path, WIDTH, HEIGHT)?))
}

#[cfg(windows)]
fn open_device() -> Result<Box<dyn Sink>, Error> {
    Ok(Box::new(win::WinCamSink::open(WIDTH, HEIGHT)?))
}

#[cfg(target_os = "macos")]
fn open_device() -> Result<Box<dyn Sink>, Error> {
    Ok(Box::new(mac::MacCamSink::open(WIDTH, HEIGHT)?))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn open_device() -> Result<Box<dyn Sink>, Error> {
    Err(Error::Unsupported)
}

struct Mailbox {
    frame: Option<Frame>,
    stop: bool,
}

/// The running camera: a worker thread and the slot it reads from.
#[derive(Debug)]
pub struct VirtualCamera {
    backend: Backend,
    slot: Arc<(Mutex<Mailbox>, Condvar)>,
    status: Arc<Mutex<Status>>,
    worker: Option<JoinHandle<()>>,
}

impl fmt::Debug for Mailbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mailbox")
            .field("waiting", &self.frame.is_some())
            .field("stop", &self.stop)
            .finish()
    }
}

impl VirtualCamera {
    /// Starts the worker on a sink. The backend is opened on the worker, so a slow or
    /// failing open never stalls the window; the status says how it went.
    pub fn start(backend: Backend) -> Self {
        let slot = Arc::new((
            Mutex::new(Mailbox {
                frame: None,
                stop: false,
            }),
            Condvar::new(),
        ));
        let status = Arc::new(Mutex::new(Status::default()));
        let worker = {
            let slot = slot.clone();
            let status = status.clone();
            let backend = backend.clone();
            std::thread::Builder::new()
                .name("opc-vcam".to_string())
                .spawn(move || run(&backend, &slot, &status))
                .ok()
        };
        Self {
            backend,
            slot,
            status,
            worker,
        }
    }

    /// The backend this camera was started on.
    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    /// A new frame for the camera. Never blocks; replaces one still waiting.
    pub fn offer(&self, frame: Frame) {
        if !frame.is_sane() {
            return;
        }
        let (mailbox, wake) = &*self.slot;
        if let Ok(mut inside) = mailbox.lock() {
            inside.frame = Some(frame);
            wake.notify_one();
        }
    }

    /// What the worker last did.
    pub fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Stops the worker and closes the backend. Also what dropping does.
    pub fn stop(mut self) {
        self.shut_down();
    }

    fn shut_down(&mut self) {
        let (mailbox, wake) = &*self.slot;
        if let Ok(mut inside) = mailbox.lock() {
            inside.stop = true;
            wake.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for VirtualCamera {
    fn drop(&mut self) {
        self.shut_down();
    }
}

fn run(backend: &Backend, slot: &Arc<(Mutex<Mailbox>, Condvar)>, status: &Arc<Mutex<Status>>) {
    let note = |update: &dyn Fn(&mut Status)| {
        if let Ok(mut inside) = status.lock() {
            update(&mut inside);
        }
    };
    let mut sink = match open(backend) {
        Ok(sink) => sink,
        Err(error) => {
            note(&|s| s.error = error.to_string());
            return;
        }
    };
    let where_ = sink.describe();
    note(&|s| {
        s.where_.clone_from(&where_);
        s.error.clear();
    });
    let (mailbox, wake) = &**slot;
    loop {
        let frame = {
            let Ok(mut inside) = mailbox.lock() else {
                return;
            };
            loop {
                if inside.stop {
                    return;
                }
                if let Some(frame) = inside.frame.take() {
                    break frame;
                }
                inside = match wake.wait(inside) {
                    Ok(inside) => inside,
                    Err(_) => return,
                };
            }
        };
        match sink.push(&frame) {
            Ok(()) => note(&|s| {
                s.frames += 1;
                s.error.clear();
            }),
            Err(error) => note(&|s| s.error = error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    struct Counting(mpsc::Sender<u32>);

    impl Sink for Counting {
        fn describe(&self) -> String {
            "test".to_string()
        }
        fn push(&mut self, frame: &Frame) -> Result<(), Error> {
            let _ = self.0.send(frame.width);
            Ok(())
        }
    }

    fn frame(width: u32) -> Frame {
        Frame {
            width,
            height: 2,
            rgba: vec![0; width as usize * 8],
        }
    }

    #[test]
    fn the_slot_keeps_only_the_latest_frame() {
        let slot = Arc::new((
            Mutex::new(Mailbox {
                frame: None,
                stop: false,
            }),
            Condvar::new(),
        ));
        let status = Arc::new(Mutex::new(Status::default()));
        let camera = VirtualCamera {
            backend: Backend::Device,
            slot: slot.clone(),
            status: status.clone(),
            worker: None,
        };
        // Two offers before anyone reads: the second replaces the first.
        camera.offer(frame(4));
        camera.offer(frame(6));
        camera.offer(Frame {
            width: 9,
            height: 9,
            rgba: vec![0; 3],
        });
        let waiting = slot.0.lock().unwrap().frame.clone().unwrap();
        assert_eq!(
            waiting.width, 6,
            "a malformed frame never replaces a good one"
        );

        let (tx, rx) = mpsc::channel();
        let mut sink: Box<dyn Sink> = Box::new(Counting(tx));
        // Drive the worker loop by hand: one frame, then stop.
        let (mailbox, wake) = &*slot;
        {
            let inside = mailbox.lock().unwrap();
            let f = inside.frame.clone().unwrap();
            drop(inside);
            sink.push(&f).unwrap();
        }
        assert_eq!(rx.try_recv(), Ok(6));
        mailbox.lock().unwrap().stop = true;
        wake.notify_all();
        assert_eq!(status.lock().unwrap().line(), "Starting");
    }

    #[test]
    fn a_status_reads_as_one_line() {
        let mut status = Status {
            where_: "Camera · /dev/video10".to_string(),
            frames: 12,
            error: String::new(),
        };
        assert_eq!(status.line(), "Camera · /dev/video10 · 12 frames");
        status.error = "gone".to_string();
        assert_eq!(status.line(), "gone");
        assert_eq!(Error::NoDevice.to_string().split(' ').next(), Some("no"));
    }

    #[test]
    fn a_stream_camera_runs_end_to_end() {
        let camera = VirtualCamera::start(Backend::Stream { port: 0 });
        // The worker opens the server; wait for it to say where.
        let mut tries = 0;
        while camera.status().where_.is_empty() && camera.status().error.is_empty() && tries < 200 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            tries += 1;
        }
        let status = camera.status();
        assert!(status.error.is_empty(), "{}", status.error);
        assert!(status.where_.starts_with("Stream · http://127.0.0.1:"));
        camera.offer(frame(8));
        tries = 0;
        while camera.status().frames == 0 && tries < 200 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            tries += 1;
        }
        assert_eq!(camera.status().frames, 1);
        camera.stop();
    }
}
