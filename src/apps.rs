//! Per-app data usage, counted live.
//!
//! The counting itself happens in speedmeter-service.exe (see src/service), which
//! publishes running byte totals per process and network adapter. Here the meter:
//!
//! * reads those totals every second (`Tracker`), keeps only the adapters it is
//!   measuring and the signed-in user's own processes plus Windows services, and
//!   turns process ids into apps: an exe path, a group of Windows services, or System;
//! * names apps and finds their icons (`Resolver`);
//! * checks, installs and removes the service (`service_*`).
//!
//! Traffic that stays on the PC (localhost) is never counted. Only app names and
//! byte counts are stored, never addresses.

use crate::appnet::{self, Snapshot};
use crate::sys;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::core::{w, HSTRING, PCWSTR, PWSTR};
use windows::ApplicationModel::Package;
use windows::Foundation::Uri;
use windows::Management::Deployment::PackageManager;
use windows::Win32::Foundation::{CloseHandle, FILETIME, MAX_PATH};
use windows::Win32::Storage::FileSystem::{
    FindClose, FindFirstFileW, GetFileVersionInfoSizeW, GetFileVersionInfoW, QueryDosDeviceW, VerQueryValueW,
    WIN32_FIND_DATAW,
};
use windows::Win32::System::Services::{
    CloseServiceHandle, EnumServicesStatusExW, GetServiceDisplayNameW, OpenSCManagerW, OpenServiceW, QueryServiceStatus, ENUM_SERVICE_STATUS_PROCESSW,
    SC_ENUM_PROCESS_INFO, SC_MANAGER_CONNECT, SC_MANAGER_ENUMERATE_SERVICE, SERVICE_ACTIVE, SERVICE_QUERY_STATUS,
    SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_WIN32,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetExitCodeProcess, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
    INFINITE, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

/// Bytes one app moved since the previous reading.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppDelta {
    /// What identifies the app in the history: see `Resolver::describe`.
    pub key: String,
    /// A better spelled path or a display name, used when the app is seen first.
    pub hint: Option<String>,
    pub rx: u64,
    pub tx: u64,
}

// ---------------------------------------------------------------------------
// Keys
//
// * an exe path, lowercased: `c:\program files\mozilla firefox\firefox.exe`
// * Windows services sharing one process: `svc:bits,wuauserv`
// * `system` for the kernel itself, `` (empty) for bytes no app accounts for
// * just a file name (`app.exe`) when the full path couldn't be found

pub const OTHER: &str = "";
pub const SYSTEM: &str = "system";

fn is_path(key: &str) -> bool {
    key.contains(":\\") || key.starts_with("\\\\")
}

