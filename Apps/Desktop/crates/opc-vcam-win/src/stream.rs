//! The one video stream: NV12, 1280 × 720, 30 frames a second, from the bridge.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use opc_vcam::wire::nv12_len;
use opc_vcam::{HEIGHT, WIDTH};
use windows::core::{implement, Ref, Result, GUID, HRESULT};
use windows::Win32::Foundation::{ERROR_SET_NOT_FOUND, S_OK};
use windows::Win32::Media::KernelStreaming::{
    IKsControl, IKsControl_Impl, KSIDENTIFIER, PINNAME_VIDEO_CAPTURE,
};
use windows::Win32::Media::MediaFoundation::{
    IMFAsyncCallback, IMFAsyncResult, IMFMediaEvent, IMFMediaEventGenerator_Impl,
    IMFMediaEventQueue, IMFMediaSource, IMFMediaStream2, IMFMediaStream2_Impl, IMFMediaStream_Impl,
    IMFMediaType, IMFStreamDescriptor, MEMediaSample, MEStreamStarted, MEStreamStopped,
    MFCreateEventQueue, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateStreamDescriptor, MFFrameSourceTypes_Color, MFGetSystemTime, MFMediaType_Video,
    MFNominalRange_16_235, MFSampleExtension_DeviceTimestamp, MFSampleExtension_Token,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFVideoTransferMatrix_BT601,
    MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS, MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES,
    MF_DEVICESTREAM_FRAMESERVER_SHARED, MF_DEVICESTREAM_STREAM_CATEGORY, MF_DEVICESTREAM_STREAM_ID,
    MF_E_INVALIDREQUEST, MF_E_SHUTDOWN, MF_MT_ALL_SAMPLES_INDEPENDENT, MF_MT_DEFAULT_STRIDE,
    MF_MT_FIXED_SIZE_SAMPLES, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SAMPLE_SIZE, MF_MT_SUBTYPE,
    MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_YUV_MATRIX, MF_STREAM_STATE, MF_STREAM_STATE_PAUSED,
    MF_STREAM_STATE_RUNNING, MF_STREAM_STATE_STOPPED,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;

use crate::bridge::Bridge;
use crate::FRAME_RATE;

/// `HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND)`: the answer to every KS query we do not have.
pub fn ks_not_found() -> windows::core::Error {
    ERROR_SET_NOT_FOUND.to_hresult().into()
}

/// One frame's time in 100 ns units.
fn frame_interval() -> i64 {
    10_000_000 / i64::from(FRAME_RATE)
}

/// Packs two 32-bit halves the way `MFSetAttributeSize` and `MFSetAttributeRatio` do.
fn pack(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

#[derive(Debug)]
struct Pace {
    /// When the next frame is due, so delivery never runs ahead of the frame rate.
    due: Option<Instant>,
}

/// What the source and the stream both reach: the descriptor, the events, the bridge.
#[derive(Debug)]
pub struct Shared {
    pub descriptor: IMFStreamDescriptor,
    events: IMFMediaEventQueue,
    source: Mutex<Option<IMFMediaSource>>,
    state: Mutex<MF_STREAM_STATE>,
    shutdown: Mutex<bool>,
    bridge: Mutex<Option<Bridge>>,
    pace: Mutex<Pace>,
}

// Safety: the Frame Server calls the source and the stream from whichever of its
// threads it likes, as COM allows for a `Both`-model object. The Media Foundation
// objects held here (the event queue, the descriptor, the source) are free-threaded by
// contract; everything else sits behind a mutex.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Shared {
    fn alive(&self) -> Result<()> {
        if self.shutdown.lock().map(|s| *s).unwrap_or(true) {
            return Err(MF_E_SHUTDOWN.into());
        }
        Ok(())
    }

    pub fn set_source(&self, source: Option<IMFMediaSource>) {
        if let Ok(mut inside) = self.source.lock() {
            *inside = source;
        }
    }

    /// Running: the bridge reads the pipe and `MEStreamStarted` goes out.
    pub fn start(&self, position: *const PROPVARIANT) -> Result<()> {
        self.alive()?;
        if let Ok(mut bridge) = self.bridge.lock() {
            if bridge.is_none() {
                *bridge = Some(Bridge::start(WIDTH, HEIGHT));
            }
        }
        if let Ok(mut state) = self.state.lock() {
            *state = MF_STREAM_STATE_RUNNING;
        }
        if let Ok(mut pace) = self.pace.lock() {
            pace.due = None;
        }
        unsafe {
            self.events.QueueEventParamVar(
                MEStreamStarted.0 as u32,
                &GUID::zeroed(),
                S_OK,
                position,
            )
        }
    }

    pub fn stop(&self) -> Result<()> {
        self.alive()?;
        if let Ok(mut state) = self.state.lock() {
            *state = MF_STREAM_STATE_STOPPED;
        }
        if let Ok(mut bridge) = self.bridge.lock() {
            *bridge = None;
        }
        unsafe {
            self.events.QueueEventParamVar(
                MEStreamStopped.0 as u32,
                &GUID::zeroed(),
                S_OK,
                std::ptr::null(),
            )
        }
    }

    pub fn shutdown(&self) {
        if let Ok(mut done) = self.shutdown.lock() {
            *done = true;
        }
        if let Ok(mut bridge) = self.bridge.lock() {
            *bridge = None;
        }
        self.set_source(None);
        unsafe {
            let _ = self.events.Shutdown();
        }
    }

    /// Sleeps until the next frame is due, then marks the one after.
    fn pace(&self) {
        let interval = Duration::from_nanos(frame_interval() as u64 * 100);
        let Ok(mut pace) = self.pace.lock() else {
            return;
        };
        let now = Instant::now();
        let due = pace.due.unwrap_or(now);
        if due > now {
            std::thread::sleep(due - now);
        }
        // From the due time, not from now, so a late wake does not drift the cadence.
        pace.due = Some(due.max(now - interval) + interval);
    }

    /// The newest frame as a sample, timed now.
    fn sample(
        &self,
        token: Option<&windows::core::IUnknown>,
    ) -> Result<windows::Win32::Media::MediaFoundation::IMFSample> {
        let frame = self
            .bridge
            .lock()
            .ok()
            .and_then(|bridge| bridge.as_ref().map(|bridge| bridge.latest().0))
            .unwrap_or_else(|| Arc::new(crate::bridge::black(WIDTH, HEIGHT)));
        let length = nv12_len(WIDTH, HEIGHT) as u32;
        unsafe {
            let sample = MFCreateSample()?;
            let buffer = MFCreateMemoryBuffer(length)?;
            let mut bytes = std::ptr::null_mut();
            let mut capacity = 0;
            buffer.Lock(&mut bytes, Some(&mut capacity), None)?;
            let count = frame.len().min(capacity as usize);
            std::ptr::copy_nonoverlapping(frame.as_ptr(), bytes, count);
            buffer.Unlock()?;
            buffer.SetCurrentLength(count as u32)?;
            sample.AddBuffer(&buffer)?;
            let now = MFGetSystemTime();
            sample.SetSampleTime(now)?;
            sample.SetSampleDuration(frame_interval())?;
            sample.SetUINT64(&MFSampleExtension_DeviceTimestamp, now as u64)?;
            if let Some(token) = token {
                sample.SetUnknown(&MFSampleExtension_Token, token)?;
            }
            Ok(sample)
        }
    }
}

/// The NV12 type the stream offers.
fn media_type() -> Result<IMFMediaType> {
    unsafe {
        let media_type = MFCreateMediaType()?;
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        media_type.SetUINT64(&MF_MT_FRAME_SIZE, pack(WIDTH, HEIGHT))?;
        media_type.SetUINT64(&MF_MT_FRAME_RATE, pack(FRAME_RATE, 1))?;
        media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        media_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        media_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, WIDTH)?;
        media_type.SetUINT32(&MF_MT_SAMPLE_SIZE, nv12_len(WIDTH, HEIGHT) as u32)?;
        media_type.SetUINT32(&MF_MT_FIXED_SIZE_SAMPLES, 1)?;
        media_type.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT601.0 as u32)?;
        media_type.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        Ok(media_type)
    }
}

