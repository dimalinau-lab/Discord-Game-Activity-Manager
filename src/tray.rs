use eframe::egui;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

pub struct TrayManager {
    _tray_icon: TrayIcon,
    show_item: MenuItem,
    hide_item: MenuItem,
    next_item: MenuItem,
    stop_item: MenuItem,
    quit_item: MenuItem,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Hide,
    NextGame,
    StopGames,
    Quit,
}

impl TrayManager {
    pub fn new(icon_rgba: &[u8], width: u32, height: u32, ctx: egui::Context) -> Result<Self, String> {
        let icon = Icon::from_rgba(icon_rgba.to_vec(), width, height)
            .map_err(|e| format!("Ошибка загрузки иконки трея: {}", e))?;

        let tray_menu = Menu::new();
        let show_item = MenuItem::new("Открыть", true, None);
        let hide_item = MenuItem::new("Скрыть в трей", true, None);
        let next_item = MenuItem::new("Следующая игра", true, None);
        let stop_item = MenuItem::new("Остановить игры", true, None);
        let quit_item = MenuItem::new("Выход", true, None);

        tray_menu
            .append_items(&[
                &show_item,
                &hide_item,
                &PredefinedMenuItem::separator(),
                &next_item,
                &stop_item,
                &PredefinedMenuItem::separator(),
                &quit_item,
            ])
            .map_err(|e| format!("Ошибка добавления пунктов меню: {}", e))?;

        let tray_icon = TrayIconBuilder::new()
            .with_id("discord-game-farmer-tray")
            .with_menu(Box::new(tray_menu))
            .with_tooltip("Discord Game Activity Manager")
            .with_icon(icon)
            .build()
            .map_err(|e| format!("Ошибка инициализации системного трея: {}", e))?;

        let ctx1 = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |_| {
            ctx1.request_repaint();
        }));

        let ctx2 = ctx.clone();
        MenuEvent::set_event_handler(Some(move |_| {
            ctx2.request_repaint();
        }));

        Ok(Self {
            _tray_icon: tray_icon,
            show_item,
            hide_item,
            next_item,
            stop_item,
            quit_item,
        })
    }

    pub fn set_tooltip(&self, tooltip: &str) {
        let _ = self._tray_icon.set_tooltip(Some(tooltip));
    }

    pub fn update(&self) -> Option<TrayAction> {
        let mut result = None;

        // Опрашиваем события клика по самой иконке в трее
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            match event {
                TrayIconEvent::Click { button, .. } | TrayIconEvent::DoubleClick { button, .. } => {
                    if button == tray_icon::MouseButton::Left {
                        result = Some(TrayAction::Show);
                    }
                }
                _ => {}
            }
        }

        // Опрашиваем события клика по пунктам контекстного меню
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.show_item.id() {
                result = Some(TrayAction::Show);
            } else if event.id == self.hide_item.id() {
                result = Some(TrayAction::Hide);
            } else if event.id == self.next_item.id() {
                result = Some(TrayAction::NextGame);
            } else if event.id == self.stop_item.id() {
                result = Some(TrayAction::StopGames);
            } else if event.id == self.quit_item.id() {
                result = Some(TrayAction::Quit);
            }
        }

        result
    }
}
