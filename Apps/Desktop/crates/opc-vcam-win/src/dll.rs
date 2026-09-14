//! The DLL's face to COM: the exports, the class factory and the registry entries.

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

use windows::core::{implement, Interface, Ref, Result, BOOL, GUID, HRESULT, HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_POINTER, HINSTANCE, S_FALSE, S_OK,
};
use windows::Win32::Media::MediaFoundation::IMFMediaSourceEx;
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE,
    KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

use crate::source::MediaSource;

/// `{6f0b4c1e-8d7a-4f3b-9c2e-5a1d7e9b3c40}`, the text in [`crate::CLSID_TEXT`].
pub const CLSID: GUID = GUID::from_u128(0x6f0b_4c1e_8d7a_4f3b_9c2e_5a1d_7e9b_3c40);

const DLL_PROCESS_ATTACH: u32 = 1;
const REGISTRY_PATH: &str = r"SOFTWARE\Classes\CLSID\{6f0b4c1e-8d7a-4f3b-9c2e-5a1d7e9b3c40}";

static MODULE: AtomicIsize = AtomicIsize::new(0);
static LOCKS: AtomicUsize = AtomicUsize::new(0);

/// Remembers the module handle, for the path the registration writes.
#[no_mangle]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module.0 as isize, Ordering::SeqCst);
    }
    BOOL(1)
}

/// The class factory for the source, and nothing else.
///
/// # Safety
/// `rclsid`, `riid` and `ppv` are COM's out-parameters and are only read or written
/// when non-null.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if rclsid.is_null() || riid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    // Safety: checked non-null above.
    unsafe {
        *ppv = std::ptr::null_mut();
        if *rclsid != CLSID {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = Factory.into();
        factory.query(riid, ppv)
    }
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if LOCKS.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    match register() {
        Ok(()) => S_OK,
        Err(error) => error.code(),
    }
}

#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    let path = HSTRING::from(REGISTRY_PATH);
    // Safety: a plain registry call with a live string.
    unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(path.as_ptr())) }
        .ok()
        .map_or_else(|error| error.code(), |()| S_OK)
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, windows::core::IUnknown>,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> Result<()> {
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let source: IMFMediaSourceEx = MediaSource::new()?.into();
        // Safety: COM hands us its own out-parameters.
        unsafe { source.query(riid, ppv) }.ok()
    }

    fn LockServer(&self, lock: BOOL) -> Result<()> {
        if lock.as_bool() {
            LOCKS.fetch_add(1, Ordering::SeqCst);
        } else {
            LOCKS.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// Where this DLL is on disk, for `InprocServer32`.
fn module_path() -> Result<String> {
    let module = MODULE.load(Ordering::SeqCst);
    let mut buffer = vec![0u16; 1024];
    // Safety: the buffer is ours and the length is its capacity.
    let length = unsafe {
        GetModuleFileNameW(
            Some(windows::Win32::Foundation::HMODULE(module as *mut c_void)),
            &mut buffer,
        )
    };
    if length == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(String::from_utf16_lossy(&buffer[..length as usize]))
}

fn set_string(key: HKEY, name: Option<&str>, value: &str) -> Result<()> {
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes: &[u8] = bytemuck_u16(&wide);
    let name = name.map(HSTRING::from);
    let name = name
        .as_ref()
        .map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr()));
    // Safety: the key is open and the bytes are a terminated UTF-16 string.
    unsafe { RegSetValueExW(key, name, None, REG_SZ, Some(bytes)) }.ok()
}

fn bytemuck_u16(words: &[u16]) -> &[u8] {
    // Safety: u16 to bytes over the same memory; the slice keeps its lifetime.
    unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * 2) }
}

fn create_key(path: &str) -> Result<HKEY> {
    let path = HSTRING::from(path);
    let mut key = HKEY::default();
    // Safety: plain registry call; `key` receives the handle.
    unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
    }
    .ok()?;
    Ok(key)
}

/// `HKLM\SOFTWARE\Classes\CLSID\{…}`: the name, `InprocServer32` with this DLL's path
/// and a `Both` threading model.
fn register() -> Result<()> {
    let path = module_path()?;
    let key = create_key(REGISTRY_PATH)?;
    let outcome = set_string(key, None, "OpenPocketCine Virtual Camera");
    // Safety: closing what we opened.
    unsafe {
        let _ = RegCloseKey(key);
    }
    outcome?;
    let server = create_key(&format!(r"{REGISTRY_PATH}\InprocServer32"))?;
    let outcome = set_string(server, None, &path)
        .and_then(|()| set_string(server, Some("ThreadingModel"), "Both"));
    // Safety: closing what we opened.
    unsafe {
        let _ = RegCloseKey(server);
    }
    outcome
}
