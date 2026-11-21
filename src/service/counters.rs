//! Running totals per process and network adapter, and the shared table that
//! readers copy them from.

use crate::appnet::{
    set_wide, AdapterBytes, Bytes, Entry, Mapping, Table, TrafficEntry, CAPACITY, MAGIC, MAPPING, SERVICE_VERSION,
};
use std::collections::HashMap;
use std::sync::atomic::{fence, Ordering};
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::{LocalFree, BOOL, ERROR_BUFFER_OVERFLOW, HLOCAL, INVALID_HANDLE_VALUE, NO_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_FRIENDLY_NAME,
    GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6};
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_ALL_ACCESS, PAGE_READWRITE};
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
    total: Bytes,
    internet: Bytes,
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
            16 => {
                let bytes: [u8; 16] = b.try_into().ok()?;
                Some(match std::net::Ipv6Addr::from(bytes).to_ipv4_mapped() {
                    Some(ip) => Addr::V4(ip.octets()),
                    None => Addr::V6(bytes),
                })
            }
            _ => None,
        }
    }

    /// Traffic that never leaves the PC (127.x, ::1, and IPv4-mapped 127.x).
    fn is_loopback(&self) -> bool {
        match self {
            Addr::V4(a) => a[0] == 127,
            Addr::V6(a) => std::net::Ipv6Addr::from(*a).is_loopback(),
        }
    }

    fn is_link_scoped(self) -> bool {
        match self {
            Addr::V4([169, 254, ..] | [224, 0, 0, _] | [239, 255, ..] | [255, 255, 255, 255]) => true,
            Addr::V4(_) => false,
            Addr::V6(a) => (a[0] == 0xfe && a[1] & 0xc0 == 0x80) || (a[0] == 0xff && matches!(a[1] & 0x0f, 1 | 2)),
        }
    }

    fn same_subnet(self, peer: Self, prefix: u8) -> bool {
        match (self, peer) {
            (Addr::V4(a), Addr::V4(b)) if (1..=32).contains(&prefix) => {
                (u32::from_be_bytes(a) ^ u32::from_be_bytes(b)) >> (32 - prefix) == 0
            }
            (Addr::V6(a), Addr::V6(b)) if (1..=128).contains(&prefix) => {
                (u128::from_be_bytes(a) ^ u128::from_be_bytes(b)) >> (128 - prefix) == 0
            }
            _ => false,
        }
    }
}

/// Which adapter owns each local address.
struct Adapters {
    by_addr: HashMap<Addr, u64>,
    subnets: HashMap<u64, Vec<(Addr, u8)>>,
    refreshed: Option<Instant>,
}

impl Adapters {
    /// Returns the local endpoint's adapter and whether the peer is on the LAN.
    /// Received UDP lists the local address second, so both ends are tried.
    fn classify(&mut self, first: Addr, second: Option<Addr>) -> (u64, bool) {
        let find = |map: &HashMap<Addr, u64>| {
            map.get(&first).map(|&luid| (luid, second))
                .or_else(|| second.and_then(|addr| map.get(&addr)).map(|&luid| (luid, Some(first))))
        };
        // Neither end is known: an adapter may have come up. Look again, at most
        // every five seconds.
        let mut endpoints = find(&self.by_addr);
        if endpoints.is_none() && self.refreshed.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
            self.refresh();
            endpoints = find(&self.by_addr);
        }
        let (luid, peer) = endpoints.unwrap_or((0, second));
        let lan = peer.is_some_and(|peer| {
            peer.is_link_scoped()
                || self.by_addr.contains_key(&peer)
                || self.subnets.get(&luid).is_some_and(|subnets| {
                    subnets.iter().any(|&(addr, prefix)| addr.same_subnet(peer, prefix))
                })
        });
        (luid, lan)
    }

    fn add_address(&mut self, addr: Addr, luid: u64, prefix: u8) {
        self.by_addr.insert(addr, luid);
        self.subnets.entry(luid).or_default().push((addr, prefix));
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
            self.subnets.clear();
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
                            Addr::from_bytes(&unsafe { sin6.sin6_addr.u.Byte })
                        } else {
                            None
                        };
                        if let Some(addr) = addr {
                            let prefix = if family == AF_INET6 && matches!(addr, Addr::V4(_)) {
                                ua.OnLinkPrefixLength.saturating_sub(96)
                            } else {
                                ua.OnLinkPrefixLength
                            };
                            self.add_address(addr, luid, prefix);
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
    adapter_internet: HashMap<u64, Bytes>,
    adapters: Adapters,
    names_checked: Option<Instant>,
}

