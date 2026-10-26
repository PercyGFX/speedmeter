//! Names and icons for the per-app lists: the same icons Explorer shows, drawn by
//! the shell on a background thread and kept as textures while the window is open.

use super::charts::Lead;
use crate::apps::{self, Resolver};
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
#[cfg(test)]
use eframe::egui::Color32;
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use windows::core::HSTRING;
use windows::Win32::Foundation::{HWND, SIZE};
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT};

/// How an app looks in a list.
pub struct Look {
    pub name: String,
    pub file: String,
    icon: Option<TextureHandle>,
}

enum Slot {
    Loading,
    Ready(Look),
}

type Done = (String, String, String, Option<ColorImage>);

pub struct Icons {
    slots: HashMap<String, Slot>,
    jobs: Option<Sender<(String, u32)>>,
    done: Option<Receiver<Done>>,
}

impl Icons {
    pub fn new() -> Self {
        Self { slots: HashMap::new(), jobs: None, done: None }
    }

    /// Name and icon for an app key, or None while they load. `px` is the icon
    /// size in physical pixels.
    pub fn look(&mut self, ctx: &egui::Context, key: &str, px: u32) -> Option<&Look> {
        self.collect(ctx);
        if !self.slots.contains_key(key) {
            self.request(ctx, key, px);
        }
        match self.slots.get(key) {
            Some(Slot::Ready(look)) => Some(look),
            _ => None,
        }
    }

    /// What to put in front of an app's name: its icon, a symbol, or its initial.
    pub fn lead(&mut self, ctx: &egui::Context, key: &str, name: &str, px: u32, has_glyphs: bool) -> Lead {
        if let Some((_, _, glyph)) = apps::special(key) {
            if has_glyphs {
                return Lead::Glyph(glyph);
            }
        }
        match self.look(ctx, key, px).and_then(|l| l.icon.as_ref()) {
            Some(t) => Lead::Image(t.id()),
            None => Lead::Letter(name.chars().find(|c| c.is_alphanumeric()).map_or('?', |c| c.to_ascii_uppercase())),
        }
    }

    fn request(&mut self, ctx: &egui::Context, key: &str, px: u32) {
        if self.jobs.is_none() {
            let (jobs, rx) = mpsc::channel::<(String, u32)>();
            let (tx, done) = mpsc::channel();
            let ctx = ctx.clone();
            let spawned = std::thread::Builder::new().name("app-icons".into()).spawn(move || worker(rx, tx, ctx));
            if spawned.is_err() {
                return;
            }
            self.jobs = Some(jobs);
            self.done = Some(done);
        }
        if let Some(jobs) = &self.jobs {
            if jobs.send((key.to_string(), px)).is_ok() {
                self.slots.insert(key.to_string(), Slot::Loading);
            }
        }
    }

    fn collect(&mut self, ctx: &egui::Context) {
        let Some(done) = &self.done else { return };
        while let Ok((key, name, file, img)) = done.try_recv() {
            let icon = img.map(|img| ctx.load_texture(format!("app-icon:{key}"), img, TextureOptions::LINEAR));
            self.slots.insert(key, Slot::Ready(Look { name, file, icon }));
        }
    }
}

fn worker(jobs: Receiver<(String, u32)>, done: Sender<Done>, ctx: egui::Context) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let mut resolver = Resolver::new();
    for (key, px) in jobs {
        let (name, file) = resolver.describe(&key, None);
        let img = resolver.icon_source(&key).and_then(|path| unsafe { shell_image(&path, px) });
        if done.send((key, name, file, img)).is_err() {
            break;
        }
        ctx.request_repaint();
    }
}

/// Asks the shell for a file's icon (an exe) or thumbnail (a package's logo image).
unsafe fn shell_image(path: &Path, px: u32) -> Option<ColorImage> {
    let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).ok()?;
    let size = px.clamp(16, 256) as i32;
    let bitmap = factory.GetImage(SIZE { cx: size, cy: size }, SIIGBF_RESIZETOFIT).ok()?;
    let img = to_image(bitmap);
    let _ = DeleteObject(bitmap);
    img
}

unsafe fn to_image(bitmap: HBITMAP) -> Option<ColorImage> {
    let mut bm = BITMAP::default();
    if GetObjectW(bitmap, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 {
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
    if w <= 0 || h <= 0 || w > 1024 || h > 1024 {
        return None;
    }
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down rows
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut px = vec![0u8; (w * h * 4) as usize];
    let dc = GetDC(HWND::default());
    let lines = GetDIBits(dc, bitmap, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
    ReleaseDC(HWND::default(), dc);
    if lines == 0 {
        return None;
    }
    Some(from_bgra(w as usize, h as usize, px))
}

/// Shell bitmaps are BGRA. Thumbnails can come without alpha (all zero) and icons
/// with or without premultiplied alpha, so check rather than assume.
fn from_bgra(w: usize, h: usize, mut px: Vec<u8>) -> ColorImage {
    let has_alpha = px.chunks_exact(4).any(|p| p[3] != 0);
    let premultiplied = has_alpha && px.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]);
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
        if !has_alpha {
            p[3] = 255;
        }
    }
    if premultiplied {
        ColorImage::from_rgba_premultiplied([w, h], &px)
    } else {
        ColorImage::from_rgba_unmultiplied([w, h], &px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_conversion() {
        // Opaque blue pixel in BGRA, no alpha channel at all: becomes opaque.
        let img = from_bgra(1, 1, vec![255, 0, 0, 0]);
        assert_eq!(img.pixels[0], Color32::from_rgb(0, 0, 255));
        // Straight (not premultiplied) alpha: red at half opacity.
        let img = from_bgra(1, 1, vec![0, 0, 255, 128]);
        assert_eq!(img.pixels[0].a(), 128);
        assert!(img.pixels[0].r() < 255);
    }
}
