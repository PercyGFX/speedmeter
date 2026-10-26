//! Running totals per process and network adapter, and the shared table that
//! readers copy them from.

use crate::appnet::{set_wide, Entry, Table, CAPACITY, MAGIC, MAPPING, SERVICE_VERSION};
use std::collections::HashMap;
use std::sync::atomic::{fence, Ordering};
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, LocalFree, BOOL, ERROR_BUFFER_OVERFLOW, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, NO_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_FRIENDLY_NAME,
    GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6};
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows::Win32::System::RemoteDesktop::{WTSEnumerateProcessesW, WTSFreeMemory, WTS_CURRENT_SERVER_HANDLE, WTS_PROCESS_INFOW};

/// How long an ended process stays in the table, so readers pick up its last bytes.
const KEEP_ENDED: Duration = Duration::from_secs(120);

struct Proc {
    created: u64,
    session: u32,
    image: String,
    ended: Option<Instant>,
}

#[derive(Default)]
struct Flow {
    rx: u64,
    tx: u64,
    last: Option<Instant>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Addr {
    V4([u8; 4]),
    V6([u8; 16]),
}

impl Addr {
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        match b.len() {
            4 => Some(Addr::V4(b.try_into().ok()?)),
            16 => Some(Addr::V6(b.try_into().ok()?)),
            _ => None,
        }
    }

    /// Traffic that never leaves the PC (127.x, ::1, and IPv4-mapped 127.x).
    fn is_loopback(&self) -> bool {
        match self {
            Addr::V4(a) => a[0] == 127,
            Addr::V6(a) => {
                let mapped_v4 = a[..10].iter().all(|&b| b == 0) && a[10] == 0xff && a[11] == 0xff;
                (a[..15].iter().all(|&b| b == 0) && a[15] == 1) || (mapped_v4 && a[12] == 127)
            }
        }
    }
}

/// Which adapter owns each local address.
struct Adapters {
    by_addr: HashMap<Addr, u64>,
    refreshed: Option<Instant>,
}

impl Adapters {
    fn luid(&mut self, local: Addr) -> u64 {
        if let Some(l) = self.by_addr.get(&local) {
            return *l;
        }
        // A new address (an adapter came up). Look again, at most every two seconds.
        if self.refreshed.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
            self.refresh();
        }
        self.by_addr.get(&local).copied().unwrap_or(0)
    }

    fn refresh(&mut self) {
        self.refreshed = Some(Instant::now());
        let mut size: u32 = 32 * 1024;
        for _ in 0..4 {
            let mut buf: Vec<u64> = vec![0; (size as usize).div_ceil(8)];
            let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER | GAA_FLAG_SKIP_FRIENDLY_NAME;
            let ret = unsafe { GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH), &mut size) };
            if ret == ERROR_BUFFER_OVERFLOW.0 {
                continue;
            }
            if ret != NO_ERROR.0 {
                return;
            }
            self.by_addr.clear();
            let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
            while !p.is_null() {
                let a = unsafe { &*p };
                let luid = unsafe { a.Luid.Value };
                let mut u = a.FirstUnicastAddress;
                while !u.is_null() {
                    let ua = unsafe { &*u };
                    let sa = ua.Address.lpSockaddr;
                    if !sa.is_null() {
                        let family = unsafe { (*sa).sa_family };
                        let addr = if family == AF_INET {
                            let sin = unsafe { &*(sa as *const SOCKADDR_IN) };
                            Some(Addr::V4(unsafe { sin.sin_addr.S_un.S_addr }.to_ne_bytes()))
                        } else if family == AF_INET6 {
                            let sin6 = unsafe { &*(sa as *const SOCKADDR_IN6) };
                            Some(Addr::V6(unsafe { sin6.sin6_addr.u.Byte }))
                        } else {
                            None
                        };
                        if let Some(addr) = addr {
                            self.by_addr.insert(addr, luid);
                        }
                    }
                    u = ua.Next;
                }
                p = a.Next;
            }
            return;
        }
    }
}

pub struct Counters {
    procs: HashMap<u32, Proc>,
    flows: HashMap<(u32, u64, u64), Flow>,
    adapters: Adapters,
    names_checked: Option<Instant>,
}