/// Text for apps that aren't a file: what to call them, a note, and a Fluent icon.
pub fn special(key: &str) -> Option<(&'static str, &'static str, char)> {
    match key {
        OTHER => Some(("Other", "Bytes no app accounts for: packet headers, other users and short-lived system traffic.", '\u{E9CE}')),
        SYSTEM => Some(("System", "Windows itself: drivers and file sharing.", '\u{E770}')),
        k if k.starts_with("svc:") => Some(("Windows services", "Windows services sharing one process.", '\u{E713}')),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Live counting

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// The service isn't installed or isn't running.
    Off,
    /// It runs but couldn't start counting.
    Error(String),
    Counting,
}

/// Reads the service's totals and turns them into per-app bytes.
pub struct Tracker {
    reader: Option<appnet::Reader>,
    next_open: Instant,
    snap: Snapshot,
    generation: Option<u64>,
    /// Last totals per (pid, created, adapter), and when they were seen.
    prev: HashMap<(u32, u64, u64), (u64, u64, Instant)>,
    keys: HashMap<(u32, u64), (String, Option<String>, Instant)>,
    resolver: Resolver,
    services: Option<(Instant, HashMap<u32, Vec<(String, String)>>)>,
    own_session: u32,
    stale_since: Option<Instant>,
    pub status: Status,
}

impl Tracker {
    pub fn new() -> Self {
        let mut own_session = u32::MAX;
        unsafe {
            let _ = ProcessIdToSessionId(GetCurrentProcessId(), &mut own_session);
        }
        Self {
            reader: None,
            next_open: Instant::now(),
            snap: Snapshot::default(),
            generation: None,
            prev: HashMap::new(),
            keys: HashMap::new(),
            resolver: Resolver::new(),
            services: None,
            own_session,
            stale_since: None,
            status: Status::Off,
        }
    }

    /// Per-app bytes since the last call, for the adapters in `luids`. None while
    /// the service isn't counting.
    pub fn tick(&mut self, luids: &[u64]) -> Option<Vec<AppDelta>> {
        if self.reader.is_none() {
            if Instant::now() < self.next_open {
                return None;
            }
            self.reader = appnet::Reader::open();
            self.next_open = Instant::now() + Duration::from_secs(5);
            if self.reader.is_none() {
                self.status = Status::Off;
                return None;
            }
        }
        let reader = self.reader.as_ref()?;
        if !reader.read(&mut self.snap) {
            return None;
        }
        if self.snap.error != 0 {
            self.status = Status::Error(self.snap.error_text.clone());
            return None;
        }
        if !self.snap.is_fresh() {
            // Stopped. Let go of the table so a restarted service starts a fresh one.
            let since = *self.stale_since.get_or_insert_with(Instant::now);
            if since.elapsed() > Duration::from_secs(10) {
                self.reader = None;
                self.status = Status::Off;
            }
            return None;
        }
        self.stale_since = None;
        self.status = Status::Counting;

        // The first reading only sets the baseline. After a service restart its
        // totals start from zero, so everything in them is new.
        let counting = match self.generation {
            None => false,
            Some(g) if g != self.snap.generation => {
                self.prev.clear();
                true
            }
            Some(_) => true,
        };
        self.generation = Some(self.snap.generation);

        let now = Instant::now();
        let mut out: HashMap<String, AppDelta> = HashMap::new();
        let entries = std::mem::take(&mut self.snap.entries);
        for e in &entries {
            let id = (e.pid, e.created, e.luid);
            let (prx, ptx) = match self.prev.get(&id) {
                Some((rx, tx, _)) => (*rx, *tx),
                None if counting => (0, 0),
                None => (e.rx, e.tx),
            };
            self.prev.insert(id, (e.rx, e.tx, now));
            let (drx, dtx) = (e.rx.saturating_sub(prx), e.tx.saturating_sub(ptx));
            let ours = e.session == self.own_session || e.session == 0 || e.session == u32::MAX;
            let measured = e.luid == 0 || luids.contains(&e.luid);
            if drx + dtx == 0 || !ours || !measured {
                continue;
            }
            let (key, hint) = self.key_for(e);
            let d = out.entry(key.clone()).or_insert_with(|| AppDelta { key, hint, ..Default::default() });
            d.rx += drx;
            d.tx += dtx;
        }
        self.snap.entries = entries;

        // Forget processes that have been gone for a while.
        let cutoff = Duration::from_secs(600);
        self.prev.retain(|_, (_, _, seen)| seen.elapsed() < cutoff);
        self.keys.retain(|_, (_, _, seen)| seen.elapsed() < cutoff);
        Some(out.into_values().collect())
    }

    fn key_for(&mut self, e: &appnet::Entry) -> (String, Option<String>) {
        if let Some((key, hint, seen)) = self.keys.get_mut(&(e.pid, e.created)) {
            *seen = Instant::now();
            return (key.clone(), hint.clone());
        }
        let image = appnet::wide_str(&e.image);
        let (key, hint) = if e.pid == 0 || e.pid == 4 || image.eq_ignore_ascii_case("system") {
            (SYSTEM.to_string(), None)
        } else {
            let path = if image.starts_with('\\') && !image.starts_with("\\\\") {
                self.resolver.win32_path(&image)
            } else {
                None
            }
            .or_else(|| process_path(e.pid, e.created));
            match path {
                Some(p) if p.file_name().is_some_and(|f| f.eq_ignore_ascii_case("svchost.exe")) => self.service_key(e.pid, &p),
                Some(p) => (p.to_string_lossy().to_lowercase(), Some(p.to_string_lossy().into_owned())),
                None if !image.is_empty() => (image.to_lowercase(), None),
                None => (OTHER.to_string(), None),
            }
        };
        self.keys.insert((e.pid, e.created), (key.clone(), hint.clone(), Instant::now()));
        (key, hint)
    }

    /// A shared service host is named after the services it runs.
    fn service_key(&mut self, pid: u32, path: &Path) -> (String, Option<String>) {
        let stale = self.services.as_ref().is_none_or(|(at, map)| at.elapsed() > Duration::from_secs(600) || (!map.contains_key(&pid) && at.elapsed() > Duration::from_secs(2)));
        if stale {
            self.services = Some((Instant::now(), services_by_process()));
        }
        match self.services.as_ref().and_then(|(_, m)| m.get(&pid)) {
            Some(list) if !list.is_empty() => {
                let mut names: Vec<&str> = list.iter().map(|(n, _)| n.as_str()).collect();
                names.sort_unstable();
                let shown: Vec<&str> = list.iter().map(|(_, d)| d.as_str()).collect();
                (format!("svc:{}", names.join(",")).to_lowercase(), Some(shown.join(", ")))
            }
            _ => (path.to_string_lossy().to_lowercase(), Some(path.to_string_lossy().into_owned())),
        }
    }
}

fn filetime(t: FILETIME) -> u64 {
    (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64
}

/// The exe of a running process, if it's still the one created at `created`.
fn process_path(pid: u32, created: u64) -> Option<PathBuf> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut c, mut x, mut k, mut u) = Default::default();
        let same = created == 0 || (GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u).is_ok() && filetime(c) == created);
        let mut buf = [0u16; MAX_PATH as usize * 2];
        let mut len = buf.len() as u32;
        let ok = same && QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// Running Windows services by process id: (service name, display name).