impl Counters {
    pub fn new() -> Self {
        let mut c = Self {
            procs: HashMap::new(),
            flows: HashMap::new(),
            adapter_internet: HashMap::new(),
            adapters: Adapters { by_addr: HashMap::new(), subnets: HashMap::new(), refreshed: None },
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
        let (luid, is_lan) = self.adapters.classify(local, remote);
        if !self.procs.contains_key(&pid) {
            self.learn_names(pid);
        }
        let created = self.procs.get(&pid).map_or(0, |p| p.created);
        let f = self.flows.entry((pid, created, luid)).or_default();
        f.total.record(bytes, received);
        if !is_lan {
            f.internet.record(bytes, received);
            self.adapter_internet.entry(luid).or_default().record(bytes, received);
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
        flows.sort_by_key(|(_, f)| std::cmp::Reverse(f.total.rx + f.total.tx));
        let known = self.adapters.by_addr.len();
        let mut out = vec![format!("{} flows, {} processes, {known} local addresses", self.flows.len(), self.procs.len())];
        for ((pid, created, luid), f) in flows.into_iter().take(12) {
            let image = self.procs.get(pid).filter(|p| p.created == *created).map_or("?", |p| p.image.as_str());
            out.push(format!("  pid {pid} luid {luid:#x} rx {} tx {} {image}", f.total.rx, f.total.tx));
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
            let proc_ = self.procs.get(pid).filter(|p| p.created == *created);
            let e = &mut table.entries[i];
            *e = TrafficEntry {
                app: Entry { pid: *pid, created: *created, luid: *luid, rx: f.total.rx, tx: f.total.tx, ..Entry::default() },
                internet: f.internet,
            };
            match proc_ {
                Some(p) => {
                    e.app.session = p.session;
                    set_wide(&mut e.app.image, &p.image);
                }
                None if *pid == 4 || *pid == 0 => {
                    e.app.session = 0;
                    set_wide(&mut e.app.image, "System");
                }
                None => {}
            }
        }
        table.adapter_count = self.adapter_internet.len().min(CAPACITY) as u32;
        for (slot, (&luid, &bytes)) in table.adapters.iter_mut().zip(&self.adapter_internet) {
            *slot = AdapterBytes { luid, bytes };
        }
        table.header.count = flows.len() as u32;
        table.header.updated_ms = chrono::Utc::now().timestamp_millis();
        fence(Ordering::Release);
        table.header.seq.fetch_add(1, Ordering::AcqRel);
    }
}

/// The service writes the mapping, and signed-in users can read it.
pub struct Publisher {
    mapping: Mapping,
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
            let size = std::mem::size_of::<Table>();
            let handle = CreateFileMappingW(INVALID_HANDLE_VALUE, Some(&sa), PAGE_READWRITE, 0, size as u32, MAPPING);
            let _ = LocalFree(HLOCAL(sd.0));
            let handle = handle.map_err(|e| format!("shared table: {e}"))?;
            let mapping = Mapping::map(handle, FILE_MAP_ALL_ACCESS, size).ok_or_else(|| "shared table: can't map it".to_owned())?;
            let mut publisher = Self { mapping };
            let table = publisher.table();
            let h = &mut table.header;
            // A reader can keep the previous mapping alive across a service restart.
            // Replace its generation and counters together, even if it was left mid-write.
            let writing = h.seq.fetch_or(1, Ordering::AcqRel) | 1;
            fence(Ordering::Release);
            table.adapter_count = 0;
            h.version = SERVICE_VERSION;
            h.generation = chrono::Utc::now().timestamp_millis() as u64;
            h.count = 0;
            h.updated_ms = 0;
            h.events_lost = 0;
            h.error = 0;
            h.counted = 0;
            h.error_text[0] = 0;
            h.magic = MAGIC;
            h.seq.store(writing.wrapping_add(1), Ordering::Release);
            Ok(publisher)
        }
    }

