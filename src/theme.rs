use eframe::egui::{self, Color32, RichText, Rounding, Stroke, Vec2};

/// Палитра и геометрические токены: чистый чёрный фон + чёткая белая окантовка контролов.
pub const BG_BASE: Color32 = Color32::from_rgb(0, 0, 0);            // #000000
pub const BG_SURFACE: Color32 = Color32::from_rgb(5, 5, 7);         // #050507
pub const BG_SURFACE_HOVER: Color32 = Color32::from_rgb(14, 15, 19);// #0E0F13
pub const BG_INPUT: Color32 = Color32::from_rgb(0, 0, 0);           // #000000
pub const BG_FOOTER: Color32 = Color32::from_rgb(0, 0, 0);          // #000000

pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(72, 74, 82);   // Чёткие разделители сетки
pub const BORDER_STRONG: Color32 = Color32::from_rgb(205, 209, 218);// Тонкая белая окантовка кнопок и полей
pub const BORDER_HOVER: Color32 = Color32::WHITE;                   // Яркая белая окантовка при наведении
pub const BTN_STROKE_W: f32 = 0.6;                                  // Тонкая обводка контролов

pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(243, 244, 246); // #F3F4F6
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(156, 163, 175);// #9CA3AF
pub const TEXT_MUTED: Color32 = Color32::from_rgb(107, 114, 128);   // #6B7280

pub const WHITE_BTN_BG: Color32 = Color32::from_rgb(245, 246, 248);
pub const WHITE_BTN_FG: Color32 = Color32::from_rgb(12, 13, 16);

pub const BANNER_BLUE_BG: Color32 = Color32::from_rgb(12, 24, 43);    // #0C182B
pub const BANNER_BLUE_BORDER: Color32 = Color32::from_rgb(31, 78, 140);// #1F4E8C
pub const ACCENT_BLUE: Color32 = Color32::from_rgb(29, 111, 232);    // #1D6FE8

pub const BADGE_AMBER_BG: Color32 = Color32::from_rgb(36, 28, 14);   // #241C0E
pub const BADGE_AMBER_BORDER: Color32 = Color32::from_rgb(102, 75, 27);// #664B1B
pub const BADGE_AMBER_TEXT: Color32 = Color32::from_rgb(229, 169, 60);// #E5A93C

pub const ACTIVE_GREEN_BORDER: Color32 = Color32::from_rgb(30, 92, 58);// #1E5C3A
pub const ACTIVE_GREEN_TEXT: Color32 = Color32::from_rgb(52, 211, 153);// #34D399

pub const STOP_RED_BORDER: Color32 = Color32::from_rgb(220, 38, 38); // #DC2626
pub const GRAY_BTN_BG: Color32 = Color32::from_rgb(54, 57, 66);      // #363942
pub const GRAY_BTN_BORDER: Color32 = Color32::from_rgb(138, 142, 152);

/// Настраивает глобальную тему egui под строгий монохромный интерфейс.
pub fn apply_artcraft_visuals(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let mut visuals = egui::Visuals::dark();

    visuals.window_fill = BG_BASE;
    visuals.panel_fill = BG_BASE;
    visuals.extreme_bg_color = BG_INPUT;
    visuals.faint_bg_color = BG_SURFACE;
    visuals.code_bg_color = BG_INPUT;

    visuals.window_rounding = Rounding::same(0.0);
    visuals.menu_rounding = Rounding::same(2.0);
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER_SUBTLE);

    visuals.widgets.noninteractive.bg_fill = BG_SURFACE;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER_SUBTLE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_SECONDARY);
    visuals.widgets.noninteractive.rounding = Rounding::same(2.0);

    visuals.widgets.inactive.bg_fill = BG_INPUT;
    visuals.widgets.inactive.weak_bg_fill = BG_INPUT;
    visuals.widgets.inactive.bg_stroke = Stroke::new(BTN_STROKE_W, BORDER_STRONG);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    visuals.widgets.inactive.rounding = Rounding::same(2.0);

    visuals.widgets.hovered.bg_fill = Color32::from_rgb(16, 18, 22);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(16, 18, 22);
    visuals.widgets.hovered.bg_stroke = Stroke::new(BTN_STROKE_W, BORDER_HOVER);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.hovered.rounding = Rounding::same(2.0);

    visuals.widgets.active.bg_fill = Color32::from_rgb(34, 37, 46);
    visuals.widgets.active.bg_stroke = Stroke::new(BTN_STROKE_W, TEXT_PRIMARY);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.active.rounding = Rounding::same(2.0);

    visuals.selection.bg_fill = Color32::from_rgb(29, 78, 160);
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT_BLUE);

    style.visuals = visuals;
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.scroll = egui::style::ScrollStyle {
        floating: true,
        bar_width: 7.0,
        handle_min_length: 28.0,
        bar_inner_margin: 4.0,
        bar_outer_margin: 2.0,
        floating_width: 5.0,
        floating_allocated_width: 10.0,
        foreground_color: true,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.15,
        interact_background_opacity: 0.25,
        dormant_handle_opacity: 0.45,
        active_handle_opacity: 0.70,
        interact_handle_opacity: 1.0,
    };
    ctx.set_style(style);
}

