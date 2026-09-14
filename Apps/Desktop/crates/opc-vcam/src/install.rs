//! The camera component's life on this machine: is it installed, put it in, take it
//! out. What "it" is depends on the platform — a kernel module on Linux, a registered
//! COM source on Windows 11, a camera extension on macOS — and so does what the app can
//! do about it from inside. Everything the operator sees is a [`ComponentReport`].

use std::path::PathBuf;
use std::process::Command;

/// Where the component stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComponentState {
    /// The component is in and a camera can be opened on it.
    Installed,
    /// Nothing there yet; `detail` says what installing does.
    NotInstalled,
    /// This platform has no native camera path; the stream is the way.
    Unsupported,
    /// An install or remove is running.
    Busy,
    /// Not looked yet.
    #[default]
    Unknown,
}

impl ComponentState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Installed => "Installed",
            Self::NotInstalled => "Not installed",
            Self::Unsupported => "Not available here",
            Self::Busy => "Working…",
            Self::Unknown => "Checking…",
        }
    }
}

/// What the Output tab shows about the component.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ComponentReport {
    /// "Linux · v4l2loopback", "Windows 11 · Media Foundation virtual camera",
    /// "macOS · camera extension".
    pub platform: String,
    pub state: ComponentState,
    /// Where it is, what installing would do, or what went wrong.
    pub detail: String,
    /// Whether Install does anything from here.
    pub can_install: bool,
    /// Whether Remove does anything from here.
    pub can_remove: bool,
}

impl ComponentReport {
    fn new(platform: &str, state: ComponentState, detail: impl Into<String>) -> Self {
        Self {
            platform: platform.to_string(),
            state,
            detail: detail.into(),
            can_install: false,
            can_remove: false,
        }
    }

    /// The report while an action runs.
    pub fn busy(&self, what: &str) -> Self {
        Self {
            state: ComponentState::Busy,
            detail: what.to_string(),
            can_install: false,
            can_remove: false,
            ..self.clone()
        }
    }
}

/// Looks at the machine.
pub fn probe() -> ComponentReport {
    platform::probe()
}

/// Installs the component, asking for the operator's permission the platform's way,
/// then looks again.
pub fn install() -> ComponentReport {
    match platform::install() {
        Ok(()) => probe(),
        Err(error) => ComponentReport {
            detail: error,
            ..probe()
        },
    }
}

/// Removes the component, then looks again.
pub fn remove() -> ComponentReport {
    match platform::remove() {
        Ok(()) => probe(),
        Err(error) => ComponentReport {
            detail: error,
            ..probe()
        },
    }
}

/// Opens a URL in the operator's browser, the platform's way. Never waits.
pub fn open_url(url: &str) -> Result<(), String> {
    let mut command = platform::open_command(url);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not open {url}: {error}"))
}

/// Whether a program is on the PATH.
#[allow(dead_code)]
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|dir| {
                let candidate = dir.join(program);
                candidate.is_file() || cfg!(windows) && dir.join(format!("{program}.exe")).is_file()
            })
        })
        .unwrap_or(false)
}

/// Runs a command to completion; the error carries its stderr.
#[allow(dead_code)]
fn run(command: &mut Command) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("{:?} could not start: {error}", command.get_program()))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        format!("{:?} failed ({})", command.get_program(), output.status)
    } else {
        stderr
    })
}

#[allow(dead_code)]
fn beside_exe(file: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(file))
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{on_path, run, ComponentReport, ComponentState};
    use std::path::Path;
    use std::process::Command;

    const PLATFORM: &str = "Linux · v4l2loopback";
    /// Loads the module now and keeps it across reboots.
    pub const INSTALL_SCRIPT: &str = "modprobe v4l2loopback exclusive_caps=1 card_label=OpenPocketCine \
&& printf 'v4l2loopback\\n' > /etc/modules-load.d/openpocketcine.conf \
&& printf 'options v4l2loopback exclusive_caps=1 card_label=OpenPocketCine\\n' > /etc/modprobe.d/openpocketcine.conf";
    pub const REMOVE_SCRIPT: &str = "modprobe -r v4l2loopback; \
rm -f /etc/modules-load.d/openpocketcine.conf /etc/modprobe.d/openpocketcine.conf";
    const MANUAL: &str = "sudo modprobe v4l2loopback exclusive_caps=1 card_label=OpenPocketCine";

    pub fn probe() -> ComponentReport {
        let can_act = on_path("pkexec");
        let mut report = match crate::v4l2::find_loopback() {
            Some(path) => ComponentReport::new(
                PLATFORM,
                ComponentState::Installed,
                format!("{} · module loaded", path.display()),
            ),
            None if Path::new("/sys/module/v4l2loopback").exists() => ComponentReport::new(
                PLATFORM,
                ComponentState::NotInstalled,
                "the module is loaded without an output device — Install reloads it with exclusive_caps=1",
            ),
            None => ComponentReport::new(
                PLATFORM,
                ComponentState::NotInstalled,
                if can_act {
                    "Install loads the module and keeps it across reboots (asks for your password)"
                        .to_string()
                } else {
                    format!("no pkexec here — run: {MANUAL}")
                },
            ),
        };
        report.can_install = can_act && report.state != ComponentState::Installed;
        report.can_remove = can_act && report.state == ComponentState::Installed;
        report
    }

    pub fn install() -> Result<(), String> {
        if !on_path("pkexec") {
            return Err(format!("no pkexec here — run: {MANUAL}"));
        }
        run(Command::new("pkexec").args(["sh", "-c", INSTALL_SCRIPT])).map_err(|error| {
            if error.contains("not found") || error.contains("Module v4l2loopback not found") {
                "v4l2loopback is not installed — add the v4l2loopback-dkms package, then Install again".to_string()
            } else {
                error
            }
        })
    }

    pub fn remove() -> Result<(), String> {
        if !on_path("pkexec") {
            return Err("no pkexec here — run: sudo modprobe -r v4l2loopback".to_string());
        }
        run(Command::new("pkexec").args(["sh", "-c", REMOVE_SCRIPT]))
    }

    pub fn open_command(url: &str) -> Command {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    }
}

