//! A private real-time event tracing session with two kernel providers:
//! Microsoft-Windows-Kernel-Network (bytes sent and received, per process) and
//! Microsoft-Windows-Kernel-Process (which exe each process id belongs to).
//!
//! Network events arrive thousands of times a second under load, so their layout
//! is looked up once per event type (through TDH, Windows' own event decoder) and
//! after that read straight from the payload.

use super::counters::Counters;
use std::collections::HashMap;
use std::sync::Mutex;
use windows::core::{w, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_INSUFFICIENT_BUFFER, WIN32_ERROR};
use windows::Win32::System::Diagnostics::Etw::*;

pub const NETWORK: GUID = GUID::from_u128(0x7dd42a49_5329_4832_8dfd_43d979153a88);
pub const PROCESS: GUID = GUID::from_u128(0x22fb2cd6_0e7b_422b_a0c7_2fad1fd0e716);
const SESSION: PCWSTR = w!("SpeedMeter-AppNet");
const SESSION_NAME: &str = "SpeedMeter-AppNet";
/// KERNEL_NETWORK_KEYWORD_IPV4 | KERNEL_NETWORK_KEYWORD_IPV6
const NETWORK_KEYWORDS: u64 = 0x10 | 0x20;
/// WINEVENT_KEYWORD_PROCESS
const PROCESS_KEYWORDS: u64 = 0x10;

/// Where the fields we need sit in one kind of network event.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    received: bool,
    pid: usize,
    size: usize,
    /// (offset, length) of the local and remote addresses.
    local: (usize, usize),
    remote: (usize, usize),
}

pub struct Shared {
    pub counters: Mutex<Counters>,
    layouts: Mutex<HashMap<(u16, u8), Option<Layout>>>,
    /// Kinds of events seen, for the diagnostic log: (provider, id, opcode name).
    pub seen: Mutex<Vec<(char, u16, String)>>,
}

impl Shared {
    pub fn new() -> Self {
        Self { counters: Mutex::new(Counters::new()), layouts: Mutex::new(HashMap::new()), seen: Mutex::new(Vec::new()) }
    }
}

fn properties() -> Vec<u64> {
    // EVENT_TRACE_PROPERTIES followed by room for the session name and a log file name.
    let size = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() + 2 * 1024;
    let mut buf = vec![0u64; size.div_ceil(8)];
    let p = buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
    unsafe {
        (*p).Wnode.BufferSize = size as u32;
        (*p).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        (*p).Wnode.ClientContext = 1; // QPC timestamps
        (*p).LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        (*p).BufferSize = 64; // KB
        (*p).MinimumBuffers = 4;
        (*p).MaximumBuffers = 64;
        (*p).FlushTimer = 1; // seconds: readers see bytes within about a second
        (*p).LoggerNameOffset = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() as u32;
        (*p).LogFileNameOffset = (std::mem::size_of::<EVENT_TRACE_PROPERTIES>() + 1024) as u32;
    }
    buf
}

pub struct Session {
    handle: CONTROLTRACE_HANDLE,
    props: Vec<u64>,
}

fn check(e: WIN32_ERROR, what: &str) -> Result<(), (u32, String)> {
    if e.0 == 0 {
        Ok(())
    } else {
        Err((e.0, format!("{what} failed: {}", windows::core::Error::from(e.to_hresult()).message())))
    }
}

/// Stops a session left over from a crash, if any.
fn stop_by_name() {
    let mut props = properties();
    unsafe {
        let _ = ControlTraceW(CONTROLTRACE_HANDLE { Value: 0 }, SESSION, props.as_mut_ptr() as *mut _, EVENT_TRACE_CONTROL_STOP);
    }
}

impl Session {
    pub fn start() -> Result<Self, (u32, String)> {
        let mut props = properties();
        let mut handle = CONTROLTRACE_HANDLE::default();
        unsafe {
            let mut r = StartTraceW(&mut handle, SESSION, props.as_mut_ptr() as *mut _);
            if r == ERROR_ALREADY_EXISTS {
                stop_by_name();
                props = properties();
                r = StartTraceW(&mut handle, SESSION, props.as_mut_ptr() as *mut _);
            }
            check(r, "Starting the trace session")?;
            let s = Self { handle, props };
            let level = TRACE_LEVEL_INFORMATION as u8;
            let enable = EVENT_CONTROL_CODE_ENABLE_PROVIDER.0;
            check(EnableTraceEx2(s.handle, &NETWORK, enable, level, NETWORK_KEYWORDS, 0, 0, None), "Enabling network events")?;
            check(EnableTraceEx2(s.handle, &PROCESS, enable, level, PROCESS_KEYWORDS, 0, 0, None), "Enabling process events")?;
            Ok(s)
        }
    }