fn services_by_process() -> HashMap<u32, Vec<(String, String)>> {
    let mut out: HashMap<u32, Vec<(String, String)>> = HashMap::new();
    unsafe {
        let Ok(scm) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE) else { return out };
        let (mut needed, mut count, mut resume) = (0u32, 0u32, 0u32);
        let _ = EnumServicesStatusExW(scm, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_ACTIVE, None, &mut needed, &mut count, Some(&mut resume), PCWSTR::null());
        let mut buf = vec![0u64; (needed as usize).div_ceil(8) + 64];
        let bytes = std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * 8);
        resume = 0;
        if EnumServicesStatusExW(scm, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_ACTIVE, Some(bytes), &mut needed, &mut count, Some(&mut resume), PCWSTR::null()).is_ok() {
            let list = std::slice::from_raw_parts(buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, count as usize);
            for s in list {
                let pid = s.ServiceStatusProcess.dwProcessId;
                if pid != 0 {
                    let name = s.lpServiceName.to_string().unwrap_or_default();
                    let display = s.lpDisplayName.to_string().unwrap_or_default();
                    out.entry(pid).or_default().push((name, display));
                }
            }
        }
        let _ = CloseServiceHandle(scm);
    }
    out
}

// ---------------------------------------------------------------------------
// Names and icons

fn usable(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= 60 && !s.starts_with("ms-resource:")
}

/// `...\WindowsApps\SpotifyAB.SpotifyMusic_1.3.2.0_x64__zpdnekdrzrea0\Spotify.exe`
/// -> `SpotifyAB.SpotifyMusic_zpdnekdrzrea0`, for Microsoft Store apps.
fn package_family(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    let at = lower.find("\\windowsapps\\")? + "\\windowsapps\\".len();
    let full = path[at..].split('\\').next()?;
    let name = full.split('_').next()?;
    let publisher = full.rsplit("__").next().filter(|p| !p.contains('_'))?;
    Some(format!("{name}_{publisher}"))
}

/// Turns keys into names and icon files. Keeps the drive list and package manager
/// between calls.
pub struct Resolver {
    drives: Option<Vec<(String, String)>>,
    packages: Option<Option<PackageManager>>,
}

impl Resolver {
    pub fn new() -> Self {
        Self { drives: None, packages: None }
    }

    /// `\device\harddiskvolume3` -> `C:`, for every drive letter in use.
    fn drives(&mut self) -> &[(String, String)] {
        self.drives.get_or_insert_with(|| {
            let mut out = Vec::new();
            for letter in b'A'..=b'Z' {
                let drive = format!("{}:", letter as char);
                let mut buf = [0u16; 512];
                if unsafe { QueryDosDeviceW(&HSTRING::from(drive.as_str()), Some(&mut buf)) } > 0 {
                    out.push((sys::from_wide(&buf).to_ascii_lowercase(), drive));
                }
            }
            out
        })
    }

    /// `\Device\HarddiskVolume3\Program Files\x.exe` -> `C:\Program Files\x.exe`.
    pub fn win32_path(&mut self, nt: &str) -> Option<PathBuf> {
        let lower = nt.to_ascii_lowercase();
        for (device, drive) in self.drives() {
            if lower.starts_with(device.as_str()) && lower[device.len()..].starts_with('\\') {
                return Some(PathBuf::from(format!("{drive}{}", &nt[device.len()..])));
            }
        }
        const UNC: &str = "\\device\\mup\\";
        lower.starts_with(UNC).then(|| PathBuf::from(format!("\\\\{}", &nt[UNC.len()..])))
    }

