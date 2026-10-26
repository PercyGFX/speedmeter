//! Per-app network counters, shared by the counting service (speedmeter-service.exe)
//! and the meter through a named file mapping that only the service can write.
//!
//! The service keeps one running total per process and network adapter and rewrites
//! the table twice a second. Readers copy it and work with the differences between
//! reads, so a late or missed read loses nothing. Remote addresses are never stored.
//!
//! This file is compiled into both executables.

#![allow(dead_code)]

use std::sync::atomic::{fence, AtomicU32, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS,
};

pub const MAPPING: PCWSTR = w!("Global\\SpeedMeter.AppNet.v1");
pub const MAGIC: u32 = 0x5350_4E31; // "SPN1"
/// Bump whenever the service changes, so the app offers to update an older one.
pub const SERVICE_VERSION: u32 = 1;
pub const SERVICE_NAME: PCWSTR = w!("SpeedMeterApps");
pub const SERVICE_EXE: &str = "speedmeter-service.exe";
pub const CAPACITY: usize = 1024;

#[repr(C)]
pub struct Header {
    pub magic: u32,
    pub version: u32,
    pub seq: AtomicU32,
    pub count: u32,
    /// Changes every time the service starts counting from zero.
    pub generation: u64,
    /// Unix milliseconds of the last rewrite.
    pub updated_ms: i64,
    /// Events Windows dropped because the service fell behind.
    pub events_lost: u64,
    /// 0 while counting; otherwise the Win32 error that stopped tracing.
    pub error: u32,
    pub _pad: u32,
    pub error_text: [u16; 128],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Entry {
    pub pid: u32,
    /// Windows session of the process (0 for services), u32::MAX if unknown.
    pub session: u32,
    /// Process creation time (FILETIME), so a reused process id counts as a new process.
    pub created: u64,
    /// The adapter (NET_LUID) the traffic went through, 0 if unknown.
    pub luid: u64,
    pub rx: u64,
    pub tx: u64,
    /// The exe in kernel form (`\Device\HarddiskVolume3\...\app.exe`), or only its
    /// file name when the full path isn't known.
    pub image: [u16; 260],
}

impl Default for Entry {
    fn default() -> Self {
        Self { pid: 0, session: u32::MAX, created: 0, luid: 0, rx: 0, tx: 0, image: [0; 260] }
    }
}

#[repr(C)]
pub struct Table {
    pub header: Header,
    pub entries: [Entry; CAPACITY],
}

pub fn wide_str(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub fn set_wide(dst: &mut [u16], s: &str) {
    let mut n = 0;
    for (d, c) in dst.iter_mut().zip(s.encode_utf16()) {
        *d = c;
        n += 1;
    }
    let end = n.min(dst.len().saturating_sub(1));
    dst[end..].fill(0);
}

/// What a reader copied out.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub version: u32,
    pub generation: u64,
    pub updated_ms: i64,
    pub events_lost: u64,
    pub error: u32,
    pub error_text: String,
    pub entries: Vec<Entry>,
}

impl Snapshot {
    /// The service rewrote the table in the last few seconds.
    pub fn is_fresh(&self) -> bool {
        chrono::Utc::now().timestamp_millis() - self.updated_ms < 5000
    }
}

pub struct Reader {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

// The view is read-only and only ever read through volatile copies.
unsafe impl Send for Reader {}

impl Reader {
    pub fn open() -> Option<Self> {
        unsafe {
            let handle = OpenFileMappingW(FILE_MAP_READ.0, false, MAPPING).ok()?;
            let view = MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0);
            if view.Value.is_null() {
                let _ = CloseHandle(handle);
                return None;
            }
            let r = Self { handle, view };
            (std::ptr::read_volatile(&(*r.table()).header.magic) == MAGIC).then_some(r)
        }
    }

    fn table(&self) -> *const Table {
        self.view.Value as *const Table
    }

    /// Copies the table into `out` (reusing its memory). False if the service kept
    /// writing for the whole attempt.
    pub fn read(&self, out: &mut Snapshot) -> bool {
        let t = self.table();
        for _ in 0..64 {
            unsafe {
                let h = &(*t).header;
                let s1 = h.seq.load(Ordering::Acquire);
                if s1 % 2 == 1 {
                    std::hint::spin_loop();
                    continue;
                }
                fence(Ordering::Acquire);
                let count = (std::ptr::read_volatile(&h.count) as usize).min(CAPACITY);
                out.entries.clear();
                for i in 0..count {
                    out.entries.push(std::ptr::read_volatile(&(*t).entries[i]));
                }
                out.version = std::ptr::read_volatile(&h.version);
                out.generation = std::ptr::read_volatile(&h.generation);
                out.updated_ms = std::ptr::read_volatile(&h.updated_ms);
                out.events_lost = std::ptr::read_volatile(&h.events_lost);
                out.error = std::ptr::read_volatile(&h.error);
                out.error_text = wide_str(&std::ptr::read_volatile(&h.error_text));
                fence(Ordering::Acquire);
                if h.seq.load(Ordering::Acquire) == s1 {
                    return true;
                }
            }
        }
        false
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}
