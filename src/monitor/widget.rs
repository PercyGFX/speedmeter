//! The widget window: either docked inside the taskbar (re-parented into
//! Shell_TrayWnd, the same technique TrafficMonitor uses) or a floating
//! always-on-top window you can drag anywhere.

use super::{MAIN_HWND, WM_APP_WIDGET};
use crate::sys::wide;
use std::cell::Cell;
use std::sync::atomic::Ordering;
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    MapWindowPoints, AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, HDC,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    SetWindowTheme, TTF_IDISHWND, TTF_SUBCLASS, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW, WM_MOUSELEAVE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const EV_HOVER_ON: usize = 1;
pub const EV_HOVER_OFF: usize = 2;
pub const EV_DOUBLE_CLICK: usize = 3;
pub const EV_RIGHT_CLICK: usize = 4;
pub const EV_MOVED: usize = 5;
pub const EV_DESTROYED: usize = 6;

const CLASS: PCWSTR = w!("SpeedMeter.Widget");
const TTM_ADDTOOLW: u32 = WM_USER + 50;
const TTM_UPDATETIPTEXTW: u32 = WM_USER + 57;
const TTM_SETMAXTIPWIDTH: u32 = WM_USER + 24;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
}

fn post(ev: usize) {
    let main = HWND(MAIN_HWND.load(Ordering::Relaxed) as *mut _);
    unsafe {
        let _ = PostMessageW(main, WM_APP_WIDGET, WPARAM(ev), LPARAM(0));
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            if !TRACKING.get() {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
                TRACKING.set(true);
                post(EV_HOVER_ON);
            }
        }
        WM_MOUSELEAVE => {
            TRACKING.set(false);
            post(EV_HOVER_OFF);
        }
        WM_LBUTTONDBLCLK => post(EV_DOUBLE_CLICK),
        WM_LBUTTONDOWN => {
            if GetWindowLongPtrW(hwnd, GWLP_USERDATA) == 1 {
                // Floating: let Windows run the move loop as if the title bar was grabbed.
                let _ = ReleaseCapture();
                SendMessageW(hwnd, WM_NCLBUTTONDOWN, WPARAM(HTCAPTION as usize), LPARAM(0));
                post(EV_MOVED);
            }
        }
        WM_RBUTTONUP => post(EV_RIGHT_CLICK),
        WM_DESTROY => post(EV_DESTROYED),
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

pub fn register_class() {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(proc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

pub struct Widget {
    pub hwnd: HWND,
    pub floating: bool,
    pub parent: HWND,
    rect: Option<RECT>,
    tooltip: HWND,
    tip_text: Vec<u16>,
    tooltip_dark: Option<bool>,
    /// Whether SetWindowPos on the docked window takes screen coordinates. Windows
    /// treats re-parented popups inconsistently, so this is detected on first move.
    screen_coords: bool,
}

impl Widget {
    pub fn create(floating: bool, taskbar: Option<HWND>, owner: HWND) -> Option<Self> {
        unsafe {
            let hinst = GetModuleHandleW(None).ok()?;
            let mut ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
            if floating {
                ex |= WS_EX_TOPMOST;
            }
            let hwnd = CreateWindowExW(ex, CLASS, w!("SpeedMeter widget"), WS_POPUP, 0, 0, 1, 1, None, None, hinst, None).ok()?;
            let mut parent = HWND::default();
            if floating {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 1);
            } else if let Some(tb) = taskbar {
                if SetParent(hwnd, tb).is_err() {
                    let _ = DestroyWindow(hwnd);
                    return None;
                }
                parent = tb;
            }
            let tooltip = Self::make_tooltip(hwnd, owner);
            Some(Self { hwnd, floating, parent, rect: None, tooltip, tip_text: Vec::new(), tooltip_dark: None, screen_coords: false })
        }
    }

    fn make_tooltip(hwnd: HWND, owner: HWND) -> HWND {
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            let Ok(tt) = CreateWindowExW(
                WS_EX_TOPMOST,
                w!("tooltips_class32"),
                None,
                WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                owner,
                None,
                hinst,
                None,
            ) else {
                return HWND::default();
            };
            let mut text = wide("SpeedMeter");
            let ti = TTTOOLINFOW {
                cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
                uFlags: TTF_IDISHWND | TTF_SUBCLASS,
                hwnd,
                uId: hwnd.0 as usize,
                lpszText: PWSTR(text.as_mut_ptr()),
                ..Default::default()
            };
            SendMessageW(tt, TTM_ADDTOOLW, WPARAM(0), LPARAM(&ti as *const _ as isize));
            SendMessageW(tt, TTM_SETMAXTIPWIDTH, WPARAM(0), LPARAM(480));
            tt
        }
    }

    pub fn set_tooltip(&mut self, text: &str) {
        if self.tooltip.is_invalid() {
            return;
        }
        let w = wide(text);
        if w == self.tip_text {
            return;
        }
        self.tip_text = w;
        unsafe {
            let ti = TTTOOLINFOW {
                cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
                hwnd: self.hwnd,
                uId: self.hwnd.0 as usize,
                lpszText: PWSTR(self.tip_text.as_mut_ptr()),
                ..Default::default()
            };
            SendMessageW(self.tooltip, TTM_UPDATETIPTEXTW, WPARAM(0), LPARAM(&ti as *const _ as isize));
        }
    }

    /// Matches the tooltip to the system theme (a white tooltip on a dark taskbar is jarring).
    pub fn set_dark(&mut self, dark: bool) {
        if self.tooltip.is_invalid() || self.tooltip_dark == Some(dark) {
            return;
        }
        self.tooltip_dark = Some(dark);
        unsafe {
            let theme = if dark { w!("DarkMode_Explorer") } else { w!("Explorer") };
            let _ = SetWindowTheme(self.tooltip, theme, None);
        }
    }

    pub fn alive(&self) -> bool {
        unsafe {
            if !IsWindow(self.hwnd).as_bool() {
                return false;
            }
            self.floating || GetAncestor(self.hwnd, GA_PARENT) == self.parent
        }
    }

    pub fn screen_rect(&self) -> RECT {
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut r);
        }
        r
    }

    /// Moves/resizes the docked widget to `r` (screen coordinates).
    pub fn place_docked(&mut self, r: RECT) {
        if self.rect == Some(r) {
            self.keep_on_top();
            return;
        }
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        for _ in 0..2 {
            let (x, y) = if self.screen_coords {
                (r.left, r.top)
            } else {
                let mut p = [POINT { x: r.left, y: r.top }];
                unsafe { MapWindowPoints(None, self.parent, &mut p) };
                (p[0].x, p[0].y)
            };
            unsafe {
                let _ = SetWindowPos(self.hwnd, HWND_TOP, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
            let got = self.screen_rect();
            if got.left == r.left && got.top == r.top {
                break;
            }
            self.screen_coords = !self.screen_coords;
        }
        self.rect = Some(r);
    }

    /// The taskbar's own content can end up above us after Explorer relayouts.
    fn keep_on_top(&self) {
        unsafe {
            if GetWindow(self.parent, GW_CHILD).ok() != Some(self.hwnd) {
                let _ = SetWindowPos(self.hwnd, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    pub fn place_floating(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let r = RECT { left: x, top: y, right: x + w, bottom: y + h };
        if self.rect == Some(r) {
            return;
        }
        unsafe {
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
        self.rect = Some(r);
    }

    /// Resize in place, keeping wherever the user dragged it.
    pub fn resize_floating(&mut self, w: i32, h: i32) {
        let cur = self.screen_rect();
        self.place_floating(cur.left, cur.top, w, h);
    }

    pub fn set_visible(&self, visible: bool) {
        unsafe {
            let shown = IsWindowVisible(self.hwnd).as_bool();
            if shown != visible {
                let _ = ShowWindow(self.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
    }

    pub fn present(&self, dc: HDC, w: i32, h: i32) -> windows::core::Result<()> {
        let size = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe { UpdateLayeredWindow(self.hwnd, None, None, Some(&size), dc, Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA) }
    }
}

impl Drop for Widget {
    fn drop(&mut self) {
        unsafe {
            if !self.tooltip.is_invalid() {
                let _ = DestroyWindow(self.tooltip);
            }
            if IsWindow(self.hwnd).as_bool() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}