/// The descriptor a capture pipeline expects of a camera pin.
fn stream_descriptor(media_type: &IMFMediaType) -> Result<IMFStreamDescriptor> {
    unsafe {
        let descriptor = MFCreateStreamDescriptor(0, &[Some(media_type.clone())])?;
        descriptor
            .GetMediaTypeHandler()?
            .SetCurrentMediaType(media_type)?;
        descriptor.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, 0)?;
        descriptor.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
        descriptor.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
        descriptor.SetUINT32(
            &MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES,
            MFFrameSourceTypes_Color.0 as u32,
        )?;
        Ok(descriptor)
    }
}

#[implement(IMFMediaStream2, IKsControl)]
#[derive(Debug)]
pub struct MediaStream {
    shared: Arc<Shared>,
}

impl MediaStream {
    /// The stream as COM sees it, and the state the source shares with it.
    pub fn create() -> Result<(IMFMediaStream2, Arc<Shared>)> {
        let media_type = media_type()?;
        let descriptor = stream_descriptor(&media_type)?;
        let events = unsafe { MFCreateEventQueue()? };
        let shared = Arc::new(Shared {
            descriptor,
            events,
            source: Mutex::new(None),
            state: Mutex::new(MF_STREAM_STATE_STOPPED),
            shutdown: Mutex::new(false),
            bridge: Mutex::new(None),
            pace: Mutex::new(Pace { due: None }),
        });
        let stream: IMFMediaStream2 = MediaStream {
            shared: shared.clone(),
        }
        .into();
        Ok((stream, shared))
    }
}