    /// Asks the process provider to describe every process that's already running.
    pub fn request_rundown(&self) {
        unsafe {
            let _ = EnableTraceEx2(self.handle, &PROCESS, EVENT_CONTROL_CODE_CAPTURE_STATE.0, TRACE_LEVEL_INFORMATION as u8, PROCESS_KEYWORDS, 0, 0, None);
        }
    }

    pub fn events_lost(&mut self) -> u64 {
        unsafe {
            let p = self.props.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
            if ControlTraceW(self.handle, PCWSTR::null(), p, EVENT_TRACE_CONTROL_QUERY).0 == 0 {
                (*p).EventsLost as u64 + (*p).RealTimeBuffersLost as u64
            } else {
                0
            }
        }
    }

    pub fn stop(&mut self) {
        unsafe {
            let mut props = properties();
            let _ = ControlTraceW(self.handle, PCWSTR::null(), props.as_mut_ptr() as *mut _, EVENT_TRACE_CONTROL_STOP);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Delivers events to `shared` until the session stops. Blocks.
pub fn consume(shared: &'static Shared) -> Result<(), String> {
    let mut name: Vec<u16> = SESSION_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    let mut log = EVENT_TRACE_LOGFILEW { LoggerName: PWSTR(name.as_mut_ptr()), ..Default::default() };
    log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
    log.Anonymous2.EventRecordCallback = Some(on_event);
    log.Context = shared as *const Shared as *mut _;
    unsafe {
        let h = OpenTraceW(&mut log);
        if h.Value == u64::MAX {
            return Err(format!("Opening the trace failed: {}", windows::core::Error::from_win32().message()));
        }
        let r = ProcessTrace(&[h], None, None);
        let _ = CloseTrace(h);
        if r.0 != 0 {
            return Err(format!("Reading the trace failed: {}", windows::core::Error::from(r.to_hresult()).message()));
        }
    }
    Ok(())
}

unsafe extern "system" fn on_event(rec: *mut EVENT_RECORD) {
    let rec = &*rec;
    let Some(shared) = (rec.UserContext as *const Shared).as_ref() else { return };
    let provider = rec.EventHeader.ProviderId;
    if provider == NETWORK {
        network_event(shared, rec);
    } else if provider == PROCESS {
        process_event(shared, rec);
    }
}

unsafe fn payload(rec: &EVENT_RECORD) -> &[u8] {
    if rec.UserData.is_null() {
        &[]
    } else {
        std::slice::from_raw_parts(rec.UserData as *const u8, rec.UserDataLength as usize)
    }
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

unsafe fn network_event(shared: &Shared, rec: &EVENT_RECORD) {
    let d = &rec.EventHeader.EventDescriptor;
    let layout = {
        let Ok(mut layouts) = shared.layouts.lock() else { return };
        *layouts.entry((d.Id, d.Version)).or_insert_with(|| {
            let (layout, opcode) = network_layout(rec);
            if let Ok(mut seen) = shared.seen.lock() {
                seen.push(('N', d.Id, format!("{opcode} -> {layout:?}")));
            }
            layout
        })
    };
    let Some(l) = layout else { return };
    let data = payload(rec);
    let (Some(pid), Some(size)) = (read_u32(data, l.pid), read_u32(data, l.size)) else { return };
    let (Some(local), Some(remote)) = (data.get(l.local.0..l.local.0 + l.local.1), data.get(l.remote.0..l.remote.0 + l.remote.1)) else {
        return;
    };
    if let Ok(mut c) = shared.counters.lock() {
        c.add(pid, local, remote, size, l.received);
    }
}

/// Reads the event's schema: is it data sent or received, and where are the
/// process id, size and addresses? None for every other kind of network event
/// (connects, disconnects, retransmits, copies), which would double count.
unsafe fn network_layout(rec: &EVENT_RECORD) -> (Option<Layout>, String) {
    let Some(info) = event_info(rec) else { return (None, "no schema".into()) };
    let base = info.as_ptr() as *const u8;
    let ti = &*(base as *const TRACE_EVENT_INFO);
    let opcode = wide_at(base, ti.OpcodeNameOffset);
    let op = opcode.to_ascii_lowercase();
    let received = if op.starts_with("datasent") {
        false
    } else if op.starts_with("datareceived") {
        true
    } else {
        return (None, opcode);
    };
    let pointer = if rec.EventHeader.Flags as u32 & EVENT_HEADER_FLAG_64_BIT_HEADER != 0 { 8 } else { 4 };
    let props = std::slice::from_raw_parts(ti.EventPropertyInfoArray.as_ptr(), ti.TopLevelPropertyCount as usize);
    let (mut pid, mut size, mut saddr, mut daddr) = (None, None, None, None);
    let mut at = 0usize;
    for p in props {
        if p.Flags.0 & (PropertyStruct.0 | PropertyParamLength.0 | PropertyParamCount.0) != 0 {
            break;
        }
        let in_type = p.Anonymous1.nonStructType.InType as i32;
        let len = match _TDH_IN_TYPE(in_type) {
            TDH_INTYPE_INT8 | TDH_INTYPE_UINT8 => 1,
            TDH_INTYPE_INT16 | TDH_INTYPE_UINT16 => 2,
            TDH_INTYPE_INT32 | TDH_INTYPE_UINT32 | TDH_INTYPE_HEXINT32 | TDH_INTYPE_BOOLEAN => 4,
            TDH_INTYPE_INT64 | TDH_INTYPE_UINT64 | TDH_INTYPE_HEXINT64 | TDH_INTYPE_FILETIME => 8,
            TDH_INTYPE_POINTER | TDH_INTYPE_SIZET => pointer,
            TDH_INTYPE_GUID => 16,
            TDH_INTYPE_BINARY => p.Anonymous3.length as usize,
            _ => break,
        };
        let count = if p.Flags.0 & PropertyParamFixedCount.0 != 0 { p.Anonymous2.count.max(1) as usize } else { 1 };
        match wide_at(base, p.NameOffset).as_str() {
            "PID" => pid = Some(at),
            "size" => size = Some(at),
            "saddr" => saddr = Some((at, len)),
            "daddr" => daddr = Some((at, len)),
            _ => {}
        }
        at += len * count;
    }
    // The kernel names the local end "saddr" for both directions.
    let layout = match (pid, size, saddr, daddr) {
        (Some(pid), Some(size), Some(local), Some(remote)) => Some(Layout { received, pid, size, local, remote }),
        _ => None,
    };
    (layout, opcode)
}

unsafe fn event_info(rec: &EVENT_RECORD) -> Option<Vec<u64>> {
    let mut size = 0u32;
    if TdhGetEventInformation(rec, None, None, &mut size) != ERROR_INSUFFICIENT_BUFFER.0 {
        return None;
    }
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    (TdhGetEventInformation(rec, None, Some(buf.as_mut_ptr() as *mut TRACE_EVENT_INFO), &mut size) == 0).then_some(buf)
}

unsafe fn wide_at(base: *const u8, offset: u32) -> String {
    if offset == 0 {
        return String::new();
    }
    let p = base.add(offset as usize) as *const u16;
    let mut n = 0;
    while *p.add(n) != 0 && n < 512 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

unsafe fn property(rec: &EVENT_RECORD, name: PCWSTR) -> Option<Vec<u8>> {
    let desc = [PROPERTY_DATA_DESCRIPTOR { PropertyName: name.0 as u64, ArrayIndex: u32::MAX, Reserved: 0 }];
    let mut size = 0u32;
    if TdhGetPropertySize(rec, None, &desc, &mut size) != 0 || size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    (TdhGetProperty(rec, None, &desc, &mut buf) == 0).then_some(buf)
}

unsafe fn property_u32(rec: &EVENT_RECORD, name: PCWSTR) -> Option<u32> {
    property(rec, name).and_then(|b| b.get(..4).map(|b| u32::from_le_bytes(b.try_into().unwrap())))
}

unsafe fn property_u64(rec: &EVENT_RECORD, name: PCWSTR) -> Option<u64> {
    property(rec, name).and_then(|b| b.get(..8).map(|b| u64::from_le_bytes(b.try_into().unwrap())))
}

unsafe fn property_text(rec: &EVENT_RECORD, name: PCWSTR) -> Option<String> {
    let b = property(rec, name)?;
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
    Some(String::from_utf16_lossy(&w))
}

/// Process start (opcode 1), stop (2) and rundown events.
unsafe fn process_event(shared: &Shared, rec: &EVENT_RECORD) {
    let d = &rec.EventHeader.EventDescriptor;
    let Some(pid) = property_u32(rec, w!("ProcessID")) else { return };
    let created = property_u64(rec, w!("CreateTime")).unwrap_or(0);
    if let Ok(mut seen) = shared.seen.lock() {
        if !seen.iter().any(|(p, id, _)| *p == 'P' && *id == d.Id) {
            seen.push(('P', d.Id, format!("opcode {}", d.Opcode)));
        }
    }
    if d.Opcode == 2 {
        if let Ok(mut c) = shared.counters.lock() {
            c.process_ended(pid, created);
        }
        return;
    }
    let Some(image) = property_text(rec, w!("ImageName")) else { return };
    let session = property_u32(rec, w!("SessionID")).unwrap_or(u32::MAX);
    if let Ok(mut c) = shared.counters.lock() {
        c.process_seen(pid, created, session, image);
    }
}