/// Возвращает контрастную пару цветов (фон, акцент) для процедурной иконки игры.
pub fn monogram_palette(name: &str) -> (Color32, Color32) {
    const PALETTES: [(Color32, Color32); 6] = [
        (Color32::from_rgb(234, 179, 8), Color32::from_rgb(18, 18, 20)),   // Жёлто-золотой
        (Color32::from_rgb(74, 222, 128), Color32::from_rgb(14, 22, 18)),  // Изумрудно-лаймовый
        (Color32::from_rgb(244, 114, 182), Color32::from_rgb(24, 14, 22)), // Неоново-розовый
        (Color32::from_rgb(45, 212, 191), Color32::from_rgb(12, 22, 22)),  // Бирюзовый
        (Color32::from_rgb(129, 140, 248), Color32::from_rgb(16, 16, 28)), // Индиго
        (Color32::from_rgb(251, 146, 60), Color32::from_rgb(24, 16, 12)),  // Янтарно-оранжевый
    ];

    let mut hash: usize = 5381;
    for b in name.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as usize);
    }
    PALETTES[hash % PALETTES.len()]
}

/// Рисует яркую квадратную процедурную иконку-монограмму, пока или если нет Discord CDN иконки.
pub fn draw_monogram_icon(ui: &mut egui::Ui, size: f32, name: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let (bg, fg) = monogram_palette(name);

    let painter = ui.painter();
    let rounding = Rounding::same(6.0);
    painter.rect_filled(rect, rounding, bg);
    painter.rect_stroke(rect, rounding, Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(0, 0, 0, 90)));

    // Декоративная диагональная полоса для визуальной глубины
    let stripe_rect = egui::Rect::from_min_max(
        egui::pos2(rect.min.x, rect.max.y - size * 0.24),
        rect.max,
    );
    painter.rect_filled(
        stripe_rect,
        Rounding { nw: 0.0, ne: 0.0, sw: 6.0, se: 6.0 },
        Color32::from_rgba_unmultiplied(0, 0, 0, 28),
    );

    let initials: String = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(2)
        .filter_map(|w| w.chars().next())
        .collect::<String>()
        .to_uppercase();

    let display = if initials.is_empty() {
        name.chars().take(2).collect::<String>().to_uppercase()
    } else {
        initials
    };

    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        display,
        egui::FontId::proportional(size * 0.40),
        fg,
    );
}

/// Рисует прямоугольный бейдж с обводкой (например, "DISCORDRPC" или "ДОСТУПЕН КВЕСТ").
pub fn draw_badge(ui: &mut egui::Ui, text: &str, bg: Color32, border: Color32, fg: Color32) {
    egui::Frame::none()
        .fill(bg)
        .stroke(Stroke::new(BTN_STROKE_W, border))
        .rounding(Rounding::same(2.0))
        .inner_margin(egui::Margin::symmetric(5.0, 2.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(9.0).strong().color(fg));
        });
}

/// Высококонтрастная белая кнопка с чёрным жирным текстом (как "ОБНОВИТЬ ВСЕ" / "ОБНОВИТЬ").
pub fn white_primary_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(
        RichText::new(label)
            .size(10.5)
            .strong()
            .color(WHITE_BTN_FG),
    )
    .fill(WHITE_BTN_BG)
    .stroke(Stroke::new(BTN_STROKE_W, WHITE_BTN_BG))
    .rounding(Rounding::same(2.0))
}

/// Строгая контурная кнопка с тонкой белой окантовкой на чёрном фоне.
pub fn outline_button(label: &str, text_color: Color32) -> egui::Button<'_> {
    egui::Button::new(
        RichText::new(label)
            .size(10.0)
            .strong()
            .color(text_color),
    )
    .fill(BG_INPUT)
    .stroke(Stroke::new(BTN_STROKE_W, BORDER_STRONG))
    .rounding(Rounding::same(2.0))
}

/// Серая кнопка остановки/отключения в монохромном стиле интерфейса.
pub fn gray_stop_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(
        RichText::new(label)
            .size(10.0)
            .strong()
            .color(Color32::WHITE),
    )
    .fill(GRAY_BTN_BG)
    .stroke(Stroke::new(BTN_STROKE_W, GRAY_BTN_BORDER))
    .rounding(Rounding::same(2.0))
}

/// Обработка ресайза безрамочного окна за края и углы (5px зона).
pub fn handle_frameless_resize(ctx: &egui::Context) {
    let is_maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    if is_maximized {
        return;
    }

    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let screen = ctx.screen_rect();
    let edge = 5.0;

    let left = pos.x <= screen.min.x + edge;
    let right = pos.x >= screen.max.x - edge;
    let top = pos.y <= screen.min.y + edge;
    let bottom = pos.y >= screen.max.y - edge;

    let dir = match (left, right, top, bottom) {
        (true, false, true, false) => Some((egui::ResizeDirection::NorthWest, egui::CursorIcon::ResizeNwSe)),
        (false, true, true, false) => Some((egui::ResizeDirection::NorthEast, egui::CursorIcon::ResizeNeSw)),
        (true, false, false, true) => Some((egui::ResizeDirection::SouthWest, egui::CursorIcon::ResizeNeSw)),
        (false, true, false, true) => Some((egui::ResizeDirection::SouthEast, egui::CursorIcon::ResizeNwSe)),
        (true, false, false, false) => Some((egui::ResizeDirection::West, egui::CursorIcon::ResizeHorizontal)),
        (false, true, false, false) => Some((egui::ResizeDirection::East, egui::CursorIcon::ResizeHorizontal)),
        (false, false, true, false) => Some((egui::ResizeDirection::North, egui::CursorIcon::ResizeVertical)),
        (false, false, false, true) => Some((egui::ResizeDirection::South, egui::CursorIcon::ResizeVertical)),
        _ => None,
    };

    if let Some((resize_dir, cursor)) = dir {
        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(resize_dir));
        }
    }
}
