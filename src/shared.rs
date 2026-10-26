//! Live data shared from the background monitor to the dashboard through a named
//! file mapping. The monitor writes one sample per second into a ring buffer; the
//! dashboard copies it out. A sequence counter (seqlock) guards against torn reads.

use crate::sys::{copy_wide, from_wide};
use std::sync::atomic::{fence, AtomicU32, Ordering};
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};

/// One hour of per-second samples.
pub const RING: usize = 3600;

pub const PAGE_LIVE: u32 = 1;
pub const PAGE_HISTORY: u32 = 2;
pub const PAGE_SETTINGS: u32 = 3;

pub fn page_name(page: u32) -> &'static str {
    match page {
        PAGE_HISTORY => "history",
        PAGE_SETTINGS => "settings",
        _ => "live",
    }
}

pub fn page_from_name(name: &str) -> u32 {
    match name {
        "history" => PAGE_HISTORY,
        "settings" => PAGE_SETTINGS,
        _ => PAGE_LIVE,
    }
}
const MAGIC: u32 = 0x5350_4D31; // "SPM1"
const NAME: windows::core::PCWSTR = w!("Local\\SpeedMeter.Live.v1");

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Sample {
    /// Unix time in milliseconds.
    pub t: i64,
    /// Download, bytes per second.
    pub down: f32,
    /// Upload, bytes per second.
    pub up: f32,
}

#[repr(C)]
struct Live {
    magic: u32,
    seq: AtomicU32,
    /// Page the dashboard should switch to (set by the monitor's menu), 0 = none.
    page_request: AtomicU32,
    _pad: u32,
    head: u64,
    today_rx: u64,
    today_tx: u64,
    month_rx: u64,
    month_tx: u64,
    iface_name: [u16; 64],
    iface_desc: [u16; 128],
    samples: [Sample; RING],
}

/// Plain copy of the shared state, owned by the reader.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub today_rx: u64,
    pub today_tx: u64,
    pub month_rx: u64,
    pub month_tx: u64,
    pub iface_name: String,
    pub iface_desc: String,
    /// Oldest first.
    pub samples: Vec<Sample>,
}

impl Snapshot {
    pub fn latest(&self) -> Option<Sample> {
        self.samples.last().copied()
    }

    /// True if the monitor wrote a sample in the last few seconds.
    pub fn is_fresh(&self) -> bool {
        self.latest()
            .map(|s| chrono::Utc::now().timestamp_millis() - s.t < 4000)
            .unwrap_or(false)
    }
}

struct Mapping {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl Mapping {
    fn ptr(&self) -> *mut Live {
        self.view.Value as *mut Live
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

pub struct Writer {
    map: Mapping,
}

impl Writer {
    pub fn create() -> Option<Self> {
        let size = std::mem::size_of::<Live>() as u32;
        unsafe {
            let handle = CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, size, NAME).ok()?;
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 0);
            if view.Value.is_null() {
                let _ = CloseHandle(handle);
                return None;
            }
            let map = Mapping { handle, view };
            let live = &mut *map.ptr();
            live.magic = MAGIC;
            live.head = 0;
            Some(Self { map })
        }
    }

    fn live(&mut self) -> &mut Live {
        unsafe { &mut *self.map.ptr() }
    }

    pub fn publish(&mut self, sample: Sample, totals: [u64; 4], name: &str, desc: &str) {
        let live = self.live();
        live.seq.fetch_add(1, Ordering::AcqRel);
        fence(Ordering::Release);
        let slot = (live.head % RING as u64) as usize;
        live.samples[slot] = sample;
        live.head += 1;
        live.today_rx = totals[0];
        live.today_tx = totals[1];
        live.month_rx = totals[2];
        live.month_tx = totals[3];
        copy_wide(&mut live.iface_name, name);
        copy_wide(&mut live.iface_desc, desc);
        fence(Ordering::Release);
        live.seq.fetch_add(1, Ordering::AcqRel);
    }

    pub fn request_page(&mut self, page: u32) {
        self.live().page_request.store(page, Ordering::Release);
    }
}

pub struct Reader {
    map: Mapping,
}

impl Reader {
    pub fn open() -> Option<Self> {
        unsafe {
            let handle = OpenFileMappingW((FILE_MAP_READ | FILE_MAP_WRITE).0, false, NAME).ok()?;
            let view = MapViewOfFile(handle, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, 0);
            if view.Value.is_null() {
                let _ = CloseHandle(handle);
                return None;
            }
            let map = Mapping { handle, view };
            if (*map.ptr()).magic != MAGIC {
                return None;
            }
            Some(Self { map })
        }
    }

    pub fn snapshot(&self) -> Option<Snapshot> {
        let live = self.map.ptr() as *const Live;
        for _ in 0..64 {
            unsafe {
                let s1 = (*live).seq.load(Ordering::Acquire);
                if s1 % 2 == 1 {
                    std::hint::spin_loop();
                    continue;
                }
                fence(Ordering::Acquire);
                let head = std::ptr::read_volatile(&(*live).head);
                let n = head.min(RING as u64) as usize;
                let mut samples = Vec::with_capacity(n);
                let start = head - n as u64;
                for i in start..head {
                    let slot = (i % RING as u64) as usize;
                    samples.push(std::ptr::read_volatile(&(*live).samples[slot]));
                }
                let snap = Snapshot {
                    today_rx: std::ptr::read_volatile(&(*live).today_rx),
                    today_tx: std::ptr::read_volatile(&(*live).today_tx),
                    month_rx: std::ptr::read_volatile(&(*live).month_rx),
                    month_tx: std::ptr::read_volatile(&(*live).month_tx),
                    iface_name: from_wide(&std::ptr::read_volatile(&(*live).iface_name)),
                    iface_desc: from_wide(&std::ptr::read_volatile(&(*live).iface_desc)),
                    samples,
                };
                fence(Ordering::Acquire);
                if (*live).seq.load(Ordering::Acquire) == s1 {
                    return Some(snap);
                }
            }
        }
        None
    }

    pub fn take_page_request(&self) -> u32 {
        unsafe { (*self.map.ptr()).page_request.swap(0, Ordering::AcqRel) }
    }

    pub fn request_page(&self, page: u32) {
        unsafe { (*self.map.ptr()).page_request.store(page, Ordering::Release) }
    }
}
