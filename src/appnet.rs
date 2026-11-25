//! Per-app network counters, shared by the counting service (speedmeter-service.exe)
//! and the meter through a named file mapping that only the service can write.
//!
//! The service rewrites total and internet-only counters twice a second. Readers
//! take differences between reads, so a late or missed read loses nothing. Remote
//! addresses are never stored.
//!
//! This file is compiled into both executables.

#![allow(dead_code)]

use std::sync::atomic::{fence, AtomicU32, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS,
};

pub const MAPPING: PCWSTR = w!("Global\\SpeedMeter.AppNet.v2");
const LEGACY_MAPPING: PCWSTR = w!("Global\\SpeedMeter.AppNet.v1");
pub const MAGIC: u32 = 0x5350_4E32; // "SPN2"
const LEGACY_MAGIC: u32 = 0x5350_4E31; // "SPN1"
/// Bump whenever the service changes, so the app offers to update an older one.
/// 2: data events are recognized by opcode number (1 counted nothing).
/// 3: received UDP is matched to its adapter (its addresses come the other way round).
/// 4: the v2 mapping adds internet-only counters.
pub const SERVICE_VERSION: u32 = 4;
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
    /// Network events counted since the service started (wraps around).
    pub counted: u32,
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
struct LegacyTable {
    header: Header,
    entries: [Entry; CAPACITY],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Bytes {
    pub rx: u64,
    pub tx: u64,
}

impl Bytes {
    pub fn record(&mut self, bytes: u32, received: bool) {
        if received {
            self.rx += u64::from(bytes);
        } else {
            self.tx += u64::from(bytes);
        }
    }

