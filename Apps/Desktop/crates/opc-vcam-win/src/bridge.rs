//! The pipe from the viewfinder, read on its own thread into a latest-frame slot.

use std::io::Read;
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use opc_vcam::wire::{self, Header, HEADER_LEN, PIPE_NAME};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::IO::CancelSynchronousIo;

/// The frame the source hands out next.
#[derive(Debug)]
struct Latest {
    frame: Arc<Vec<u8>>,
    /// Counts up with every frame off the pipe, so a reader can tell fresh from stale.
    seq: u64,
}

#[derive(Debug)]
struct Inner {
    width: u32,
    height: u32,
    latest: Mutex<Latest>,
    closing: AtomicBool,
}

/// Reads the viewfinder's pipe for as long as it lives.
#[derive(Debug)]
pub struct Bridge {
    inner: Arc<Inner>,
    reader: Option<JoinHandle<()>>,
}

/// NV12 black: luma at 16, chroma at 128.
pub fn black(width: u32, height: u32) -> Vec<u8> {
    let luma = (width as usize) * (height as usize);
    let mut out = vec![128u8; wire::nv12_len(width, height)];
    out[..luma].fill(16);
    out
}

impl Bridge {
    pub fn start(width: u32, height: u32) -> Self {
        let inner = Arc::new(Inner {
            width,
            height,
            latest: Mutex::new(Latest {
                frame: Arc::new(black(width, height)),
                seq: 0,
            }),
            closing: AtomicBool::new(false),
        });
        let reader = {
            let inner = inner.clone();
            std::thread::Builder::new()
                .name("opc-vcam-bridge".to_string())
                .spawn(move || read_loop(&inner))
                .ok()
        };
        Self { inner, reader }
    }

    /// The newest frame and its count.
    pub fn latest(&self) -> (Arc<Vec<u8>>, u64) {
        self.inner
            .latest
            .lock()
            .map(|latest| (latest.frame.clone(), latest.seq))
            .unwrap_or_else(|_| (Arc::new(black(self.inner.width, self.inner.height)), 0))
    }

    pub fn stop(&mut self) {
        self.inner.closing.store(true, Ordering::SeqCst);
        if let Some(reader) = self.reader.take() {
            // Safety: the thread is alive until joined; a blocked read is cancelled.
            unsafe {
                let _ = CancelSynchronousIo(HANDLE(reader.as_raw_handle()));
            }
            let _ = reader.join();
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_loop(inner: &Inner) {
    while !inner.closing.load(Ordering::SeqCst) {
        match std::fs::File::open(PIPE_NAME) {
            Ok(mut pipe) => {
                while !inner.closing.load(Ordering::SeqCst) {
                    if read_frame(inner, &mut pipe).is_err() {
                        break;
                    }
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

/// One header and its payload; a frame of another size is read and dropped.
fn read_frame(inner: &Inner, pipe: &mut std::fs::File) -> std::io::Result<()> {
    let mut header = [0u8; HEADER_LEN];
    pipe.read_exact(&mut header)?;
    let header = Header::decode(&header)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad header"))?;
    let mut payload = vec![0u8; header.length as usize];
    pipe.read_exact(&mut payload)?;
    if header.width != inner.width || header.height != inner.height {
        return Ok(());
    }
    if let Ok(mut latest) = inner.latest.lock() {
        latest.frame = Arc::new(payload);
        latest.seq += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_is_video_black_not_green() {
        let frame = black(4, 2);
        assert_eq!(frame.len(), 12);
        assert!(frame[..8].iter().all(|b| *b == 16));
        assert!(frame[8..].iter().all(|b| *b == 128));
    }
}