impl Counters {
    pub fn new() -> Self {
        let mut c = Self {
            procs: HashMap::new(),
            flows: HashMap::new(),
            adapters: Adapters { by_addr: HashMap::new(), refreshed: None },
            names_checked: None,
        };
        c.adapters.refresh();
        c
    }

    /// A process started, or was already running when tracing began.
    pub fn process_seen(&mut self, pid: u32, created: u64, session: u32, image: String) {
        self.procs.insert(pid, Proc { created, session, image, ended: None });
    }

    pub fn process_ended(&mut self, pid: u32, created: u64) {
        if let Some(p) = self.procs.get_mut(&pid) {
            if created == 0 || p.created == 0 || p.created == created {
                p.ended = Some(Instant::now());
            }
        }
    }

    /// `local` is the address on this PC's side of the connection.
    pub fn add(&mut self, pid: u32, local: &[u8], remote: &[u8], bytes: u32, received: bool) {
        let (Some(local), remote) = (Addr::from_bytes(local), Addr::from_bytes(remote)) else { return };
        if local.is_loopback() || remote.is_some_and(|r| r.is_loopback()) {
            return;
        }
        let luid = self.adapters.luid(local);
        if !self.procs.contains_key(&pid) {
            self.learn_names(pid);
        }
        let created = self.procs.get(&pid).map_or(0, |p| p.created);
        let f = self.flows.entry((pid, created, luid)).or_default();
        if received {
            f.rx += bytes as u64;
        } else {
            f.tx += bytes as u64;
        }
        f.last = Some(Instant::now());
    }

    /// Fills in processes that started before tracing did (if Windows' rundown
    /// didn't name them): file name and session, from the session manager.
    fn learn_names(&mut self, pid: u32) {
        if self.names_checked.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
            self.procs.entry(pid).or_insert(Proc { created: 0, session: u32::MAX, image: String::new(), ended: None });
            return;
        }
        self.names_checked = Some(Instant::now());
        unsafe {
            let mut info: *mut WTS_PROCESS_INFOW = std::ptr::null_mut();
            let mut count = 0u32;
            if WTSEnumerateProcessesW(WTS_CURRENT_SERVER_HANDLE, 0, 1, &mut info, &mut count).is_ok() {
                for p in std::slice::from_raw_parts(info, count as usize) {
                    if !self.procs.contains_key(&p.ProcessId) {
                        let image = p.pProcessName.to_string().unwrap_or_default();
                        self.procs.insert(p.ProcessId, Proc { created: 0, session: p.SessionId, image, ended: None });
                    }
                }
                WTSFreeMemory(info as *mut _);
            }
        }
        self.procs.entry(pid).or_insert(Proc { created: 0, session: u32::MAX, image: String::new(), ended: None });
    }

    /// Drops processes that ended a while ago, and keeps the table within capacity.
    fn tidy(&mut self) {
        let procs = &self.procs;
        let current = |pid: &u32, created: &u64| procs.get(pid).is_some_and(|p| p.created == *created && p.ended.is_none_or(|t| t.elapsed() < KEEP_ENDED));
        self.flows.retain(|(pid, created, _), f| current(pid, created) || f.last.is_some_and(|t| t.elapsed() < KEEP_ENDED));
        self.procs.retain(|_, p| p.ended.is_none_or(|t| t.elapsed() < KEEP_ENDED));
        if self.adapters.refreshed.is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
            self.adapters.refresh();
        }
    }

    /// The busiest processes, for the diagnostic log.
    pub fn summary(&self) -> Vec<String> {
        let mut flows: Vec<_> = self.flows.iter().collect();
        flows.sort_by_key(|(_, f)| std::cmp::Reverse(f.rx + f.tx));
        let known = self.adapters.by_addr.len();
        let mut out = vec![format!("{} flows, {} processes, {known} local addresses", self.flows.len(), self.procs.len())];
        for ((pid, created, luid), f) in flows.into_iter().take(12) {
            let image = self.procs.get(pid).filter(|p| p.created == *created).map_or("?", |p| p.image.as_str());
            out.push(format!("  pid {pid} luid {luid:#x} rx {} tx {} {image}", f.rx, f.tx));
        }
        out
    }

    pub fn publish(&mut self, out: &mut Publisher) {
        self.tidy();
        let mut flows: Vec<(&(u32, u64, u64), &Flow)> = self.flows.iter().collect();
        if flows.len() > CAPACITY {
            flows.sort_by(|a, b| b.1.last.cmp(&a.1.last));
            flows.truncate(CAPACITY);
        }
        let table = out.table();
        table.header.seq.fetch_add(1, Ordering::AcqRel);
        fence(Ordering::Release);
        for (i, ((pid, created, luid), f)) in flows.iter().enumerate() {
            let e = &mut table.entries[i];
            let proc_ = self.procs.get(pid).filter(|p| p.created == *created);
            *e = Entry { pid: *pid, created: *created, luid: *luid, rx: f.rx, tx: f.tx, ..Entry::default() };
            match proc_ {
                Some(p) => {
                    e.session = p.session;
                    set_wide(&mut e.image, &p.image);
                }
                None if *pid == 4 || *pid == 0 => {
                    e.session = 0;
                    set_wide(&mut e.image, "System");
                }
                None => {}
            }
        }
        table.header.count = flows.len() as u32;
        table.header.updated_ms = chrono::Utc::now().timestamp_millis();
        fence(Ordering::Release);
        table.header.seq.fetch_add(1, Ordering::AcqRel);
    }
}