impl IMFMediaStream_Impl for MediaStream_Impl {
    fn GetMediaSource(&self) -> Result<IMFMediaSource> {
        self.shared.alive()?;
        self.shared
            .source
            .lock()
            .ok()
            .and_then(|source| source.clone())
            .ok_or_else(|| MF_E_SHUTDOWN.into())
    }

    fn GetStreamDescriptor(&self) -> Result<IMFStreamDescriptor> {
        self.shared.alive()?;
        Ok(self.shared.descriptor.clone())
    }

    fn RequestSample(&self, token: Ref<'_, windows::core::IUnknown>) -> Result<()> {
        self.shared.alive()?;
        if self
            .shared
            .state
            .lock()
            .map(|state| *state)
            .unwrap_or(MF_STREAM_STATE_STOPPED)
            != MF_STREAM_STATE_RUNNING
        {
            return Err(MF_E_INVALIDREQUEST.into());
        }
        self.shared.pace();
        let sample = self.shared.sample(token.as_ref())?;
        unsafe {
            self.shared.events.QueueEventParamUnk(
                MEMediaSample.0 as u32,
                &GUID::zeroed(),
                S_OK,
                &sample,
            )
        }
    }
}

impl IMFMediaStream2_Impl for MediaStream_Impl {
    fn SetStreamState(&self, state: MF_STREAM_STATE) -> Result<()> {
        self.shared.alive()?;
        match state {
            MF_STREAM_STATE_RUNNING => self.shared.start(std::ptr::null()),
            MF_STREAM_STATE_STOPPED => self.shared.stop(),
            MF_STREAM_STATE_PAUSED => {
                if let Ok(mut inside) = self.shared.state.lock() {
                    *inside = MF_STREAM_STATE_PAUSED;
                }
                Ok(())
            }
            _ => Err(MF_E_INVALIDREQUEST.into()),
        }
    }

    fn GetStreamState(&self) -> Result<MF_STREAM_STATE> {
        self.shared.alive()?;
        Ok(self
            .shared
            .state
            .lock()
            .map(|state| *state)
            .unwrap_or(MF_STREAM_STATE_STOPPED))
    }
}

impl IMFMediaEventGenerator_Impl for MediaStream_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        unsafe { self.shared.events.GetEvent(flags.0) }
    }

    fn BeginGetEvent(
        &self,
        callback: Ref<'_, IMFAsyncCallback>,
        state: Ref<'_, windows::core::IUnknown>,
    ) -> Result<()> {
        unsafe {
            self.shared
                .events
                .BeginGetEvent(callback.as_ref(), state.as_ref())
        }
    }

    fn EndGetEvent(&self, result: Ref<'_, IMFAsyncResult>) -> Result<IMFMediaEvent> {
        unsafe { self.shared.events.EndGetEvent(result.as_ref()) }
    }

    fn QueueEvent(
        &self,
        event_type: u32,
        extended_type: *const GUID,
        status: HRESULT,
        value: *const PROPVARIANT,
    ) -> Result<()> {
        unsafe {
            self.shared
                .events
                .QueueEventParamVar(event_type, extended_type, status, value)
        }
    }
}

impl IKsControl_Impl for MediaStream_Impl {
    fn KsProperty(
        &self,
        _property: *const KSIDENTIFIER,
        _property_length: u32,
        _data: *mut c_void,
        _data_length: u32,
        _bytes_returned: *mut u32,
    ) -> Result<()> {
        Err(ks_not_found())
    }

    fn KsMethod(
        &self,
        _method: *const KSIDENTIFIER,
        _method_length: u32,
        _data: *mut c_void,
        _data_length: u32,
        _bytes_returned: *mut u32,
    ) -> Result<()> {
        Err(ks_not_found())
    }

    fn KsEvent(
        &self,
        _event: *const KSIDENTIFIER,
        _event_length: u32,
        _data: *mut c_void,
        _data_length: u32,
        _bytes_returned: *mut u32,
    ) -> Result<()> {
        Err(ks_not_found())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_interval_and_packing_follow_media_foundation() {
        assert_eq!(frame_interval(), 333_333);
        assert_eq!(pack(1280, 720), 0x0000_0500_0000_02D0);
        assert_eq!(ks_not_found().code().0 as u32, 0x8007_0492);
    }
}
