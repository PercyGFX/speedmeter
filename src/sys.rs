//! Small Windows helpers shared by the monitor and the dashboard.

use std::path::PathBuf;
use windows::core::{w, PCSTR, PCWSTR};
use windows::Win32::Foundation::{BOOL, CloseHandle, HWND, LPARAM, MAX_PATH};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegGetValueW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_DWORD,
    RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub fn copy_wide(dst: &mut [u16], s: &str) {
    let mut n = 0;
    for (d, c) in dst.iter_mut().zip(s.encode_utf16()) {
        *d = c;
        n += 1;
    }
    let end = n.min(dst.len().saturating_sub(1));
    for d in &mut dst[end..] {
        *d = 0;
    }
}

fn reg_dword(path: PCWSTR, name: PCWSTR) -> Option<u32> {
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            path,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r.is_ok().then_some(value)
}

const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

/// The taskbar follows the *system* theme (not the app theme).
pub fn taskbar_is_dark() -> bool {
    let light = reg_dword(PERSONALIZE, w!("SystemUsesLightTheme")).unwrap_or(0) != 0;
    let accent = reg_dword(PERSONALIZE, w!("ColorPrevalence")).unwrap_or(0) != 0;
    !light || accent
}

pub fn apps_are_dark() -> bool {
    reg_dword(PERSONALIZE, w!("AppsUseLightTheme")).unwrap_or(0) == 0
}

/// 0 = left aligned, 1 = centered (the Windows 11 default).
pub fn taskbar_centered() -> bool {
    reg_dword(
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"),
        w!("TaskbarAl"),
    )
    .unwrap_or(1)
        != 0
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("SpeedMeter");

pub fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("speedmeter.exe"))
}

fn autostart_command() -> String {
    format!("\"{}\" --background", exe_path().display())
}

pub fn autostart_value() -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    r.is_ok().then(|| from_wide(&buf))
}

/// Adds or removes the HKCU Run entry. Rewrites it if the exe has moved.
pub fn set_autostart(enable: bool) {
    unsafe {
        if enable {
            let cmd = autostart_command();
            if autostart_value().as_deref() == Some(cmd.as_str()) {
                return;
            }
            let mut key = HKEY::default();
            if RegCreateKeyExW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                0,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
            .is_ok()
            {
                let data = wide(&cmd);
                let bytes = std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
                let _ = RegSetValueExW(key, RUN_VALUE, 0, REG_SZ, Some(bytes));
                let _ = RegCloseKey(key);
            }
        } else if autostart_value().is_some() {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
        }
    }
}

/// Switches Win32 popup menus to dark or light using the (undocumented but stable since
/// 1809) uxtheme ordinals that Explorer itself uses.
pub fn set_menu_theme(dark: bool) {
    unsafe {
        let Ok(lib) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        if let Some(f) = GetProcAddress(lib, PCSTR(135 as *const u8)) {
            let set_preferred_app_mode: extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set_preferred_app_mode(if dark { 2 } else { 3 }); // ForceDark / ForceLight
        }
        if let Some(f) = GetProcAddress(lib, PCSTR(136 as *const u8)) {
            let flush_menu_themes: extern "system" fn() = std::mem::transmute(f);
            flush_menu_themes();
        }
    }
}

fn process_exe(pid: u32) -> Option<PathBuf> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; MAX_PATH as usize * 2];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        ok.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// Finds a visible top-level window with this exact title that belongs to a process
/// running the same executable as us (so a folder named "SpeedMeter" never matches).
pub fn find_own_window(title: &str) -> Option<HWND> {
    struct Search {
        title: String,
        exe: PathBuf,
        own_pid: u32,
        found: Option<HWND>,
    }
    unsafe extern "system" fn cb(hwnd: HWND, lp: LPARAM) -> BOOL {
        let s = &mut *(lp.0 as *mut Search);
        if !IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() {
            return true.into();
        }
        let mut buf = [0u16; 128];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n <= 0 || String::from_utf16_lossy(&buf[..n as usize]) != s.title {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == s.own_pid {
            return true.into();
        }
        match process_exe(pid) {
            Some(p) if p.eq(&s.exe) || p.file_name() == s.exe.file_name() => {
                s.found = Some(hwnd);
                false.into()
            }
            _ => true.into(),
        }
    }
    let mut s = Search {
        title: title.to_string(),
        exe: exe_path(),
        own_pid: std::process::id(),
        found: None,
    };
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut s as *mut Search as isize));
    }
    s.found
}

pub fn bring_to_front(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

pub fn open_in_explorer(path: &std::path::Path) {
    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
}

/// Appends a line to the log file. Used for panics and unexpected failures, since a
/// windowed app has no console.
pub fn log(msg: &str) {
    use std::io::Write;
    let path = crate::config::log_path();
    if std::fs::metadata(&path).map(|m| m.len() > 512 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{}] {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
    }
}