/// The named file mapping: the service writes it, everyone signed in can read it.
pub struct Publisher {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

unsafe impl Send for Publisher {}

impl Publisher {
    pub fn create() -> Result<Self, String> {
        // System, Local Service and administrators: full access. Signed-in users: read.
        let sddl = w!("D:P(A;;GA;;;SY)(A;;GA;;;LS)(A;;GA;;;BA)(A;;GR;;;AU)");
        unsafe {
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, SDDL_REVISION_1, &mut sd, None)
                .map_err(|e| format!("security descriptor: {e}"))?;
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: BOOL(0),
            };
            let size = std::mem::size_of::<Table>() as u32;
            let handle = CreateFileMappingW(INVALID_HANDLE_VALUE, Some(&sa), PAGE_READWRITE, 0, size, MAPPING);
            let _ = LocalFree(HLOCAL(sd.0));
            let handle = handle.map_err(|e| format!("shared table: {e}"))?;
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 0);
            if view.Value.is_null() {
                let _ = CloseHandle(handle);
                return Err("shared table: can't map it".into());
            }
            let mut p = Self { handle, view };
            let h = &mut p.table().header;
            h.version = SERVICE_VERSION;
            h.generation = chrono::Utc::now().timestamp_millis() as u64;
            h.count = 0;
            h.magic = MAGIC;
            Ok(p)
        }
    }

    fn table(&mut self) -> &mut Table {
        unsafe { &mut *(self.view.Value as *mut Table) }
    }

    pub fn set_status(&mut self, error: u32, text: &str, events_lost: u64) {
        let h = &mut self.table().header;
        h.seq.fetch_add(1, Ordering::AcqRel);
        fence(Ordering::Release);
        h.error = error;
        set_wide(&mut h.error_text, text);
        h.events_lost = events_lost;
        h.updated_ms = chrono::Utc::now().timestamp_millis();
        fence(Ordering::Release);
        h.seq.fetch_add(1, Ordering::AcqRel);
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback() {
        assert!(Addr::V4([127, 0, 0, 1]).is_loopback());
        assert!(!Addr::V4([192, 168, 1, 2]).is_loopback());
        let mut one = [0u8; 16];
        one[15] = 1;
        assert!(Addr::V6(one).is_loopback());
        let mut mapped = [0u8; 16];
        mapped[10] = 0xff;
        mapped[11] = 0xff;
        mapped[12] = 127;
        mapped[15] = 1;
        assert!(Addr::V6(mapped).is_loopback());
    }

    #[test]
    fn counts_per_process_and_skips_loopback() {
        let mut c = Counters::new();
        c.process_seen(100, 7, 1, r"\Device\HarddiskVolume3\app.exe".into());
        c.add(100, &[10, 0, 0, 2], &[1, 1, 1, 1], 500, true);
        c.add(100, &[10, 0, 0, 2], &[1, 1, 1, 1], 20, false);
        c.add(100, &[127, 0, 0, 1], &[127, 0, 0, 1], 999, true);
        let f = c.flows.values().next().unwrap();
        assert_eq!((c.flows.len(), f.rx, f.tx), (1, 500, 20));
    }
}