    fn table(&mut self) -> &mut Table {
        unsafe { &mut *self.mapping.ptr() }
    }

    pub fn set_status(&mut self, error: u32, text: &str, events_lost: u64, counted: u32) {
        let h = &mut self.table().header;
        h.seq.fetch_add(1, Ordering::AcqRel);
        fence(Ordering::Release);
        h.error = error;
        set_wide(&mut h.error_text, text);
        h.events_lost = events_lost;
        h.counted = counted;
        h.updated_ms = chrono::Utc::now().timestamp_millis();
        fence(Ordering::Release);
        h.seq.fetch_add(1, Ordering::AcqRel);
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
        assert!(Addr::from_bytes(&mapped).unwrap().is_loopback());
    }

    #[test]
    fn counts_per_process_and_skips_loopback() {
        let mut c = Counters::new();
        c.process_seen(100, 7, 1, r"\Device\HarddiskVolume3\app.exe".into());
        c.add(100, &[10, 0, 0, 2], &[1, 1, 1, 1], 500, true);
        c.add(100, &[10, 0, 0, 2], &[1, 1, 1, 1], 20, false);
        c.add(100, &[127, 0, 0, 1], &[127, 0, 0, 1], 999, true);
        let f = c.flows.values().next().unwrap();
        assert_eq!((c.flows.len(), f.total.rx, f.total.tx), (1, 500, 20));
    }

    #[test]
    fn finds_the_adapter_from_either_end() {
        let mut a = Adapters { by_addr: HashMap::new(), subnets: HashMap::new(), refreshed: Some(Instant::now()) };
        let (local, remote) = (Addr::V4([10, 0, 0, 2]), Addr::V4([1, 1, 1, 1]));
        a.add_address(local, 7, 24);
        assert_eq!(a.classify(local, Some(remote)), (7, false));
        // Received UDP: the local address comes second.
        assert_eq!(a.classify(remote, Some(local)), (7, false));
        assert_eq!(a.classify(remote, None), (0, false));
    }

    #[test]
    fn internet_totals_exclude_connected_subnets_in_both_directions() {
        let mut c = Counters::new();
        let local = [192, 168, 40, 2];
        let lan = [192, 168, 40, 30];
        let routed = [192, 168, 41, 30];
        c.adapters.by_addr.clear();
        c.adapters.subnets.clear();
        c.adapters.add_address(Addr::V4(local), 7, 24);
        c.process_seen(100, 7, 1, "app.exe".into());
        c.add(100, &local, &lan, 10_000, false);
        c.add(100, &lan, &local, 20_000, true); // received UDP puts the peer first
        c.add(100, &local, &routed, 40, false);
        c.add(100, &routed, &local, 60, true);
        let flow = c.flows.values().next().unwrap();
        let adapter = c.adapter_internet.get(&7).unwrap();
        assert_eq!((flow.total.rx, flow.total.tx), (20_060, 10_040));
        assert_eq!((flow.internet.rx, flow.internet.tx), (60, 40));
        assert_eq!((adapter.rx, adapter.tx), (60, 40));
    }

    #[test]
    fn ipv6_local_scope_and_prefix() {
        let local = Addr::V6("2001:db8:1::2".parse::<std::net::Ipv6Addr>().unwrap().octets());
        let peer = Addr::V6("2001:db8:1::3".parse::<std::net::Ipv6Addr>().unwrap().octets());
        let routed = Addr::V6("2001:db8:2::3".parse::<std::net::Ipv6Addr>().unwrap().octets());
        let multicast = Addr::V6("ff02::fb".parse::<std::net::Ipv6Addr>().unwrap().octets());
        assert!(local.same_subnet(peer, 64));
        assert!(!local.same_subnet(routed, 64));
        assert!(multicast.is_link_scoped());
        assert!(!routed.is_link_scoped());
    }
}
