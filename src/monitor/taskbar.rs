//! Locating the Windows 11 taskbar and working out where the widget goes on it.

use super::layout::Layout;
use crate::config::Placement;
use windows::core::w;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW, GetWindowRect};

pub struct Taskbar {
    pub hwnd: HWND,
    pub rect: RECT,
    /// The notification area (tray icons + clock), in screen coordinates.
    pub tray: Option<RECT>,
    /// The running-apps strip; useful when the taskbar is left aligned.
    pub tasks: Option<RECT>,
    pub dpi: u32,
}

fn rect_of(h: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(h, &mut r).ok()? };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

pub fn find() -> Option<Taskbar> {
    unsafe {
        let hwnd = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
        if hwnd.is_invalid() {
            return None;
        }
        let rect = rect_of(hwnd)?;
        let tray = FindWindowExW(hwnd, None, w!("TrayNotifyWnd"), None).ok().and_then(rect_of);
        let tasks = FindWindowExW(hwnd, None, w!("ReBarWindow32"), None)
            .ok()
            .and_then(|rebar| FindWindowExW(rebar, None, w!("MSTaskSwWClass"), None).ok())
            .and_then(rect_of);
        let dpi = match GetDpiForWindow(hwnd) {
            0 => 96,
            d => d,
        };
        Some(Taskbar { hwnd, rect, tray, tasks, dpi })
    }
}

impl Taskbar {
    pub fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }

    pub fn height(&self) -> i32 {
        self.rect.bottom - self.rect.top
    }

    /// Screen rectangle for a widget `width` pixels wide. `layout` holds the real
    /// button positions from UI Automation, when available.
    pub fn slot(&self, placement: Placement, width: i32, offset_dip: i32, layout: Option<&Layout>) -> RECT {
        let s = self.scale();
        let px = |dip: f32| (dip * s).round() as i32;
        let r = self.rect;
        let offset = px(offset_dip as f32);
        let tray_left = layout
            .and_then(|l| l.tray_left)
            .or(self.tray.map(|t| t.left))
            .unwrap_or(r.right - px(240.0));
        let x = match placement {
            Placement::TaskbarLeft => {
                let base = match layout {
                    // First gap from the left that fits: after the last app icon on a
                    // left-aligned taskbar, after the Widgets button on a centered one.
                    Some(l) if !l.buttons.is_empty() => {
                        let gap = px(6.0);
                        let mut buttons: Vec<RECT> =
                            l.buttons.iter().copied().filter(|b| b.left < tray_left).collect();
                        buttons.sort_by_key(|b| b.left);
                        let mut x = r.left + px(8.0);
                        for b in &buttons {
                            if b.left < x + width + gap && b.right + gap > x {
                                x = b.right + gap;
                            }
                        }
                        x
                    }
                    _ if crate::sys::taskbar_centered() => r.left + px(12.0),
                    _ => self.tasks.map(|t| t.right + px(6.0)).unwrap_or(r.left + px(60.0)),
                };
                base + offset
            }
            _ => tray_left - width - offset,
        };
        let x = x.clamp(r.left, (r.right - width).max(r.left));
        RECT { left: x, top: r.top, right: x + width, bottom: r.bottom }
    }
}
