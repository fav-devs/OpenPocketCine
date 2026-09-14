//! The media source the Frame Server talks to: one stream, live, no seeking.

use std::ffi::c_void;
use std::sync::Mutex;

use windows::core::{implement, IUnknownImpl, Interface, Ref, Result, GUID, HRESULT};
use windows::Win32::Foundation::{E_POINTER, S_OK};
use windows::Win32::Media::KernelStreaming::{IKsControl, IKsControl_Impl, KSIDENTIFIER};
use windows::Win32::Media::MediaFoundation::{
    IMFAsyncCallback, IMFAsyncResult, IMFAttributes, IMFGetService, IMFGetService_Impl,
    IMFMediaEvent, IMFMediaEventGenerator_Impl, IMFMediaEventQueue, IMFMediaSource,
    IMFMediaSourceEx, IMFMediaSourceEx_Impl, IMFMediaSource_Impl, IMFMediaStream2,
    IMFPresentationDescriptor, MENewStream, MESourceStarted, MESourceStopped, MEUpdatedStream,
    MFCreateAttributes, MFCreateEventQueue, MFCreatePresentationDescriptor,
    MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS, MFMEDIASOURCE_IS_LIVE, MF_E_INVALID_STATE_TRANSITION,
    MF_E_SHUTDOWN, MF_E_UNSUPPORTED_SERVICE, MF_E_UNSUPPORTED_TIME_FORMAT,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;

use crate::stream::{ks_not_found, MediaStream, Shared};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Stopped,
    Started,
    Shutdown,
}

#[implement(IMFMediaSourceEx, IMFGetService, IKsControl)]
#[derive(Debug)]
pub struct MediaSource {
    events: IMFMediaEventQueue,
    attributes: IMFAttributes,
    descriptor: IMFPresentationDescriptor,
    stream: IMFMediaStream2,
    shared: std::sync::Arc<Shared>,
    state: Mutex<State>,
    /// Whether the stream has been announced with `MENewStream` yet.
    announced: Mutex<bool>,
}

impl MediaSource {
    pub fn new() -> Result<Self> {
        let events = unsafe { MFCreateEventQueue()? };
        let mut attributes = None;
        unsafe { MFCreateAttributes(&mut attributes, 1)? };
        let attributes = attributes.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
        let (stream, shared) = MediaStream::create()?;
        let descriptor =
            unsafe { MFCreatePresentationDescriptor(Some(&[Some(shared.descriptor.clone())]))? };
        unsafe { descriptor.SelectStream(0)? };
        let source = Self {
            events,
            attributes,
            descriptor,
            stream,
            shared,
            state: Mutex::new(State::Stopped),
            announced: Mutex::new(false),
        };
        Ok(source)
    }

    fn check_alive(&self) -> Result<()> {
        if *self
            .state
            .lock()
            .map_err(|_| windows::core::Error::from(MF_E_SHUTDOWN))?
            == State::Shutdown
        {
            return Err(MF_E_SHUTDOWN.into());
        }
        Ok(())
    }
}

/// The source's own interface, for the stream's `GetMediaSource`.
fn as_source(this: &MediaSource_Impl) -> Result<IMFMediaSource> {
    let ex: IMFMediaSourceEx = this.to_interface();
    ex.cast()
}

impl IMFMediaSource_Impl for MediaSource_Impl {
    fn GetCharacteristics(&self) -> Result<u32> {
        self.check_alive()?;
        Ok(MFMEDIASOURCE_IS_LIVE.0 as u32)
    }

    fn CreatePresentationDescriptor(&self) -> Result<IMFPresentationDescriptor> {
        self.check_alive()?;
        unsafe { self.descriptor.Clone() }
    }

