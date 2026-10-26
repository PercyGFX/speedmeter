//! Running as a Windows service, and installing or removing it.
//!
//! `--install` (run elevated) copies this exe to Program Files, where only
//! administrators can change it, and registers it to start with Windows under the
//! Local Service account with only two privileges. `--uninstall` undoes that.

use crate::appnet::{SERVICE_EXE, SERVICE_NAME};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::{Duration, Instant};
use windows::core::{w, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_CALL_NOT_IMPLEMENTED, ERROR_SERVICE_DOES_NOT_EXIST, NO_ERROR};
use windows::Win32::System::Services::*;

pub static STOP: AtomicBool = AtomicBool::new(false);
static STATUS: AtomicIsize = AtomicIsize::new(0);

const DISPLAY: PCWSTR = w!("SpeedMeter per-app counter");
const DESCRIPTION: &str = "Counts how much data each app sends and receives, for SpeedMeter. \
Runs as Local Service and shares only byte counts per app, never addresses or content.";

fn report(state: SERVICE_STATUS_CURRENT_STATE, exit: u32) {
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN } else { 0 },
        dwWin32ExitCode: exit,
        dwWaitHint: 5000,
        ..Default::default()
    };
    let h = SERVICE_STATUS_HANDLE(STATUS.load(Ordering::Acquire) as *mut _);
    unsafe {
        let _ = SetServiceStatus(h, &status);
    }
}

unsafe extern "system" fn handler(control: u32, _: u32, _: *mut core::ffi::c_void, _: *mut core::ffi::c_void) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            report(SERVICE_STOP_PENDING, 0);
            STOP.store(true, Ordering::Release);
            NO_ERROR.0
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR.0,
        _ => ERROR_CALL_NOT_IMPLEMENTED.0,
    }
}

unsafe extern "system" fn service_main(_: u32, _: *mut PWSTR) {
    let Ok(h) = RegisterServiceCtrlHandlerExW(SERVICE_NAME, Some(handler), None) else { return };
    STATUS.store(h.0 as isize, Ordering::Release);
    report(SERVICE_RUNNING, 0);
    crate::run(&STOP, None);
    report(SERVICE_STOPPED, 0);
}

