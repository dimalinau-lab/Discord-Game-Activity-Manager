#![windows_subsystem = "windows"]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_DESTROY => {
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

        // Скрытое окно (WS_EX_TOOLWINDOW исключает появление кнопки на панели задач и в Alt+Tab;
        // WS_POPUP и координаты -32000,-32000 исключают показ окна на экране.
        // Окно остаётся зарегистрированным в Win32 и определяется Discord.
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            window_title.as_ptr(),
            WS_POPUP,
            -32000,
            -32000,
            1,
            1,
            0,
            0,
            instance,
            ptr::null(),
        );

        if hwnd != 0 {
            ShowWindow(hwnd, SW_HIDE);
        }

        // Легковесный цикл обработки сообщений Windows (0% CPU, ~1-2 МБ RAM)
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