    fn package(&mut self, family: &str) -> Option<Package> {
        let pm = self.packages.get_or_insert_with(|| PackageManager::new().ok()).as_ref()?;
        let found = pm.FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(family)).ok()?;
        found.into_iter().next()
    }

    /// What to call an app, and its file name. `hint` is the path with its real
    /// capitalization, or the display names of a group of services.
    pub fn describe(&mut self, key: &str, hint: Option<&str>) -> (String, String) {
        if let Some(list) = key.strip_prefix("svc:") {
            let name = hint
                .filter(|h| !h.is_empty())
                .map(str::to_string)
                .or_else(|| service_display_names(list))
                .unwrap_or_else(|| "Windows services".into());
            return (name, "svchost.exe".into());
        }
        if let Some((name, _, _)) = special(key) {
            return (name.to_string(), String::new());
        }
        if is_path(key) {
            let path = PathBuf::from(hint.unwrap_or(key));
            let file = name_on_disk(&path).or_else(|| path.file_name().map(|f| f.to_string_lossy().into_owned())).unwrap_or_else(|| key.to_string());
            let name = version_name(&path).unwrap_or_else(|| file.clone());
            return (name, file);
        }
        (key.to_string(), key.to_string())
    }

    /// A file the shell can draw as the app's icon: the exe, or a Store app's logo.
    pub fn icon_source(&mut self, key: &str) -> Option<PathBuf> {
        if !is_path(key) {
            return None;
        }
        if let Some(family) = package_family(key) {
            if let Some(logo) = self.package(&family).and_then(|p| p.Logo().ok()).and_then(|u| file_uri_path(&u)).filter(|p| p.exists()) {
                return Some(logo);
            }
        }
        Some(PathBuf::from(key)).filter(|p| p.exists())
    }
}

/// "bits,wuauserv" -> "Background Intelligent Transfer Service, Windows Update".
fn service_display_names(list: &str) -> Option<String> {
    unsafe {
        let scm = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT).ok()?;
        let names: Vec<String> = list
            .split(',')
            .filter_map(|svc| {
                let mut buf = [0u16; 256];
                let mut len = buf.len() as u32;
                GetServiceDisplayNameW(scm, &HSTRING::from(svc), PWSTR(buf.as_mut_ptr()), &mut len).ok()?;
                Some(String::from_utf16_lossy(&buf[..len as usize]))
            })
            .collect();
        let _ = CloseServiceHandle(scm);
        (!names.is_empty()).then(|| names.join(", "))
    }
}

/// `file:///C:/Program%20Files/x.png` -> `C:\Program Files\x.png`.
fn file_uri_path(uri: &Uri) -> Option<PathBuf> {
    if !uri.SchemeName().ok()?.to_string().eq_ignore_ascii_case("file") {
        return None;
    }
    let path = Uri::UnescapeComponent(&uri.Path().ok()?).ok()?.to_string();
    Some(PathBuf::from(path.trim_start_matches('/').replace('/', "\\")))
}

/// The file name with its real capitalization.
fn name_on_disk(path: &Path) -> Option<String> {
    unsafe {
        let mut data = WIN32_FIND_DATAW::default();
        let h = FindFirstFileW(&HSTRING::from(path.as_os_str()), &mut data).ok()?;
        let _ = FindClose(h);
        Some(sys::from_wide(&data.cFileName)).filter(|s| !s.is_empty())
    }
}