    pub fn since(self, last: Self) -> Self {
        Self { rx: self.rx.saturating_sub(last.rx), tx: self.tx.saturating_sub(last.tx) }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AdapterBytes {
    pub luid: u64,
    pub bytes: Bytes,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TrafficEntry {
    pub app: Entry,
    pub internet: Bytes,
}

#[repr(C)]
pub struct Table {
    pub header: Header,
    pub entries: [TrafficEntry; CAPACITY],
    pub adapter_count: u32,
    pub adapters: [AdapterBytes; CAPACITY],
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
    pub entries: Vec<TrafficEntry>,
    pub internet_adapters: Option<Vec<AdapterBytes>>,
}

impl Snapshot {
    /// The service rewrote the table in the last few seconds.
    pub fn is_fresh(&self) -> bool {
        chrono::Utc::now().timestamp_millis() - self.updated_ms < 5000
    }
}

pub struct Mapping {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl Mapping {
    /// Map an owned file-mapping handle and close it if the view cannot be opened.
    pub unsafe fn map(handle: HANDLE, access: FILE_MAP, size: usize) -> Option<Self> {
        let view = MapViewOfFile(handle, access, 0, 0, size);
        if view.Value.is_null() {
            let _ = CloseHandle(handle);
            return None;
        }
        Some(Self { handle, view })
    }

    fn open_read(name: PCWSTR, size: usize) -> Option<Self> {
        unsafe {
            let handle = OpenFileMappingW(FILE_MAP_READ.0, false, name).ok()?;
            Self::map(handle, FILE_MAP_READ, size)
        }
    }

    pub fn ptr<T>(&self) -> *mut T {
        self.view.Value.cast()
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

pub struct Reader {
    mapping: Mapping,
    legacy: bool,
}

// The view is read-only and only ever read through volatile copies.
unsafe impl Send for Reader {}

impl Reader {
    pub fn open() -> Option<Self> {
        Self::open_current().or_else(|| Self::open_mapping(LEGACY_MAPPING, true))
    }

    pub fn open_current() -> Option<Self> {
        Self::open_mapping(MAPPING, false)
    }

    pub fn is_legacy(&self) -> bool {
        self.legacy
    }

    fn open_mapping(name: PCWSTR, legacy: bool) -> Option<Self> {
        let (size, magic) = if legacy { (std::mem::size_of::<LegacyTable>(), LEGACY_MAGIC) } else {
            (std::mem::size_of::<Table>(), MAGIC)
        };
        let mapping = Mapping::open_read(name, size)?;
        let valid = unsafe { std::ptr::read_volatile(&(*mapping.ptr::<Header>()).magic) == magic };
        valid.then_some(Self { mapping, legacy })
    }

    /// Copies a consistent snapshot. Legacy services supply only total counters.
    pub fn read(&self, out: &mut Snapshot) -> bool {
        let header = self.mapping.ptr::<Header>();
        for _ in 0..64 {
            unsafe {
                let h = &*header;
                let s1 = h.seq.load(Ordering::Acquire);
                if s1 % 2 == 1 {
                    std::hint::spin_loop();
                    continue;
                }
                fence(Ordering::Acquire);
                let count = (std::ptr::read_volatile(&h.count) as usize).min(CAPACITY);
                out.entries.clear();
                if self.legacy {
                    let table = self.mapping.ptr::<LegacyTable>();
                    for i in 0..count {
                        let app = std::ptr::read_volatile(&(*table).entries[i]);
                        out.entries.push(TrafficEntry { app, internet: Bytes::default() });
                    }
                    out.internet_adapters = None;
                } else {
                    let table = self.mapping.ptr::<Table>();
                    for i in 0..count {
                        out.entries.push(std::ptr::read_volatile(&(*table).entries[i]));
                    }
                    let adapter_count = (std::ptr::read_volatile(&(*table).adapter_count) as usize).min(CAPACITY);
                    let adapters = out.internet_adapters.get_or_insert_with(Vec::new);
                    adapters.clear();
                    for i in 0..adapter_count {
                        adapters.push(std::ptr::read_volatile(&(*table).adapters[i]));
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_ALL_ACCESS, PAGE_READWRITE};

    fn fixture<T>(name: &HSTRING) -> Mapping {
        let size = std::mem::size_of::<T>();
        unsafe {
            let handle = CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, size as u32, name).unwrap();
            Mapping::map(handle, FILE_MAP_ALL_ACCESS, size).unwrap()
        }
    }

    #[test]
    fn reads_old_totals_and_new_internet_counters() {
        assert_eq!(std::mem::offset_of!(LegacyTable, entries), 304);
        assert_eq!(std::mem::size_of::<Entry>(), 560);
        let pid = std::process::id();
        let old_name = HSTRING::from(format!("Local\\SpeedMeter.AppNet.Test.{pid}.v1"));
        let new_name = HSTRING::from(format!("Local\\SpeedMeter.AppNet.Test.{pid}.v2"));
        let old = fixture::<LegacyTable>(&old_name);
        let new = fixture::<Table>(&new_name);
        let app = Entry { pid: 73, luid: 11, rx: 180, tx: 40, ..Entry::default() };
        unsafe {
            let table = &mut *old.ptr::<LegacyTable>();
            table.header.magic = LEGACY_MAGIC;
            table.header.version = 3;
            table.header.count = 1;
            table.entries[0] = app;

            let table = &mut *new.ptr::<Table>();
            table.header.magic = MAGIC;
            table.header.version = SERVICE_VERSION;
            table.header.count = 1;
            let bytes = Bytes { rx: 50, tx: 10 };
            table.entries[0] = TrafficEntry { app, internet: bytes };
            table.adapters[0] = AdapterBytes { luid: app.luid, bytes };
            table.adapter_count = 1;
        }

        let current = Reader::open_mapping(PCWSTR(new_name.as_ptr()), false).unwrap();
        let legacy = Reader::open_mapping(PCWSTR(old_name.as_ptr()), true).unwrap();
        let mut snapshot = Snapshot::default();
        assert!(current.read(&mut snapshot));
        assert_eq!(snapshot.entries[0].app.rx, 180);
        assert_eq!(snapshot.entries[0].internet.rx, 50);
        assert_eq!(snapshot.internet_adapters.as_ref().unwrap()[0].bytes.tx, 10);

        assert!(legacy.read(&mut snapshot));
        assert_eq!(snapshot.version, 3);
        assert_eq!(snapshot.entries[0].app.rx, 180);
        assert_eq!(snapshot.entries[0].internet.rx, 0);
        assert!(snapshot.internet_adapters.is_none());
    }
}