    fn Start(
        &self,
        descriptor: Ref<'_, IMFPresentationDescriptor>,
        time_format: *const GUID,
        start_position: *const PROPVARIANT,
    ) -> Result<()> {
        self.check_alive()?;
        // Safety: a null format means the default; anything else must be GUID_NULL.
        if !time_format.is_null() && unsafe { *time_format } != GUID::zeroed() {
            return Err(MF_E_UNSUPPORTED_TIME_FORMAT.into());
        }
        let descriptor = descriptor
            .as_ref()
            .ok_or_else(|| windows::core::Error::from(E_POINTER))?;
        let mut selected = false.into();
        let mut stream_descriptor = None;
        unsafe { descriptor.GetStreamDescriptorByIndex(0, &mut selected, &mut stream_descriptor)? };
        if selected.as_bool() {
            let mut announced = self
                .announced
                .lock()
                .map_err(|_| windows::core::Error::from(MF_E_SHUTDOWN))?;
            let event = if *announced {
                MEUpdatedStream
            } else {
                MENewStream
            };
            *announced = true;
            let stream: windows::core::IUnknown = self.stream.cast()?;
            unsafe {
                self.events
                    .QueueEventParamUnk(event.0 as u32, &GUID::zeroed(), S_OK, &stream)?;
            }
            self.shared.set_source(Some(as_source(self)?));
            self.shared.start(start_position)?;
        }
        unsafe {
            self.events.QueueEventParamVar(
                MESourceStarted.0 as u32,
                &GUID::zeroed(),
                S_OK,
                start_position,
            )?;
        }
        *self
            .state
            .lock()
            .map_err(|_| windows::core::Error::from(MF_E_SHUTDOWN))? = State::Started;
        Ok(())
    }

    fn Stop(&self) -> Result<()> {
        self.check_alive()?;
        self.shared.stop()?;
        unsafe {
            self.events.QueueEventParamVar(
                MESourceStopped.0 as u32,
                &GUID::zeroed(),
                S_OK,
                std::ptr::null(),
            )?;
        }
        *self
            .state
            .lock()
            .map_err(|_| windows::core::Error::from(MF_E_SHUTDOWN))? = State::Stopped;
        Ok(())
    }

    fn Pause(&self) -> Result<()> {
        self.check_alive()?;
        Err(MF_E_INVALID_STATE_TRANSITION.into())
    }

    fn Shutdown(&self) -> Result<()> {
        if let Ok(mut state) = self.state.lock() {
            if *state == State::Shutdown {
                return Err(MF_E_SHUTDOWN.into());
            }
            *state = State::Shutdown;
        }
        self.shared.shutdown();
        unsafe { self.events.Shutdown() }
    }
}

impl IMFMediaSourceEx_Impl for MediaSource_Impl {
    fn GetSourceAttributes(&self) -> Result<IMFAttributes> {
        self.check_alive()?;
        Ok(self.attributes.clone())
    }

    fn GetStreamAttributes(&self, _stream_id: u32) -> Result<IMFAttributes> {
        self.check_alive()?;
        self.shared.descriptor.cast()
    }

    fn SetD3DManager(&self, _manager: Ref<'_, windows::core::IUnknown>) -> Result<()> {
        self.check_alive()
    }
}

impl IMFMediaEventGenerator_Impl for MediaSource_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        unsafe { self.events.GetEvent(flags.0) }
    }

    fn BeginGetEvent(
        &self,
        callback: Ref<'_, IMFAsyncCallback>,
        state: Ref<'_, windows::core::IUnknown>,
    ) -> Result<()> {
        unsafe { self.events.BeginGetEvent(callback.as_ref(), state.as_ref()) }
    }

    fn EndGetEvent(&self, result: Ref<'_, IMFAsyncResult>) -> Result<IMFMediaEvent> {
        unsafe { self.events.EndGetEvent(result.as_ref()) }
    }

    fn QueueEvent(
        &self,
        event_type: u32,
        extended_type: *const GUID,
        status: HRESULT,
        value: *const PROPVARIANT,
    ) -> Result<()> {
        unsafe {
            self.events
                .QueueEventParamVar(event_type, extended_type, status, value)
        }
    }
}

impl IMFGetService_Impl for MediaSource_Impl {
    fn GetService(
        &self,
        _service: *const GUID,
        _riid: *const GUID,
        _object: *mut *mut c_void,
    ) -> Result<()> {
        Err(MF_E_UNSUPPORTED_SERVICE.into())
    }
}

impl IKsControl_Impl for MediaSource_Impl {
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
