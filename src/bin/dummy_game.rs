#![windows_subsystem = "windows"]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const ICON_POLL_TIMER_ID: usize = 1;
const MAX_ICON_POLL_TICKS: u32 = 30;

static HICON_BIG: AtomicIsize = AtomicIsize::new(0);
static HICON_SMALL: AtomicIsize = AtomicIsize::new(0);
static POLL_TICKS: AtomicU32 = AtomicU32::new(0);

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn parse_rgb_env(var: &str, default: [u8; 3]) -> [u8; 3] {
    let Ok(val) = std::env::var(var) else {
        return default;
    };
    let mut parts = val.split(',').filter_map(|s| s.trim().parse::<u8>().ok());
    match (parts.next(), parts.next(), parts.next()) {
        (Some(r), Some(g), Some(b)) => [r, g, b],
        _ => default,
    }
}

fn build_fallback_image(bg: [u8; 3], fg: [u8; 3], size: u32) -> image::RgbaImage {
    let mut img = image::RgbaImage::new(size, size);
    let radius = (size as f32 * 0.16).max(4.0);
    let stripe_y = (size as f32 * 0.76) as u32;
    let s = size as f32;

    for (x, y, pixel) in img.enumerate_pixels_mut() {
        let in_rect = if (x as f32) < radius && (y as f32) < radius {
            let dx = radius - (x as f32 + 0.5);
            let dy = radius - (y as f32 + 0.5);
            (dx * dx + dy * dy).sqrt() <= radius
        } else if (x as f32) >= s - radius && (y as f32) < radius {
            let dx = (x as f32 + 0.5) - (s - radius);
            let dy = radius - (y as f32 + 0.5);
            (dx * dx + dy * dy).sqrt() <= radius
        } else if (x as f32) < radius && (y as f32) >= s - radius {
            let dx = radius - (x as f32 + 0.5);
            let dy = (y as f32 + 0.5) - (s - radius);
            (dx * dx + dy * dy).sqrt() <= radius
        } else if (x as f32) >= s - radius && (y as f32) >= s - radius {
            let dx = (x as f32 + 0.5) - (s - radius);
            let dy = (y as f32 + 0.5) - (s - radius);
            (dx * dx + dy * dy).sqrt() <= radius
        } else {
            true
        };

        if !in_rect {
            *pixel = image::Rgba([0, 0, 0, 0]);
            continue;
        }
        let in_center = x >= size / 4 && x < size * 3 / 4 && y >= size / 4 && y < size * 3 / 4;
        let [mut r, mut g, mut b] = if in_center { fg } else { bg };
        if y >= stripe_y {
            r = (r as u16 * 8 / 10) as u8;
            g = (g as u16 * 8 / 10) as u8;
            b = (b as u16 * 8 / 10) as u8;
        }
        *pixel = image::Rgba([r, g, b, 255]);
    }
    img
}

unsafe fn create_hicon_from_rgba(rgba: &image::RgbaImage, target_px: i32) -> isize {
    let scaled = image::imageops::resize(
        rgba,
        target_px as u32,
        target_px as u32,
        image::imageops::FilterType::Lanczos3,
    );
    let mut png_bytes = Vec::new();
    if scaled
        .write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png)
        .is_err()
    {
        return 0;
    }
    CreateIconFromResourceEx(
        png_bytes.as_ptr(),
        png_bytes.len() as u32,
        1,
        0x0003_0000,
        target_px,
        target_px,
        LR_DEFAULTCOLOR,
    )
}

unsafe fn apply_rgba_icon(hwnd: HWND, rgba: &image::RgbaImage) {
    let big_px = GetSystemMetrics(SM_CXICON).max(48);
    let small_px = GetSystemMetrics(SM_CXSMICON).max(16);
    let new_big = create_hicon_from_rgba(rgba, big_px);
    let new_small = create_hicon_from_rgba(rgba, small_px);

    if new_big != 0 {
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, new_big);
        let old = HICON_BIG.swap(new_big, Ordering::SeqCst);
        if old != 0 {
            DestroyIcon(old);
        }
    }
    if new_small != 0 {
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, new_small);
        let old = HICON_SMALL.swap(new_small, Ordering::SeqCst);
        if old != 0 {
            DestroyIcon(old);
        }
    }
}

fn try_load_png_icon(path: &PathBuf) -> Option<image::RgbaImage> {
    let bytes = std::fs::read(path).ok()?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?;
    Some(img.to_rgba8())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER if wparam == ICON_POLL_TIMER_ID => {
            let ticks = POLL_TICKS.fetch_add(1, Ordering::Relaxed) + 1;
            if let Ok(path_str) = std::env::var("DGF_ICON_PNG") {
                if let Some(rgba) = try_load_png_icon(&PathBuf::from(path_str)) {
                    apply_rgba_icon(hwnd, &rgba);
                    KillTimer(hwnd, ICON_POLL_TIMER_ID);
                    return 0;
                }
            }
            if ticks >= MAX_ICON_POLL_TICKS {
                KillTimer(hwnd, ICON_POLL_TIMER_ID);
            }
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, ICON_POLL_TIMER_ID);
            let old_big = HICON_BIG.swap(0, Ordering::SeqCst);
            if old_big != 0 {
                DestroyIcon(old_big);
            }
            let old_small = HICON_SMALL.swap(0, Ordering::SeqCst);
            if old_small != 0 {
                DestroyIcon(old_small);
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let title = if args.is_empty() {
        "Game Process".to_string()
    } else {
        args.join(" ")
    };

    unsafe {
        let instance = GetModuleHandleW(ptr::null());
        let class_name = to_wide("DiscordGameDummyClass");
        let window_title = to_wide(&title);

        let wnd_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: 0,
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: 0,
        };

        RegisterClassExW(&wnd_class);

        // Компактное окно 320x200 (WS_EX_APPWINDOW + SW_SHOWNOACTIVATE: как в рабочей версии до 07.11,
        // гарантирует распознавание процесса и окна в Discord RunningGameStore без перехвата фокуса)
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name.as_ptr(),
            window_title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            320,
            200,
            0,
            0,
            instance,
            ptr::null(),
        );

        if hwnd != 0 {
            let loaded_png = std::env::var("DGF_ICON_PNG")
                .ok()
                .and_then(|p| try_load_png_icon(&PathBuf::from(p)));

            if let Some(rgba) = loaded_png {
                apply_rgba_icon(hwnd, &rgba);
            } else {
                let bg = parse_rgb_env("DGF_ICON_BG", [74, 222, 128]);
                let fg = parse_rgb_env("DGF_ICON_FG", [14, 22, 18]);
                let fallback = build_fallback_image(bg, fg, 64);
                apply_rgba_icon(hwnd, &fallback);
                if std::env::var("DGF_ICON_PNG").is_ok() {
                    SetTimer(hwnd, ICON_POLL_TIMER_ID, 500, None);
                }
            }

            // SW_SHOWNOACTIVATE (4): окно видимо для Discord (IsWindowVisible == TRUE, !IsIconic),
            // но не перехватывает фокус пользователя.
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

        // Легковесный цикл обработки сообщений Windows (0% CPU, ~1-2 МБ RAM)
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
