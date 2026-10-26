//! Network adapters and their byte counters.
//!
//! Adapters come from GetAdaptersAddresses (one entry per real adapter, no filter-driver
//! duplicates). Counters come from GetIfEntry2, which has 64-bit octet counts.

use crate::config::Connection;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GetBestInterfaceEx, GetIfEntry2, GAA_FLAG_SKIP_ANYCAST,
    GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH, MIB_IF_ROW2,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_UNSPEC, SOCKADDR, SOCKADDR_IN};

const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const IF_TYPE_TUNNEL: u32 = 131;
const IF_OPER_STATUS_UP: i32 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct Adapter {
    /// Stable GUID name, e.g. "{4D36E972-...}".
    pub id: String,
    pub luid: u64,
    pub if_index: u32,
    /// Friendly name, e.g. "Ethernet" or "Wi-Fi".
    pub name: String,
    /// Driver description, e.g. "Realtek PCIe GbE Family Controller".
    pub description: String,
    pub up: bool,
    pub ipv4: Option<String>,
    /// Link speed in bits per second, if known.
    pub link_speed: u64,
    pub virtual_like: bool,
}

fn looks_virtual(desc: &str) -> bool {
    let d = desc.to_ascii_lowercase();
    ["hyper-v", "virtual", "vmware", "virtualbox", "tap-windows", "wintun", "wireguard", "openvpn", "vpn", "loopback", "bluetooth", "wan miniport"]
        .iter()
        .any(|k| d.contains(k))
}

pub fn list_adapters() -> Vec<Adapter> {
    let mut size: u32 = 32 * 1024;
    for _ in 0..4 {
        // u64 storage keeps the buffer 8-byte aligned for the structs inside.
        let mut buf: Vec<u64> = vec![0; (size as usize).div_ceil(8)];
        let ret = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC.0 as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                None,
                Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH),
                &mut size,
            )
        };
        if ret == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        if ret != NO_ERROR.0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !p.is_null() {
            let a = unsafe { &*p };
            p = a.Next;
            if a.IfType == IF_TYPE_SOFTWARE_LOOPBACK || a.IfType == IF_TYPE_TUNNEL {
                continue;
            }
            let name = unsafe { a.FriendlyName.to_string() }.unwrap_or_default();
            let description = unsafe { a.Description.to_string() }.unwrap_or_default();
            let id = unsafe { a.AdapterName.to_string() }.unwrap_or_default();
            out.push(Adapter {
                virtual_like: looks_virtual(&description),
                id,
                luid: unsafe { a.Luid.Value },
                if_index: unsafe { a.Anonymous1.Anonymous.IfIndex },
                name,
                description,
                up: a.OperStatus.0 == IF_OPER_STATUS_UP,
                ipv4: first_ipv4(a),
                link_speed: a.ReceiveLinkSpeed.max(a.TransmitLinkSpeed),
            });
        }
        return out;
    }
    Vec::new()
}

fn first_ipv4(a: &IP_ADAPTER_ADDRESSES_LH) -> Option<String> {
    let mut u = a.FirstUnicastAddress;
    while !u.is_null() {
        let ua = unsafe { &*u };
        let sa = ua.Address.lpSockaddr;
        if !sa.is_null() && unsafe { (*sa).sa_family } == AF_INET {
            let sin = unsafe { &*(sa as *const SOCKADDR_IN) };
            let b = unsafe { sin.sin_addr.S_un.S_addr }.to_ne_bytes();
            return Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]));
        }
        u = ua.Next;
    }
    None
}

/// Total received and sent bytes since the adapter came up.
pub fn counters(luid: u64) -> Option<(u64, u64)> {
    let mut row = MIB_IF_ROW2::default();
    row.InterfaceLuid.Value = luid;
    let r = unsafe { GetIfEntry2(&mut row) };
    r.is_ok().then_some((row.InOctets, row.OutOctets))
}

