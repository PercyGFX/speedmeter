//! Reads where the Windows 11 taskbar's buttons actually are, via UI Automation.
//!
//! The XAML taskbar draws its own Start/Search/app buttons, and the legacy Win32
//! child windows no longer report their positions. UI Automation does. The query
//! runs on its own thread so the widget's UI thread never waits on Explorer.

use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::core::w;
use windows::Win32::Foundation::RECT;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_AutomationIdPropertyId,
    UIA_BoundingRectanglePropertyId, UIA_ClassNamePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::FindWindowW;

#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// Start, Search, Task View, Widgets and app buttons, in screen coordinates.
    pub buttons: Vec<RECT>,
    /// Left edge of the system tray (hidden-icons chevron), if found.
    pub tray_left: Option<i32>,
}

pub type Shared = Arc<Mutex<Option<Layout>>>;

fn query(uia: &IUIAutomation) -> windows::core::Result<Layout> {
    unsafe {
        let hwnd = FindWindowW(w!("Shell_TrayWnd"), None)?;
        let root = uia.ElementFromHandle(hwnd)?;
        let cache = uia.CreateCacheRequest()?;
        cache.AddProperty(UIA_ClassNamePropertyId)?;
        cache.AddProperty(UIA_AutomationIdPropertyId)?;
        cache.AddProperty(UIA_BoundingRectanglePropertyId)?;
        let all = root.FindAllBuildCache(TreeScope_Descendants, &uia.CreateTrueCondition()?, &cache)?;
        let mut layout = Layout::default();
        for i in 0..all.Length()? {
            let e = all.GetElement(i)?;
            let class = e.CachedClassName().map(|s| s.to_string()).unwrap_or_default();
            let id = e.CachedAutomationId().map(|s| s.to_string()).unwrap_or_default();
            let Ok(r) = e.CachedBoundingRectangle() else { continue };
            if r.right <= r.left {
                continue;
            }
            let is_button = class == "Taskbar.TaskListButtonAutomationPeer"
                || (class == "ToggleButton" && id.ends_with("Button"))
                || id == "WidgetsButton";
            if is_button {
                layout.buttons.push(r);
            } else if class.starts_with("SystemTray.") {
                layout.tray_left = Some(layout.tray_left.map_or(r.left, |l| l.min(r.left)));
            }
        }
        Ok(layout)
    }
}

/// Starts the background reader. The result is refreshed every 1.5 seconds.
pub fn spawn() -> Shared {
    let shared: Shared = Arc::new(Mutex::new(None));
    let out = shared.clone();
    let _ = std::thread::Builder::new().name("taskbar-layout".into()).spawn(move || unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            return;
        }
        let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) else {
            return;
        };
        loop {
            let layout = query(&uia).ok();
            if let Ok(mut g) = out.lock() {
                *g = layout;
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
    });
    shared
}
