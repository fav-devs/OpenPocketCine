//! The Windows camera: a Media Foundation virtual camera whose source lives in the
//! `opc_vcam_win` DLL, registered once, and which the Frame Server loads on demand.
//! This side brings the camera up for the session and serves frames to the source
//! over a named pipe, since the source runs in the service's process, not ours.

use std::os::windows::io::AsRawHandle;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, HANDLE};
use windows::Win32::Media::MediaFoundation::{
    IMFVirtualCamera, MFCreateVirtualCamera, MFShutdown, MFStartup,
    MFVirtualCameraAccess_CurrentUser, MFVirtualCameraLifetime_Session,
    MFVirtualCameraType_SoftwareCameraSource, MFSTARTUP_NOSOCKET, MF_VERSION,
};
use windows::Win32::Storage::FileSystem::{WriteFile, PIPE_ACCESS_OUTBOUND};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_WAIT,
};
use windows::Win32::System::IO::CancelSynchronousIo;

use crate::wire::{self, Header, PIPE_NAME};
use crate::{Error, Frame, Sink};

/// The friendly name apps list.
pub const CAMERA_NAME: &str = "OpenPocketCine";
pub use crate::wire::SOURCE_CLSID;

/// The newest NV12 frame waiting for the pipe, and whether we are closing.
#[derive(Debug, Default)]
struct Slot {
    frame: Option<Arc<Vec<u8>>>,
    header: Option<Header>,
    closing: bool,
}

type Shared = Arc<(Mutex<Slot>, Condvar)>;

/// The camera object, created, used and released on the worker thread alone. Media
/// Foundation objects are free-threaded; the marker only lets the sink cross into the
/// worker as a `Box<dyn Sink>`.
#[derive(Debug)]
struct Camera(IMFVirtualCamera);

// Safety: see above — never touched from any thread but the one that opened it.
unsafe impl Send for Camera {}

/// The camera for this session, and the pipe that feeds its source.
#[derive(Debug)]
pub struct WinCamSink {
    camera: Option<Camera>,
    shared: Shared,
    server: Option<JoinHandle<()>>,
    width: u32,
    height: u32,
}

impl WinCamSink {
    /// Registers the camera for this session and starts serving the pipe. Fails, with
    /// the registration hint, when the source DLL is not registered on this machine.
    pub fn open(width: u32, height: u32) -> Result<Self, Error> {
        // Safety: plain calls on the worker thread that owns this sink.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(com)?;
        }
        let shared: Shared = Arc::new((Mutex::new(Slot::default()), Condvar::new()));
        let server = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("opc-vcam-pipe".to_string())
                .spawn(move || serve_pipe(&shared))?
        };
        let name = HSTRING::from(CAMERA_NAME);
        let clsid = HSTRING::from(SOURCE_CLSID);
        // Safety: the strings outlive the call; categories may be empty.
        let camera = unsafe {
            MFCreateVirtualCamera(
                MFVirtualCameraType_SoftwareCameraSource,
                MFVirtualCameraLifetime_Session,
                MFVirtualCameraAccess_CurrentUser,
                PCWSTR(name.as_ptr()),
                PCWSTR(clsid.as_ptr()),
                None,
            )
            .and_then(|camera| {
                camera.Start(None)?;
                Ok(camera)
            })
        };
        let camera = match camera {
            Ok(camera) => camera,
            Err(error) => {
                stop_server(&shared, server);
                return Err(Error::Frame(format!(
                    "the camera could not start ({}) — register opc_vcam_win.dll once with `regsvr32` as administrator, on Windows 11 22H2 or later",
                    error.message()
                )));
            }
        };
        Ok(Self {
            camera: Some(Camera(camera)),
            shared,
            server: Some(server),
            width,
            height,
        })
    }
}

fn com(error: windows::core::Error) -> Error {
    Error::Frame(error.message())
}

impl Sink for WinCamSink {
    fn describe(&self) -> String {
        format!(
            "Camera · {CAMERA_NAME} (Media Foundation) · {}×{}",
            self.width, self.height
        )
    }

