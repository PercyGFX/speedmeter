//! Notification-area icon.

use super::WM_APP_TRAY;
use crate::sys::copy_wide;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{LoadIconMetric, LIM_SMALL};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO,
    NIIF_RESPECT_QUIET_TIME, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICONDATAW, NOTIFYICONDATAW_0, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::HICON;
use std::time::{Duration, Instant};

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
    added: bool,
    tip: String,
    /// Shown only for a notification (the user hides the icon); remove it after this.
    temporary_until: Option<Instant>,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Self {
        let icon = unsafe {
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            LoadIconMetric(hinst, PCWSTR(1 as *const u16), LIM_SMALL).unwrap_or_default()
        };
        Self { hwnd, icon, added: false, tip: String::new(), temporary_until: None }
    }

    /// Shows a Windows notification from the tray icon. If the icon is hidden it
    /// appears for a short while, since Windows needs it to show the notification.
    pub fn notify(&mut self, title: &str, text: &str, warning: bool) {
        if !self.added {
            self.set_visible(true);
            self.temporary_until = Some(Instant::now() + Duration::from_secs(30));
        }
        let mut nid = self.data();
        nid.uFlags |= NIF_INFO;
        copy_wide(&mut nid.szInfoTitle, title);
        copy_wide(&mut nid.szInfo, text);
        nid.dwInfoFlags = (if warning { NIIF_WARNING } else { NIIF_INFO }) | NIIF_RESPECT_QUIET_TIME;
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// Takes away an icon that was only there for a notification, once it's done.
    /// `wanted` is whether the user wants the icon anyway.
    pub fn tidy(&mut self, wanted: bool) {
        if let Some(until) = self.temporary_until {
            if wanted {
                self.temporary_until = None;
            } else if Instant::now() >= until {
                self.temporary_until = None;
                self.set_visible(false);
            }
        }
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: self.icon,
            Anonymous: NOTIFYICONDATAW_0 { uVersion: NOTIFYICON_VERSION_4 },
            ..Default::default()
        };
        copy_wide(&mut nid.szTip, if self.tip.is_empty() { "SpeedMeter" } else { &self.tip });
        nid
    }

    pub fn set_visible(&mut self, visible: bool) {
        unsafe {
            if visible && !self.added {
                let nid = self.data();
                if Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                    let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
                    self.added = true;
                }
            } else if !visible && self.added {
                let _ = Shell_NotifyIconW(NIM_DELETE, &self.data());
                self.added = false;
            }
        }
    }

    /// Explorer restarted: the old icon is gone, add it again.
    pub fn recreate(&mut self, visible: bool) {
        self.added = false;
        self.set_visible(visible);
    }

    pub fn set_tip(&mut self, tip: &str) {
        if tip == self.tip {
            return;
        }
        self.tip = tip.to_string();
        if self.added {
            unsafe {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &self.data());
            }
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.set_visible(false);
    }
}
