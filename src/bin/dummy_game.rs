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