#[cfg(windows)]
mod platform {
    use super::{beside_exe, ComponentReport, ComponentState};
    use std::path::PathBuf;
    use std::process::Command;
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    const PLATFORM: &str = "Windows 11 · Media Foundation virtual camera";
    const DLL: &str = "opc_vcam_win.dll";
    const SERVER_KEY: &str =
        r"SOFTWARE\Classes\CLSID\{6f0b4c1e-8d7a-4f3b-9c2e-5a1d7e9b3c40}\InprocServer32";
    /// Windows 11 22H2, the first with `MFCreateVirtualCamera`.
    const FIRST_BUILD: u32 = 22621;

    fn registry_string(path: &str, value: Option<&str>) -> Option<String> {
        let path = HSTRING::from(path);
        let value = value.map(HSTRING::from);
        let value = value
            .as_ref()
            .map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr()));
        let mut size = 0u32;
        // Safety: a size probe, then a read into a buffer of that size.
        unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(path.as_ptr()),
                value,
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size),
            )
            .ok()
            .ok()?;
            let mut buffer = vec![0u16; (size as usize).div_ceil(2).max(1)];
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(path.as_ptr()),
                value,
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
            .ok()
            .ok()?;
            let length = (size as usize / 2).saturating_sub(1).min(buffer.len());
            Some(String::from_utf16_lossy(&buffer[..length]))
        }
    }

    fn build_number() -> Option<u32> {
        registry_string(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            Some("CurrentBuildNumber"),
        )?
        .trim()
        .parse()
        .ok()
    }

    fn registered() -> Option<PathBuf> {
        registry_string(SERVER_KEY, None).map(PathBuf::from)
    }

    pub fn probe() -> ComponentReport {
        if build_number().is_some_and(|build| build < FIRST_BUILD) {
            return ComponentReport::new(
                PLATFORM,
                ComponentState::Unsupported,
                "needs Windows 11 22H2 or later — use the stream with OBS's virtual camera",
            );
        }
        let shipped = beside_exe(DLL).filter(|path| path.exists());
        let mut report = match registered() {
            Some(path) if path.exists() => ComponentReport::new(
                PLATFORM,
                ComponentState::Installed,
                format!("registered · {}", path.display()),
            ),
            Some(path) => ComponentReport::new(
                PLATFORM,
                ComponentState::NotInstalled,
                format!(
                    "registered at {} but the file is gone — Install again",
                    path.display()
                ),
            ),
            None => ComponentReport::new(
                PLATFORM,
                ComponentState::NotInstalled,
                match &shipped {
                    Some(path) => format!(
                        "Install registers {} (asks for administrator approval)",
                        path.display()
                    ),
                    None => format!(
                        "{DLL} is not beside the viewfinder — build the workspace and stage it"
                    ),
                },
            ),
        };
        report.can_install = shipped.is_some() && report.state != ComponentState::Installed;
        report.can_remove = report.state == ComponentState::Installed;
        report
    }

    /// `regsvr32` elevated, waited for. The UAC prompt is the operator's consent.
    fn regsvr32(arguments: &str) -> Result<(), String> {
        let file = HSTRING::from("regsvr32.exe");
        let verb = HSTRING::from("runas");
        let parameters = HSTRING::from(arguments);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(parameters.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        // Safety: the strings outlive the call; the process handle is closed below.
        unsafe {
            ShellExecuteExW(&mut info).map_err(|error| {
                if error.code() == ERROR_CANCELLED.to_hresult() {
                    "the administrator prompt was declined".to_string()
                } else {
                    error.message()
                }
            })?;
            let process = info.hProcess;
            WaitForSingleObject(process, INFINITE);
            let mut code = 0u32;
            let _ = GetExitCodeProcess(process, &mut code);
            let _ = CloseHandle(process);
            if code != 0 {
                return Err(format!("regsvr32 failed with code {code}"));
            }
        }
        Ok(())
    }

    pub fn install() -> Result<(), String> {
        let dll = beside_exe(DLL)
            .filter(|path| path.exists())
            .ok_or_else(|| format!("{DLL} is not beside the viewfinder"))?;
        regsvr32(&format!("/s \"{}\"", dll.display()))
    }

    pub fn remove() -> Result<(), String> {
        let dll = registered().ok_or_else(|| "nothing is registered".to_string())?;
        regsvr32(&format!("/u /s \"{}\"", dll.display()))
    }

    pub fn open_command(url: &str) -> Command {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler").arg(url);
        command
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{run, ComponentReport, ComponentState};
    use std::path::Path;
    use std::process::Command;

    const PLATFORM: &str = "macOS · camera extension";
    const HOST_BUNDLE: &str = "com.opencapture.openpocketcine.camera";
    const HOST_APP: &str = "/Applications/OpenPocketCine Camera.app";

    #[cfg(opc_core_linked)]
    fn present() -> Option<bool> {
        // Safety: a plain query on the facade.
        Some(unsafe { opc_core_sys::opc_vcam_mac_present() } == 1)
    }

    #[cfg(not(opc_core_linked))]
    fn present() -> Option<bool> {
        None
    }

    pub fn probe() -> ComponentReport {
        let host = Path::new(HOST_APP).exists();
        let mut report = match present() {
            Some(true) => ComponentReport::new(
                PLATFORM,
                ComponentState::Installed,
                "the OpenPocketCine camera extension is active",
            ),
            Some(false) if host => ComponentReport::new(
                PLATFORM,
                ComponentState::NotInstalled,
                "Install opens OpenPocketCine Camera; press Install there and approve the extension in System Settings",
            ),
            Some(false) => ComponentReport::new(
                PLATFORM,
                ComponentState::NotInstalled,
                "OpenPocketCine Camera.app is not in /Applications — build Apps/Desktop/macos and drop it there",
            ),
            None => ComponentReport::new(
                PLATFORM,
                ComponentState::Unknown,
                "this build has no Swift core, which the camera goes through",
            ),
        };
        report.can_install = host && report.state == ComponentState::NotInstalled;
        report.can_remove = host && report.state == ComponentState::Installed;
        report
    }

    fn open_host() -> Result<(), String> {
        run(Command::new("open").args(["-b", HOST_BUNDLE])).map_err(|_| {
            "OpenPocketCine Camera.app is not installed — build Apps/Desktop/macos and drop it in /Applications".to_string()
        })
    }

    pub fn install() -> Result<(), String> {
        open_host()
    }

    pub fn remove() -> Result<(), String> {
        open_host().map_err(|error| format!("{error} (press Remove there)"))
    }

    pub fn open_command(url: &str) -> Command {
        let mut command = Command::new("open");
        command.arg(url);
        command
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod platform {
    use super::{ComponentReport, ComponentState};
    use std::process::Command;

    pub fn probe() -> ComponentReport {
        ComponentReport::new(
            "this platform",
            ComponentState::Unsupported,
            "no native camera here — use the stream with OBS's virtual camera",
        )
    }

    pub fn install() -> Result<(), String> {
        Err("no native camera on this platform".to_string())
    }

    pub fn remove() -> Result<(), String> {
        Err("no native camera on this platform".to_string())
    }

    pub fn open_command(url: &str) -> Command {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_reads_its_state_and_goes_busy_for_an_action() {
        let report = ComponentReport::new("here", ComponentState::NotInstalled, "nothing yet");
        assert_eq!(report.state.label(), "Not installed");
        let busy = report.busy("Installing…");
        assert_eq!(busy.state, ComponentState::Busy);
        assert_eq!(busy.detail, "Installing…");
        assert!(!busy.can_install && !busy.can_remove);
        assert_eq!(busy.platform, "here");
        assert_eq!(ComponentState::default().label(), "Checking…");
    }

    #[test]
    fn the_probe_answers_for_this_machine() {
        let report = probe();
        assert!(!report.platform.is_empty());
        assert!(!report.detail.is_empty());
        // Nowhere is the component both installable and removable at once.
        assert!(!(report.can_install && report.can_remove));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_linux_scripts_load_the_module_the_way_the_docs_say() {
        assert!(platform::INSTALL_SCRIPT.contains("exclusive_caps=1"));
        assert!(platform::INSTALL_SCRIPT.contains("card_label=OpenPocketCine"));
        assert!(platform::INSTALL_SCRIPT.contains("/etc/modules-load.d/openpocketcine.conf"));
        assert!(platform::REMOVE_SCRIPT.starts_with("modprobe -r v4l2loopback"));
        assert!(!on_path("no-such-program-anywhere"));
    }
}