/// Hands control to the service manager. Returns false if this wasn't started as a service.
pub fn dispatch() -> bool {
    let mut name: Vec<u16> = "SpeedMeterApps\0".encode_utf16().collect();
    let table = [
        SERVICE_TABLE_ENTRYW { lpServiceName: PWSTR(name.as_mut_ptr()), lpServiceProc: Some(service_main) },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    unsafe { StartServiceCtrlDispatcherW(table.as_ptr()).is_ok() }
}

fn install_dir() -> PathBuf {
    let base = std::env::var_os("ProgramW6432").or_else(|| std::env::var_os("ProgramFiles")).unwrap_or_else(|| "C:\\Program Files".into());
    PathBuf::from(base).join("SpeedMeter")
}

struct Handle(SC_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

fn err_code(e: &windows::core::Error) -> u32 {
    (e.code().0 as u32) & 0xFFFF
}

fn stop_and_wait(svc: &Handle) {
    unsafe {
        let mut st = SERVICE_STATUS::default();
        let _ = ControlService(svc.0, SERVICE_CONTROL_STOP, &mut st);
        let until = Instant::now() + Duration::from_secs(15);
        while Instant::now() < until {
            if QueryServiceStatus(svc.0, &mut st).is_err() || st.dwCurrentState == SERVICE_STOPPED {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// Copies the exe to Program Files and registers and starts the service (or
/// updates it). Returns a Win32 error code, 0 on success.
pub fn install() -> u32 {
    match try_install() {
        Ok(()) => 0,
        Err(e) => e,
    }
}

fn try_install() -> Result<(), u32> {
    unsafe {
        let scm = Handle(OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE).map_err(|e| err_code(&e))?);
        let existing = OpenServiceW(scm.0, SERVICE_NAME, SERVICE_ALL_ACCESS).ok().map(Handle);
        if let Some(svc) = &existing {
            stop_and_wait(svc);
        }

        let dir = install_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.raw_os_error().unwrap_or(5) as u32)?;
        let target = dir.join(SERVICE_EXE);
        let source = std::env::current_exe().map_err(|_| 2u32)?;
        if !same_file(&source, &target) {
            // The old exe can stay locked for a moment after the service stops.
            let mut tries = 0;
            loop {
                match std::fs::copy(&source, &target) {
                    Ok(_) => break,
                    Err(_) if tries < 20 => {
                        tries += 1;
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    Err(e) => return Err(e.raw_os_error().unwrap_or(5) as u32),
                }
            }
        }

        let command = HSTRING::from(format!("\"{}\"", target.display()));
        let svc = match existing {
            Some(svc) => {
                ChangeServiceConfigW(
                    svc.0,
                    ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                    SERVICE_AUTO_START,
                    SERVICE_ERROR(SERVICE_NO_CHANGE),
                    &command,
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    w!("NT AUTHORITY\\LocalService"),
                    PCWSTR::null(),
                    DISPLAY,
                )
                .map_err(|e| err_code(&e))?;
                svc
            }
            None => Handle(
                CreateServiceW(
                    scm.0,
                    SERVICE_NAME,
                    DISPLAY,
                    SERVICE_ALL_ACCESS,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    &command,
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    w!("NT AUTHORITY\\LocalService"),
                    PCWSTR::null(),
                )
                .map_err(|e| err_code(&e))?,
            ),
        };

        let mut desc: Vec<u16> = DESCRIPTION.encode_utf16().chain(std::iter::once(0)).collect();
        let info = SERVICE_DESCRIPTIONW { lpDescription: PWSTR(desc.as_mut_ptr()) };
        let _ = ChangeServiceConfig2W(svc.0, SERVICE_CONFIG_DESCRIPTION, Some(&info as *const _ as *const _));

        // Only what it needs: create the shared table in the global namespace.
        // Everything else Local Service normally holds (impersonation, for one) is dropped.
        let mut privs: Vec<u16> = "SeCreateGlobalPrivilege\0SeChangeNotifyPrivilege\0\0".encode_utf16().collect();
        let info = SERVICE_REQUIRED_PRIVILEGES_INFOW { pmszRequiredPrivileges: PWSTR(privs.as_mut_ptr()) };
        let _ = ChangeServiceConfig2W(svc.0, SERVICE_CONFIG_REQUIRED_PRIVILEGES_INFO, Some(&info as *const _ as *const _));

        // Restart after a crash: 10 s, 10 s, then 60 s.
        let mut actions = [
            SC_ACTION { Type: SC_ACTION_RESTART, Delay: 10_000 },
            SC_ACTION { Type: SC_ACTION_RESTART, Delay: 10_000 },
            SC_ACTION { Type: SC_ACTION_RESTART, Delay: 60_000 },
        ];
        let info = SERVICE_FAILURE_ACTIONSW {
            dwResetPeriod: 86_400,
            lpRebootMsg: PWSTR::null(),
            lpCommand: PWSTR::null(),
            cActions: actions.len() as u32,
            lpsaActions: actions.as_mut_ptr(),
        };
        let _ = ChangeServiceConfig2W(svc.0, SERVICE_CONFIG_FAILURE_ACTIONS, Some(&info as *const _ as *const _));

        StartServiceW(svc.0, None).map_err(|e| err_code(&e))?;
        Ok(())
    }
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Stops and removes the service and its files. Returns a Win32 error code, 0 on success.
pub fn uninstall() -> u32 {
    unsafe {
        let scm = match OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) {
            Ok(h) => Handle(h),
            Err(e) => return err_code(&e),
        };
        match OpenServiceW(scm.0, SERVICE_NAME, SERVICE_STOP | SERVICE_QUERY_STATUS | DELETE) {
            Ok(h) => {
                let svc = Handle(h);
                stop_and_wait(&svc);
                if let Err(e) = DeleteService(svc.0) {
                    return err_code(&e);
                }
            }
            Err(e) if err_code(&e) == ERROR_SERVICE_DOES_NOT_EXIST.0 => {}
            Err(e) => return err_code(&e),
        }
    }
    let dir = install_dir();
    let target = dir.join(SERVICE_EXE);
    for _ in 0..20 {
        if !target.exists() || std::fs::remove_file(&target).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    // Only removes the folder if nothing else is in it.
    let _ = std::fs::remove_dir(&dir);
    0
}

const DELETE: u32 = 0x0001_0000;
