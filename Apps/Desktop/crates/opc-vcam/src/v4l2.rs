//! A `v4l2loopback` output device, driven with the kernel's own ioctls.
//!
//! The structs mirror `<linux/videodev2.h>`; their sizes and the ioctl numbers are
//! pinned by tests against the values the C header gives on x86-64 and aarch64. Only
//! what a writer needs is here: find the loopback, set the output format, write frames.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use crate::{rgba_to_yuyv, Error, Frame, Sink};

/// `struct v4l2_pix_format`.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct PixFormat {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    bytesperline: u32,
    sizeimage: u32,
    colorspace: u32,
    priv_: u32,
    flags: u32,
    ycbcr_enc: u32,
    quantization: u32,
    xfer_func: u32,
}

/// The `fmt` union of `struct v4l2_format`: 200 bytes, aligned like a pointer because
/// one of its members carries one.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FormatBody {
    pix: PixFormat,
    rest: [u8; 200 - size_of::<PixFormat>()],
    align: [usize; 0],
}

/// `struct v4l2_format`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Format {
    type_: u32,
    fmt: FormatBody,
}

/// `struct v4l2_capability`.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct Capability {
    driver: [u8; 16],
    card: [u8; 32],
    bus_info: [u8; 32],
    version: u32,
    capabilities: u32,
    device_caps: u32,
    reserved: [u32; 3],
}

const BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
const FIELD_NONE: u32 = 1;
const COLORSPACE_SMPTE170M: u32 = 1;
const PIX_FMT_PRIV_MAGIC: u32 = 0xfeed_cafe;
const CAP_VIDEO_OUTPUT: u32 = 0x0000_0002;
/// `v4l2_fourcc('Y', 'U', 'Y', 'V')`.
const PIX_FMT_YUYV: u32 = u32::from_le_bytes(*b"YUYV");

const IOC_WRITE: u64 = 1;
const IOC_READ: u64 = 2;

/// `_IOC(dir, 'V', nr, size)`.
const fn ioc(dir: u64, nr: u64, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | (b'V' as u64) << 8 | nr
}

fn vidioc_querycap() -> u64 {
    ioc(IOC_READ, 0, size_of::<Capability>())
}

fn vidioc_s_fmt() -> u64 {
    ioc(IOC_READ | IOC_WRITE, 5, size_of::<Format>())
}

fn query(file: &File) -> Option<Capability> {
    let mut cap = Capability::default();
    // Safety: VIDIOC_QUERYCAP fills exactly one `v4l2_capability`, which `cap` is.
    let rc = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            vidioc_querycap() as _,
            std::ptr::addr_of_mut!(cap),
        )
    };
    (rc == 0).then_some(cap)
}

fn c_string(bytes: &[u8]) -> &str {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).unwrap_or("")
}

/// A loopback the writer can use: the driver names itself and offers video output.
fn is_loopback(cap: &Capability) -> bool {
    c_string(&cap.driver).starts_with("v4l2 loopback")
        && (cap.capabilities | cap.device_caps) & CAP_VIDEO_OUTPUT != 0
}

/// The first `/dev/video*` whose driver is v4l2loopback and which takes output.
pub fn find_loopback() -> Option<PathBuf> {
    (0..64)
        .map(|n| PathBuf::from(format!("/dev/video{n}")))
        .find(|path| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .ok()
                .and_then(|file| query(&file))
                .is_some_and(|cap| is_loopback(&cap))
        })
}

/// The device with its output format set; frames are written straight to it.
#[derive(Debug)]
pub struct LoopbackSink {
    file: File,
    path: PathBuf,
    width: u32,
    height: u32,
}

impl LoopbackSink {
    pub fn open(path: &Path, width: u32, height: u32) -> Result<Self, Error> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut format = Format {
            type_: BUF_TYPE_VIDEO_OUTPUT,
            fmt: FormatBody {
                pix: PixFormat {
                    width,
                    height,
                    pixelformat: PIX_FMT_YUYV,
                    field: FIELD_NONE,
                    bytesperline: width * 2,
                    sizeimage: width * height * 2,
                    colorspace: COLORSPACE_SMPTE170M,
                    priv_: PIX_FMT_PRIV_MAGIC,
                    ..PixFormat::default()
                },
                rest: [0; 200 - size_of::<PixFormat>()],
                align: [],
            },
        };
        // Safety: VIDIOC_S_FMT reads and writes exactly one `v4l2_format`.
        let rc = unsafe {
            libc::ioctl(
                file.as_raw_fd(),
                vidioc_s_fmt() as _,
                std::ptr::addr_of_mut!(format),
            )
        };
        if rc != 0 {
            return Err(Error::Io(std::io::Error::last_os_error()));
        }
        Ok(Self {
            file,
            path: path.to_path_buf(),
            width,
            height,
        })
    }
}

impl Sink for LoopbackSink {
    fn describe(&self) -> String {
        format!(
            "Camera · {} · {}×{}",
            self.path.display(),
            self.width,
            self.height
        )
    }

    fn push(&mut self, frame: &Frame) -> Result<(), Error> {
        if frame.width != self.width || frame.height != self.height {
            return Err(Error::Frame(format!(
                "frame is {}×{}, the device was set to {}×{}",
                frame.width, frame.height, self.width, self.height
            )));
        }
        let packed = rgba_to_yuyv(frame.width, frame.height, &frame.rgba);
        self.file.write_all(&packed)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_structs_and_ioctls_match_the_kernel_header() {
        assert_eq!(size_of::<PixFormat>(), 48);
        assert_eq!(size_of::<Capability>(), 104);
        assert_eq!(size_of::<FormatBody>(), 200);
        assert_eq!(std::mem::offset_of!(PixFormat, priv_), 28);
        assert_eq!(std::mem::offset_of!(PixFormat, xfer_func), 44);
        assert_eq!(std::mem::offset_of!(Capability, device_caps), 88);
        assert_eq!(vidioc_querycap(), 0x8068_5600);
        assert_eq!(PIX_FMT_YUYV, 0x5659_5559);
        if size_of::<usize>() == 8 {
            assert_eq!(size_of::<Format>(), 208);
            assert_eq!(std::mem::offset_of!(Format, fmt), 8);
            assert_eq!(vidioc_s_fmt(), 0xc0d0_5605);
        } else {
            assert_eq!(size_of::<Format>(), 204);
            assert_eq!(std::mem::offset_of!(Format, fmt), 4);
        }
    }

    #[test]
    fn only_a_loopback_that_takes_output_is_a_camera() {
        let mut cap = Capability::default();
        cap.driver[..13].copy_from_slice(b"v4l2 loopback");
        cap.device_caps = CAP_VIDEO_OUTPUT;
        assert!(is_loopback(&cap));
        cap.device_caps = 0;
        cap.capabilities = CAP_VIDEO_OUTPUT | 0x1;
        assert!(is_loopback(&cap));
        cap.capabilities = 0x1;
        assert!(
            !is_loopback(&cap),
            "a capture-only device is someone's webcam"
        );
        let mut webcam = Capability::default();
        webcam.driver[..5].copy_from_slice(b"uvcvi");
        webcam.device_caps = CAP_VIDEO_OUTPUT;
        assert!(!is_loopback(&webcam));
    }
}