/// The exe's own description ("Firefox", "Telegram Desktop"), from its version info.
fn version_name(path: &Path) -> Option<String> {
    unsafe {
        let file = HSTRING::from(path.as_os_str());
        let size = GetFileVersionInfoSizeW(&file, None);
        if size == 0 {
            return None;
        }
        let mut block = vec![0u8; size as usize];
        GetFileVersionInfoW(&file, 0, size, block.as_mut_ptr() as *mut _).ok()?;
        let base = block.as_ptr() as *const core::ffi::c_void;
        let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;

        let mut langs = Vec::new();
        if VerQueryValueW(base, w!("\\VarFileInfo\\Translation"), &mut ptr, &mut len).as_bool() && len >= 4 {
            let pairs = std::slice::from_raw_parts(ptr as *const u16, (len / 2) as usize);
            for pair in pairs.chunks_exact(2) {
                langs.push(format!("{:04x}{:04x}", pair[0], pair[1]));
            }
        }
        langs.extend(["040904b0", "040904e4", "04090000"].map(String::from));

        for field in ["FileDescription", "ProductName"] {
            for lang in &langs {
                let query = HSTRING::from(format!("\\StringFileInfo\\{lang}\\{field}"));
                if VerQueryValueW(base, &query, &mut ptr, &mut len).as_bool() && len > 0 {
                    let text = sys::from_wide(std::slice::from_raw_parts(ptr as *const u16, len as usize));
                    let text = text.trim();
                    if usable(text) {
                        return Some(text.to_string());
                    }
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// The service

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    NotInstalled,
    Stopped,
    Starting,
    Running,
}

pub fn service_state() -> ServiceState {
    unsafe {
        let Ok(scm) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) else { return ServiceState::NotInstalled };
        let state = match OpenServiceW(scm, appnet::SERVICE_NAME, SERVICE_QUERY_STATUS) {
            Ok(svc) => {
                let mut st = SERVICE_STATUS::default();
                let s = match QueryServiceStatus(svc, &mut st) {
                    Ok(()) if st.dwCurrentState == SERVICE_RUNNING => ServiceState::Running,
                    Ok(()) if st.dwCurrentState == SERVICE_START_PENDING => ServiceState::Starting,
                    _ => ServiceState::Stopped,
                };
                let _ = CloseServiceHandle(svc);
                s
            }
            Err(_) => ServiceState::NotInstalled,
        };
        let _ = CloseServiceHandle(scm);
        state
    }
}

/// What the running service reports: its version, and why it isn't counting if it
/// isn't. None if it hasn't published its table.
pub fn service_report() -> Option<(u32, Option<String>)> {
    let r = appnet::Reader::open()?;
    let mut s = Snapshot::default();
    if !r.read(&mut s) {
        return None;
    }
    let error = (s.error != 0).then(|| if s.error_text.is_empty() { format!("Error {}", s.error) } else { s.error_text.clone() });
    Some((s.version, error))
}

/// The service exe that came with this copy of SpeedMeter.
pub fn bundled_service() -> Option<PathBuf> {
    Some(sys::exe_path().parent()?.join(appnet::SERVICE_EXE)).filter(|p| p.exists())
}

/// Installs (or updates) or removes the service. Shows Windows' administrator
/// prompt, then waits for the change to finish. Run it off the UI thread.
pub fn change_service(install: bool) -> Result<(), String> {
    let exe = bundled_service().ok_or("speedmeter-service.exe is missing next to SpeedMeter. Reinstall SpeedMeter to fix this.")?;
    let file = HSTRING::from(exe.as_os_str());
    let args = HSTRING::from(if install { "--install" } else { "--uninstall" });
    unsafe {
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(args.as_ptr()),
            nShow: 0,
            ..Default::default()
        };
        if ShellExecuteExW(&mut info).is_err() {
            return Err("Windows didn't allow the change. It needs an administrator's approval.".into());
        }
        if info.hProcess.is_invalid() {
            return Err("The setup didn't start.".into());
        }
        let _ = WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if code != 0 {
            let msg = windows::core::Error::from(windows::core::HRESULT::from_win32(code)).message();
            return Err(format!("Setup failed: {msg} ({code})"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_app_family() {
        let p = r"C:\Program Files\WindowsApps\SpotifyAB.SpotifyMusic_1.303.264.0_x64__zpdnekdrzrea0\Spotify.exe";
        assert_eq!(package_family(p).as_deref(), Some("SpotifyAB.SpotifyMusic_zpdnekdrzrea0"));
        assert_eq!(package_family(r"C:\Program Files\Mozilla Firefox\firefox.exe"), None);
    }

    #[test]
    fn special_keys() {
        assert_eq!(special(OTHER).map(|s| s.0), Some("Other"));
        assert_eq!(special(SYSTEM).map(|s| s.0), Some("System"));
        assert!(special(r"c:\x\app.exe").is_none());
        let mut r = Resolver::new();
        assert_eq!(r.describe("svc:bits,wuauserv", Some("Background Intelligent Transfer Service, Windows Update")).0, "Background Intelligent Transfer Service, Windows Update");
        assert_eq!(r.describe("app.exe", None), ("app.exe".to_string(), "app.exe".to_string()));
    }

    #[test]
    fn names_a_system_exe() {
        let mut r = Resolver::new();
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let drive = windir[..2].to_string();
        let device = r.drives().iter().find(|(_, d)| d.eq_ignore_ascii_case(&drive)).map(|(dev, _)| dev.clone()).unwrap();
        let nt = format!(r"{device}{}\System32\notepad.exe", &windir[2..]);
        let path = r.win32_path(&nt).unwrap();
        assert!(path.exists(), "{}", path.display());
        let key = path.to_string_lossy().to_lowercase();
        let (name, file) = r.describe(&key, Some(&path.to_string_lossy()));
        assert_eq!(file.to_ascii_lowercase(), "notepad.exe");
        assert!(!name.is_empty());
    }
}