    fn push(&mut self, frame: &Frame) -> Result<(), Error> {
        if frame.width != self.width || frame.height != self.height {
            return Err(Error::Frame(format!(
                "frame is {}×{}, the camera was set to {}×{}",
                frame.width, frame.height, self.width, self.height
            )));
        }
        let nv12 = wire::rgba_to_nv12(frame.width, frame.height, &frame.rgba);
        let (slot, wake) = &*self.shared;
        let mut inside = slot.lock().map_err(|_| Error::Frame("poisoned".into()))?;
        inside.frame = Some(Arc::new(nv12));
        inside.header = Some(Header::nv12(frame.width, frame.height));
        wake.notify_all();
        Ok(())
    }
}

impl Drop for WinCamSink {
    fn drop(&mut self) {
        if let Some(Camera(camera)) = self.camera.take() {
            // Safety: the camera is ours to stop; failures here have nowhere to go.
            unsafe {
                let _ = camera.Stop();
                let _ = camera.Remove();
                let _ = camera.Shutdown();
            }
        }
        if let Some(server) = self.server.take() {
            stop_server(&self.shared, server);
        }
        // Safety: pairs with `open`, on the same thread.
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

fn stop_server(shared: &Shared, server: JoinHandle<()>) {
    let (slot, wake) = &**shared;
    if let Ok(mut inside) = slot.lock() {
        inside.closing = true;
        wake.notify_all();
    }
    // Safety: the thread handle is valid until joined; cancelling a blocked connect or
    // write is the documented way out of one.
    unsafe {
        let _ = CancelSynchronousIo(HANDLE(server.as_raw_handle()));
    }
    // A client of our own unblocks a pending connect that was not cancelled.
    let _ = std::fs::File::open(PIPE_NAME);
    let _ = server.join();
}

/// Waits for a frame newer than the last, or `None` once closing.
fn next_frame(shared: &Shared, seen: Option<*const Vec<u8>>) -> Option<(Arc<Vec<u8>>, Header)> {
    let (slot, wake) = &**shared;
    let mut inside = slot.lock().ok()?;
    loop {
        if inside.closing {
            return None;
        }
        if let (Some(frame), Some(header)) = (&inside.frame, inside.header) {
            if seen != Some(Arc::as_ptr(frame)) {
                return Some((frame.clone(), header));
            }
        }
        inside = wake.wait(inside).ok()?;
    }
}

/// One client at a time — the Frame Server's source — reconnecting for as long as the
/// camera is on. A write the reader will not take blocks here, not in the window.
fn serve_pipe(shared: &Shared) {
    let name = HSTRING::from(PIPE_NAME);
    // Safety: a fresh pipe with room for a few frames; the handle is closed below.
    let pipe = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_OUTBOUND,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            4 * 1024 * 1024,
            0,
            0,
            None,
        )
    };
    if pipe.is_invalid() {
        return;
    }
    loop {
        if shared.0.lock().map(|s| s.closing).unwrap_or(true) {
            break;
        }
        // Safety: blocks until a reader arrives; a reader already there is fine too.
        let connected = unsafe { ConnectNamedPipe(pipe, None) };
        if let Err(error) = connected {
            if error.code() != ERROR_PIPE_CONNECTED.to_hresult() {
                break;
            }
        }
        let mut seen = None;
        while let Some((frame, header)) = next_frame(shared, seen) {
            seen = Some(Arc::as_ptr(&frame));
            if write_all(pipe, &header.encode()).is_err() || write_all(pipe, &frame).is_err() {
                break;
            }
        }
        // Safety: drop this reader and wait for the next.
        unsafe {
            let _ = DisconnectNamedPipe(pipe);
        }
    }
    // Safety: ours to close.
    unsafe {
        let _ = CloseHandle(pipe);
    }
}

fn write_all(pipe: HANDLE, mut bytes: &[u8]) -> windows::core::Result<()> {
    while !bytes.is_empty() {
        let mut written = 0u32;
        // Safety: `bytes` is a live slice; `written` receives the count.
        unsafe { WriteFile(pipe, Some(bytes), Some(&mut written), None)? };
        if written == 0 {
            return Err(windows::core::Error::from_thread());
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}
