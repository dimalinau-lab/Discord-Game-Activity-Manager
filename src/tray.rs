use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use eframe::egui;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
    Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TrayAction {
    Show,
    Hide,
    NextGame,
    StopGames,
    Quit,
}

fn map_tray_icon_event(event: &TrayIconEvent) -> Option<TrayAction> {
    match event {
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
        | TrayIconEvent::DoubleClick {
            button: MouseButton::Left,
            ..
        } => Some(TrayAction::Show),
        _ => None,
    }
}

pub struct TrayManager {
    action_rx: mpsc::Receiver<TrayAction>,
    pending_tooltip: Arc<Mutex<Option<String>>>,
    last_tooltip: Mutex<String>,
    shutdown: Arc<AtomicBool>,
    #[cfg(windows)]
    tray_hwnd: isize,
    thread: Option<JoinHandle<()>>,
}

impl TrayManager {
    pub fn new(
        icon_rgba: &[u8],
        width: u32,
        height: u32,
        ctx: egui::Context,
        main_hwnd: isize,
        on_hidden_quit: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let (action_tx, action_rx) = mpsc::channel::<TrayAction>();
        let pending_tooltip: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let shutdown = Arc::new(AtomicBool::new(false));
        let hidden_quit = Arc::new(AtomicBool::new(false));

        let (init_tx, init_rx) = mpsc::sync_channel::<Result<isize, String>>(1);

        let rgba = icon_rgba.to_vec();
        let pending_tip_clone = Arc::clone(&pending_tooltip);
        let shutdown_clone = Arc::clone(&shutdown);
        let hidden_quit_clone = Arc::clone(&hidden_quit);

        let handle = std::thread::Builder::new()
            .name("tray-loop".to_string())
            .spawn(move || {
                let icon = match Icon::from_rgba(rgba, width, height) {
                    Ok(i) => i,
                    Err(e) => {
                        let _ = init_tx.send(Err(format!("Ошибка загрузки иконки трея: {}", e)));
                        return;
                    }
                };

                let tray_menu = Menu::new();
                let show_item = MenuItem::new("Открыть", true, None);
                let hide_item = MenuItem::new("Скрыть в трей", true, None);
                let next_item = MenuItem::new("Следующая игра", true, None);
                let stop_item = MenuItem::new("Остановить игры", true, None);
                let quit_item = MenuItem::new("Выход", true, None);

                let show_id: MenuId = show_item.id().clone();
                let hide_id: MenuId = hide_item.id().clone();
                let next_id: MenuId = next_item.id().clone();
                let stop_id: MenuId = stop_item.id().clone();
                let quit_id: MenuId = quit_item.id().clone();

                if let Err(e) = tray_menu.append_items(&[
                    &show_item,
                    &hide_item,
                    &PredefinedMenuItem::separator(),
                    &next_item,
                    &stop_item,
                    &PredefinedMenuItem::separator(),
                    &quit_item,
                ]) {
                    let _ = init_tx.send(Err(format!("Ошибка добавления пунктов меню: {}", e)));
                    return;
                }

                let tray_icon = match TrayIconBuilder::new()
                    .with_id("discord-game-farmer-tray")
                    .with_menu(Box::new(tray_menu))
                    .with_tooltip("Discord Game Activity Manager")
                    .with_icon(icon)
                    .with_menu_on_left_click(false)
                    .with_menu_on_right_click(true)
                    .build()
                {
                    Ok(ti) => ti,
                    Err(e) => {
                        let _ = init_tx.send(Err(format!(
                            "Ошибка инициализации системного трея: {}",
                            e
                        )));
                        return;
                    }
                };

                #[cfg(windows)]
                let tray_hwnd = tray_icon.window_handle() as isize;
                #[cfg(not(windows))]
                let tray_hwnd = 0isize;

                // Обработчики событий ДОЛЖНЫ пересылать события сами,
                // т.к. set_event_handler(Some(...)) отключает встроенный receiver() в tray-icon/muda.
                let tx_click = action_tx.clone();
                let ctx_click = ctx.clone();
                TrayIconEvent::set_event_handler(Some(move |event| {
                    if let Some(action) = map_tray_icon_event(&event) {
                        let _ = tx_click.send(action);
                        crate::wake_and_show_window(main_hwnd, &ctx_click);
                    }
                }));

                let tx_menu = action_tx.clone();
                let ctx_menu = ctx.clone();
                let shutdown_menu = Arc::clone(&shutdown_clone);
                let hidden_quit_menu = Arc::clone(&hidden_quit_clone);
                MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                    if event.id == show_id {
                        let _ = tx_menu.send(TrayAction::Show);
                        crate::wake_and_show_window(main_hwnd, &ctx_menu);
                    } else if event.id == hide_id {
                        let _ = tx_menu.send(TrayAction::Hide);
                        ctx_menu.request_repaint();
                    } else if event.id == next_id {
                        let _ = tx_menu.send(TrayAction::NextGame);
                        ctx_menu.request_repaint();
                    } else if event.id == stop_id {
                        let _ = tx_menu.send(TrayAction::StopGames);
                        ctx_menu.request_repaint();
                    } else if event.id == quit_id {
                        if crate::is_window_visible_and_unminimized(main_hwnd) {
                            let _ = tx_menu.send(TrayAction::Quit);
                            ctx_menu.request_repaint();
                        } else {
                            // При скрытом окне eframe не доставляет WM_PAINT в App::update;
                            // завершаем цикл трея здесь, чтобы удалить иконку и выйти без мигания окна.
                            hidden_quit_menu.store(true, Ordering::Release);
                            shutdown_menu.store(true, Ordering::Release);
                        }
                    }
                }));