/// The interface Windows would use to reach the public internet right now.
fn best_internet_interface() -> Option<u32> {
    let mut addr = SOCKADDR_IN::default();
    addr.sin_family = AF_INET;
    addr.sin_addr.S_un.S_addr = u32::from_ne_bytes([1, 1, 1, 1]);
    let mut index = 0u32;
    let r = unsafe { GetBestInterfaceEx(&addr as *const SOCKADDR_IN as *const SOCKADDR, &mut index) };
    (r == NO_ERROR.0).then_some(index)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Reading {
    /// Download, bytes per second.
    pub down: f64,
    /// Upload, bytes per second.
    pub up: f64,
    /// Bytes received/sent since the previous reading.
    pub rx: u64,
    pub tx: u64,
}

/// Keeps per-adapter counters between ticks and turns them into rates.
pub struct Sampler {
    adapters: Vec<Adapter>,
    refreshed: Option<Instant>,
    prev: HashMap<u64, (u64, u64)>,
    last: Instant,
    active: Vec<u64>,
    selection: Option<Connection>,
}

const REFRESH_EVERY: Duration = Duration::from_secs(10);
/// Anything faster than this is a counter glitch, not traffic (100 Gbit/s).
const MAX_PLAUSIBLE: f64 = 12.5e9;

impl Sampler {
    pub fn new() -> Self {
        Self {
            adapters: Vec::new(),
            refreshed: None,
            prev: HashMap::new(),
            last: Instant::now(),
            active: Vec::new(),
            selection: None,
        }
    }

    pub fn adapters(&self) -> &[Adapter] {
        &self.adapters
    }

    /// Adapters currently being measured.
    pub fn active(&self) -> Vec<&Adapter> {
        self.active.iter().filter_map(|l| self.adapters.iter().find(|a| a.luid == *l)).collect()
    }

    /// LUIDs of the adapters currently being measured.
    pub fn active_luids(&self) -> &[u64] {
        &self.active
    }

    pub fn force_refresh(&mut self) {
        self.refreshed = None;
    }

    fn refresh(&mut self, sel: &Connection) {
        self.adapters = list_adapters();
        self.refreshed = Some(Instant::now());
        self.selection = Some(sel.clone());
        self.active = match sel {
            Connection::All => self.adapters.iter().filter(|a| a.up).map(|a| a.luid).collect(),
            Connection::Adapter { id, .. } => {
                self.adapters.iter().filter(|a| &a.id == id).map(|a| a.luid).collect()
            }
            Connection::Auto => self.auto_pick().into_iter().collect(),
        };
    }

    fn auto_pick(&self) -> Option<u64> {
        if let Some(idx) = best_internet_interface() {
            if let Some(a) = self.adapters.iter().find(|a| a.if_index == idx && a.up) {
                return Some(a.luid);
            }
        }
        // No route to the internet: fall back to the busiest adapter, preferring physical ones.
        let busiest = |physical: bool| {
            self.adapters
                .iter()
                .filter(|a| a.up && (!physical || !a.virtual_like))
                .filter_map(|a| counters(a.luid).map(|(r, t)| (a.luid, r + t)))
                .max_by_key(|(_, total)| *total)
                .map(|(l, _)| l)
        };
        busiest(true).or_else(|| busiest(false))
    }

    pub fn sample(&mut self, sel: &Connection) -> Reading {
        let due = self.refreshed.map_or(true, |t| t.elapsed() >= REFRESH_EVERY);
        if due || self.selection.as_ref() != Some(sel) {
            self.refresh(sel);
        }
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64().max(0.05);
        self.last = now;

        let mut reading = Reading::default();
        let mut seen = HashMap::with_capacity(self.active.len());
        for &luid in &self.active {
            let Some((r, t)) = counters(luid) else { continue };
            if let Some(&(pr, pt)) = self.prev.get(&luid) {
                // A counter that goes backwards means the adapter was reset.
                let dr = r.checked_sub(pr).unwrap_or(0);
                let dt_ = t.checked_sub(pt).unwrap_or(0);
                if (dr as f64) / dt < MAX_PLAUSIBLE && (dt_ as f64) / dt < MAX_PLAUSIBLE {
                    reading.rx += dr;
                    reading.tx += dt_;
                }
            }
            seen.insert(luid, (r, t));
        }
        self.prev = seen;
        reading.down = reading.rx as f64 / dt;
        reading.up = reading.tx as f64 / dt;
        reading
    }
}

/// A short label for whatever is being measured: "Ethernet", "All networks", ...
pub fn selection_label(sel: &Connection, active: &[&Adapter]) -> (String, String) {
    match sel {
        Connection::All => (
            "All networks".into(),
            format!("{} adapters combined", active.len()),
        ),
        Connection::Adapter { name, .. } => match active.first() {
            Some(a) => (a.name.clone(), a.description.clone()),
            None => (name.clone(), "Not connected".into()),
        },
        Connection::Auto => match active.first() {
            Some(a) => (a.name.clone(), a.description.clone()),
            None => ("No connection".into(), "Waiting for a network".into()),
        },
    }
}