                let _ = init_tx.send(Ok(tray_hwnd));

                #[cfg(windows)]
                {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        DispatchMessageW, GetMessageW, TranslateMessage, MSG,
                    };

                    let mut msg: MSG = unsafe { std::mem::zeroed() };
                    while !shutdown_clone.load(Ordering::Acquire) {
                        let ret = unsafe { GetMessageW(&mut msg, 0, 0, 0) };
                        if ret <= 0 {
                            break;
                        }
                        unsafe {
                            TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }

                        if let Ok(mut pending) = pending_tip_clone.lock() {
                            if let Some(tip) = pending.take() {
                                let _ = tray_icon.set_tooltip(Some(tip));
                            }
                        }
                    }
                }

                #[cfg(not(windows))]
                while !shutdown_clone.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }

                // Явно держим пункты меню живыми до конца цикла сообщений
                drop(show_item);
                drop(hide_item);
                drop(next_item);
                drop(stop_item);
                drop(quit_item);
                drop(tray_icon);

                if hidden_quit_clone.load(Ordering::Acquire) {
                    on_hidden_quit();
                }
            })
            .map_err(|e| format!("Ошибка запуска потока трея: {}", e))?;

        let tray_hwnd = match init_rx.recv() {
            Ok(Ok(hwnd)) => hwnd,
            Ok(Err(err)) => {
                let _ = handle.join();
                return Err(err);
            }
            Err(_) => {
                let _ = handle.join();
                return Err("Поток трея завершился до инициализации".to_string());
            }
        };

        Ok(Self {
            action_rx,
            pending_tooltip,
            last_tooltip: Mutex::new("Discord Game Activity Manager".to_string()),
            shutdown,
            #[cfg(windows)]
            tray_hwnd,
            thread: Some(handle),
        })
    }

    pub fn set_tooltip(&self, tooltip: &str) {
        if let Ok(mut last) = self.last_tooltip.lock() {
            if *last == tooltip {
                return;
            }
            *last = tooltip.to_string();
        }
        if let Ok(mut pending) = self.pending_tooltip.lock() {
            *pending = Some(tooltip.to_string());
        }
        #[cfg(windows)]
        if self.tray_hwnd != 0 {
            use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_NULL};
            unsafe {
                PostMessageW(self.tray_hwnd, WM_NULL, 0, 0);
            }
        }
    }

    pub fn update(&self) -> Option<TrayAction> {
        let mut result = None;
        while let Ok(action) = self.action_rx.try_recv() {
            result = Some(action);
        }
        result
    }
}

impl Drop for TrayManager {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        #[cfg(windows)]
        if self.tray_hwnd != 0 {
            use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_NULL};
            unsafe {
                PostMessageW(self.tray_hwnd, WM_NULL, 0, 0);
            }
        }
        if let Some(handle) = self.thread.take() {
            if std::thread::current().id() != handle.thread().id() {
                let _ = handle.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tray_icon::{dpi::PhysicalPosition, Rect, TrayIconId};

    fn dummy_rect() -> Rect {
        Rect {
            position: PhysicalPosition::new(0.0, 0.0),
            size: tray_icon::dpi::PhysicalSize::new(16, 16),
        }
    }

    #[test]
    fn left_click_up_and_double_click_map_to_show() {
        let up = TrayIconEvent::Click {
            id: TrayIconId::new("test"),
            position: PhysicalPosition::new(0.0, 0.0),
            rect: dummy_rect(),
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
        };
        assert_eq!(map_tray_icon_event(&up), Some(TrayAction::Show));

        let down = TrayIconEvent::Click {
            id: TrayIconId::new("test"),
            position: PhysicalPosition::new(0.0, 0.0),
            rect: dummy_rect(),
            button: MouseButton::Left,
            button_state: MouseButtonState::Down,
        };
        assert_eq!(map_tray_icon_event(&down), None);

        let right_up = TrayIconEvent::Click {
            id: TrayIconId::new("test"),
            position: PhysicalPosition::new(0.0, 0.0),
            rect: dummy_rect(),
            button: MouseButton::Right,
            button_state: MouseButtonState::Up,
        };
        assert_eq!(map_tray_icon_event(&right_up), None);

        let dbl = TrayIconEvent::DoubleClick {
            id: TrayIconId::new("test"),
            position: PhysicalPosition::new(0.0, 0.0),
            rect: dummy_rect(),
            button: MouseButton::Left,
        };
        assert_eq!(map_tray_icon_event(&dbl), Some(TrayAction::Show));
    }
}
