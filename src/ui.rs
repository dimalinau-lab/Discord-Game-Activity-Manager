use std::collections::HashSet;
use std::path::Path;
use eframe::egui::{self, Color32, RichText, Rounding, Stroke, Vec2};
use crate::{
    anti_pause, game_matches_query, get_i18n, theme, updater::UpdateStatus,
    AppMode, GameFarmerApp, GameSortMode, GameViewMode, I18n, Language, UiScale,
};

impl GameFarmerApp {
    /// Главная точка рендеринга интерфейса в стиле ArtCraft Dark Swiss.
    pub(crate) fn render_artcraft_ui(&mut self, ctx: &egui::Context) {
        self.icon_cache.poll(ctx);
        theme::handle_frameless_resize(ctx);

        ctx.set_pixels_per_point(self.ui_scale.factor());
        let t = get_i18n(self.language);

        self.render_titlebar(ctx, &t);
        self.render_footer(ctx, &t);
        if self.show_logs_drawer {
            self.render_logs_drawer(ctx);
        }
        if self.is_running && !self.running_processes.is_empty() {
            let strip_panel = egui::Frame::none()
                .fill(theme::BG_BASE)
                .inner_margin(egui::Margin { left: 16.0, right: 16.0, top: 8.0, bottom: 4.0 });
            egui::TopBottomPanel::bottom("artcraft_active_strip")
                .frame(strip_panel)
                .show(ctx, |ui| {
                    self.render_active_strip(ui, &t);
                });
        }

        let central_frame = egui::Frame::none()
            .fill(theme::BG_BASE)
            .inner_margin(egui::Margin::symmetric(16.0, 12.0));

        egui::CentralPanel::default().frame(central_frame).show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 12.0);

            self.render_update_banner(ui, &t);

            // Полоса навигации и фильтров как в ArtCraft: [ИГРЫ | АВТОФАРМ] [↻] ... [ИЗБРАННОЕ] [СОРТИРОВКА] [Поиск]
            self.render_mode_toolbar(ui, &t);

            match self.app_mode {
                AppMode::SingleGame => self.render_single_game_tab(ui, &t),
                AppMode::RandomBatch => self.render_random_batch_tab(ui, &t),
                AppMode::Settings => self.render_settings_tab(ui, &t),
            }
        });

        // Чёткая 1px рамка по периметру безрамочного окна
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("artcraft_window_border"),
        ))
        .rect_stroke(
            ctx.screen_rect().shrink(0.5),
            Rounding::ZERO,
            Stroke::new(1.0_f32, theme::BORDER_STRONG),
        );
    }

    fn render_titlebar(&mut self, ctx: &egui::Context, t: &I18n) {
        let bar_frame = egui::Frame::none()
            .fill(theme::BG_BASE)
            .inner_margin(egui::Margin::symmetric(16.0, 0.0));

        egui::TopBottomPanel::top("artcraft_titlebar")
            .exact_height(44.0)
            .frame(bar_frame)
            .show(ctx, |ui| {
                let panel_rect = ui.max_rect();
                ui.painter().line_segment(
                    [
                        egui::pos2(panel_rect.min.x - 16.0, panel_rect.max.y),
                        egui::pos2(panel_rect.max.x + 16.0, panel_rect.max.y),
                    ],
                    Stroke::new(1.0_f32, theme::BORDER_SUBTLE),
                );

                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(12.0, 0.0);

                    if self.is_running {
                        let count = self.running_processes.len();
                        let status_text = if count == 1 {
                            t.active_single.to_string()
                        } else {
                            t.active_multi.replace("{}", &count.to_string())
                        };
                        ui.label(
                            RichText::new(format!("● {}", status_text.to_uppercase()))
                                .size(10.0)
                                .strong()
                                .color(theme::ACTIVE_GREEN_TEXT),
                        );
                    }

                    // Правая часть: Шестерёнка настроек | — □ ✕
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(0.0, 0.0);

                        if draw_window_control_btn(ui, WindowCaptionIcon::Close).clicked() {
                            if self.minimize_to_tray && !self.is_quitting {
                                self.hide_to_tray(ctx);
                            } else {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }

                        let is_max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                        let max_icon = if is_max {
                            WindowCaptionIcon::Restore
                        } else {
                            WindowCaptionIcon::Maximize
                        };
                        if draw_window_control_btn(ui, max_icon).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
                        }

                        if draw_window_control_btn(ui, WindowCaptionIcon::Minimize).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }

                        draw_v_sep(ui, 42.0);

                        let is_settings = self.app_mode == AppMode::Settings;
                        let gear_btn = egui::Button::new(
                            RichText::new("⚙")
                                .size(14.0)
                                .color(if is_settings { Color32::WHITE } else { theme::TEXT_SECONDARY }),
                        )
                        .fill(if is_settings { theme::BG_SURFACE_HOVER } else { Color32::TRANSPARENT })
                        .stroke(Stroke::NONE)
                        .rounding(Rounding::ZERO);

                        let gear_resp = ui
                            .add_sized(Vec2::new(42.0, 42.0), gear_btn)
                            .on_hover_text(t.settings_title);
                        if gear_resp.clicked() {
                            if is_settings {
                                self.app_mode = self.prev_mode;
                            } else {
                                self.prev_mode = self.app_mode;
                                self.app_mode = AppMode::Settings;
                            }
                        }

                        draw_v_sep(ui, 42.0);

                        let drag_w = ui.available_width().max(20.0);
                        let (_, drag_resp) =
                            ui.allocate_exact_size(Vec2::new(drag_w, 42.0), egui::Sense::click_and_drag());
                        if drag_resp.drag_started_by(egui::PointerButton::Primary) {
                            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        }
                        if drag_resp.double_clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
                        }
                    });
                });
            });
    }

    fn render_footer(&mut self, ctx: &egui::Context, _t: &I18n) {
        let footer_frame = egui::Frame::none()
            .fill(theme::BG_FOOTER)
            .inner_margin(egui::Margin::symmetric(14.0, 0.0));

        egui::TopBottomPanel::bottom("artcraft_footer")
            .exact_height(28.0)
            .frame(footer_frame)
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                ui.painter().line_segment(
                    [
                        egui::pos2(rect.min.x - 14.0, rect.min.y),
                        egui::pos2(rect.max.x + 14.0, rect.min.y),
                    ],
                    Stroke::new(1.0_f32, theme::BORDER_SUBTLE),
                );

                ui.horizontal_centered(|ui| {
                    let last_log = self.logs.last().map(|s| s.as_str()).unwrap_or("");
                    let footer_status = format!("{} игр  {}", self.all_games.len(), last_log);
                    let max_left_w = (ui.available_width() - 185.0).max(100.0);

                    ui.allocate_ui_with_layout(
                        Vec2::new(max_left_w, 22.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let resp = ui
                                .add(
                                    egui::Label::new(
                                        RichText::new(footer_status)
                                            .size(9.5)
                                            .monospace()
                                            .color(if self.show_logs_drawer { Color32::WHITE } else { theme::TEXT_MUTED }),
                                    )
                                    .truncate()
                                    .sense(egui::Sense::click()),
                                )
                                .on_hover_text("Показать / скрыть журнал событий");
                            if resp.clicked() {
                                self.show_logs_drawer = !self.show_logs_drawer;
                            }
                        },
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let gh_btn = egui::Button::new(
                            RichText::new("GITHUB ↗")
                                .size(9.5)
                                .monospace()
                                .color(theme::TEXT_SECONDARY),
                        )
                        .fill(Color32::TRANSPARENT)
                        .stroke(Stroke::NONE);
                        if ui.add(gh_btn).clicked() {
                            ctx.open_url(egui::OpenUrl::new_tab(
                                "https://github.com/dimalinau-lab/Discord-Game-Activity-Manager",
                            ));
                        }

                        ui.label(
                            RichText::new(format!("V{}", self.updater.current_version))
                                .size(9.5)
                                .monospace()
                                .color(theme::TEXT_MUTED),
                        );

                        let logs_label = format!(
                            "{} ({})",
                            if self.show_logs_drawer { "▼ ЛОГИ" } else { "▲ ЛОГИ" },
                            self.logs.len()
                        );
                        let logs_btn = egui::Button::new(
                            RichText::new(logs_label)
                                .size(9.5)
                                .monospace()
                                .color(theme::TEXT_SECONDARY),
                        )
                        .fill(Color32::TRANSPARENT)
                        .stroke(Stroke::NONE);
                        if ui.add(logs_btn).clicked() {
                            self.show_logs_drawer = !self.show_logs_drawer;
                        }
                    });
                });
            });
    }

    fn render_logs_drawer(&mut self, ctx: &egui::Context) {
        let drawer_frame = egui::Frame::none()
            .fill(theme::BG_INPUT)
            .inner_margin(egui::Margin::symmetric(14.0, 8.0));

        egui::TopBottomPanel::bottom("artcraft_logs_drawer")
            .exact_height(145.0)
            .frame(drawer_frame)
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                ui.painter().line_segment(
                    [
                        egui::pos2(rect.min.x - 14.0, rect.min.y),
                        egui::pos2(rect.max.x + 14.0, rect.min.y),
                    ],
                    Stroke::new(1.0_f32, theme::BORDER_STRONG),
                );

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("ЖУРНАЛ СОБЫТИЙ")
                            .size(10.0)
                            .strong()
                            .monospace()
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme::outline_button("СКРЫТЬ", theme::TEXT_SECONDARY)).clicked() {
                            self.show_logs_drawer = false;
                        }
                    });
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for line in &self.logs {
                            ui.label(
                                RichText::new(line)
                                    .size(10.5)
                                    .monospace()
                                    .color(theme::TEXT_SECONDARY),
                            );
                        }
                    });
            });
    }

    fn render_update_banner(&mut self, ui: &mut egui::Ui, t: &I18n) {
        match self.updater.status.clone() {
            UpdateStatus::Available { version, changelog, download_url } => {
                let banner = egui::Frame::none()
                    .fill(theme::BANNER_BLUE_BG)
                    .stroke(Stroke::new(1.0_f32, theme::BANNER_BLUE_BORDER))
                    .rounding(Rounding::same(2.0))
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0));

                banner.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let apply_btn = egui::Button::new(
                            RichText::new("ОБНОВИТЬ")
                                .size(10.5)
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(theme::ACCENT_BLUE)
                        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(59, 130, 246)))
                        .rounding(Rounding::same(2.0));

                        if ui.add_sized(Vec2::new(96.0, 26.0), apply_btn).clicked() {
                            self.updater.start_download_and_install(download_url, version.clone());
                            self.add_log(&format!("Загрузка обновления {} с GitHub...", version));
                        }

                        ui.add_space(6.0);
                        let msg = t.update_available_banner.replace("{}", &version);
                        ui.label(RichText::new(msg).size(12.0).strong().color(Color32::WHITE));

                        if !changelog.is_empty() {
                            let avail = (ui.available_width() - 12.0).max(0.0);
                            if avail > 60.0 {
                                ui.add_sized(
                                    Vec2::new(avail, 20.0),
                                    egui::Label::new(
                                        RichText::new(changelog).size(11.0).color(theme::TEXT_SECONDARY),
                                    )
                                    .truncate(),
                                );
                            }
                        }
                    });
                });
            }
            UpdateStatus::Downloading { version, progress, downloaded, total } => {
                let banner = egui::Frame::none()
                    .fill(theme::BANNER_BLUE_BG)
                    .stroke(Stroke::new(1.0_f32, theme::BANNER_BLUE_BORDER))
                    .rounding(Rounding::same(2.0))
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0));

                banner.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("Доступно обновление {}.", version))
                                .size(12.0)
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let mb_down = downloaded as f64 / 1_048_576.0;
                            let mb_total = total as f64 / 1_048_576.0;
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(format!("Скачивание {:.1} MB из {:.1} MB", mb_down, mb_total))
                                        .size(10.5)
                                        .color(theme::TEXT_SECONDARY),
                                );
                                ui.add_space(3.0);
                                draw_slim_progress_bar(ui, 180.0, progress, theme::BANNER_BLUE_BORDER, Color32::from_rgb(59, 130, 246));
                            });
                        });
                    });
                });
            }
            UpdateStatus::Installing => {
                let banner = egui::Frame::none()
                    .fill(theme::BANNER_BLUE_BG)
                    .stroke(Stroke::new(1.0_f32, theme::BANNER_BLUE_BORDER))
                    .rounding(Rounding::same(2.0))
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0));
                banner.show(ui, |ui| {
                    ui.label(
                        RichText::new("Запуск инсталлятора и перезапуск программы...")
                            .size(12.0)
                            .strong()
                            .color(Color32::WHITE),
                    );
                });
            }
            _ => {}
        }
    }

    fn render_active_strip(&mut self, ui: &mut egui::Ui, t: &I18n) {
        let active_frame = egui::Frame::none()
            .fill(theme::BG_SURFACE)
            .stroke(Stroke::new(1.0_f32, theme::ACTIVE_GREEN_BORDER))
            .rounding(Rounding::same(2.0))
            .inner_margin(egui::Margin::symmetric(12.0, 8.0));

        active_frame.show(ui, |ui| {
            let total_secs = (self.interval_minutes as u64) * 60;
            let elapsed_secs = self.batch_started_at.map_or(0, |t| t.elapsed().as_secs());
            let progress = if total_secs > 0 {
                (elapsed_secs as f32 / total_secs as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let rem_secs = total_secs.saturating_sub(elapsed_secs);
            let rem_m = rem_secs / 60;
            let rem_s = rem_secs % 60;
            let total_h = self.total_seconds_farmed / 3600;
            let total_m = (self.total_seconds_farmed % 3600) / 60;

            ui.horizontal(|ui| {
                let text_w = (ui.available_width() - 130.0).max(120.0);
                let text_h = if self.active_quest_status.is_some() { 54.0 } else { 36.0 };
                ui.allocate_ui_with_layout(
                    Vec2::new(text_w, text_h),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if self.running_processes.len() == 1 {
                            let proc = &self.running_processes[0];
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("{} {}", t.active_card_title, proc.game.name))
                                        .size(12.0)
                                        .strong()
                                        .color(theme::ACTIVE_GREEN_TEXT),
                                )
                                .truncate(),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!(
                                        "{}, осталось {:02}:{:02} из {} мин ({})",
                                        t.active_process_info.replace("{}", &proc.game.exe_name),
                                        rem_m,
                                        rem_s,
                                        self.interval_minutes,
                                        t.total_farmed
                                            .replacen("{}", &total_h.to_string(), 1)
                                            .replacen("{}", &total_m.to_string(), 1)
                                    ))
                                    .size(10.5)
                                    .color(theme::TEXT_SECONDARY),
                                )
                                .truncate(),
                            );
                        } else {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("Активно в Discord: {} игр", self.running_processes.len()))
                                        .size(12.0)
                                        .strong()
                                        .color(theme::ACTIVE_GREEN_TEXT),
                                )
                                .truncate(),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!(
                                        "До ротации: {:02}:{:02} из {} мин ({})",
                                        rem_m,
                                        rem_s,
                                        self.interval_minutes,
                                        t.total_farmed
                                            .replacen("{}", &total_h.to_string(), 1)
                                            .replacen("{}", &total_m.to_string(), 1)
                                    ))
                                    .size(10.5)
                                    .color(theme::TEXT_SECONDARY),
                                )
                                .truncate(),
                            );
                        }

                        if let Some(ref q_status) = self.active_quest_status {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("Квест Discord: {}", q_status))
                                        .size(10.5)
                                        .strong()
                                        .color(Color32::from_rgb(130, 225, 255)),
                                )
                                .truncate(),
                            );
                        }
                    },
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let stop_label = t.btn_stop.to_uppercase();
                    let stop_btn = theme::gray_stop_button(&stop_label);
                    if ui.add_sized(Vec2::new(118.0, 28.0), stop_btn).clicked() {
                        self.stop_farming();
                    }
                });
            });

            ui.add_space(5.0);
            draw_slim_progress_bar(
                ui,
                ui.available_width(),
                progress,
                Color32::from_rgb(20, 50, 34),
                theme::ACTIVE_GREEN_TEXT,
            );
        });
    }

    fn render_mode_toolbar(&mut self, ui: &mut egui::Ui, t: &I18n) {
        let avail_w = ui.available_width();
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 28.0;
            ui.spacing_mut().button_padding = Vec2::new(8.0, 0.0);

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);

                // Левый переключатель режимов: [ КАТАЛОГ | АВТОФАРМ ]
                let catalog_label = match self.language {
                    Language::Ru => "КАТАЛОГ",
                    Language::Uk => "КАТАЛОГ",
                    Language::En => "CATALOG",
                };
                let farm_label = match self.language {
                    Language::Ru => "АВТОФАРМ",
                    Language::Uk => "АВТОФАРМ",
                    Language::En => "AUTO FARM",
                };

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    for (idx, (mode, label)) in [
                        (AppMode::SingleGame, catalog_label),
                        (AppMode::RandomBatch, farm_label),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let is_active = self.app_mode == mode;
                        let rounding = if idx == 0 {
                            Rounding { nw: 2.0, sw: 2.0, ne: 0.0, se: 0.0 }
                        } else {
                            Rounding { nw: 0.0, sw: 0.0, ne: 2.0, se: 2.0 }
                        };
                        let btn = egui::Button::new(
                            RichText::new(label)
                                .size(10.0)
                                .strong()
                                .color(if is_active { theme::WHITE_BTN_FG } else { theme::TEXT_PRIMARY }),
                        )
                        .fill(if is_active { theme::WHITE_BTN_BG } else { theme::BG_INPUT })
                        .stroke(Stroke::new(theme::BTN_STROKE_W, theme::BORDER_STRONG))
                        .rounding(rounding);

                        if ui.add_sized(Vec2::new(76.0, 28.0), btn).clicked() {
                            self.app_mode = mode;
                        }
                    }
                });

                // Кнопка обновления [↻]
                let refresh_btn = theme::outline_button("↻", theme::TEXT_PRIMARY);
                if ui
                    .add_sized(Vec2::new(28.0, 28.0), refresh_btn)
                    .on_hover_text("Обновить иконки и сбросить кэш")
                    .clicked()
                {
                    self.icon_cache.clear_negative_cache();
                    self.detected_pc_processes = crate::get_running_process_names();
                    self.last_search_query.clear();
                }

                if self.app_mode == AppMode::SingleGame {
                    let fav_count = self.favorite_games.len();
                    let fav_text = format!("★ {}", fav_count);
                    let fav_btn = if self.filter_only_favorites {
                        theme::white_primary_button(&fav_text)
                    } else {
                        theme::outline_button(&fav_text, theme::TEXT_PRIMARY)
                    };
                    if ui
                        .add_sized(Vec2::new(48.0, 28.0), fav_btn)
                        .on_hover_text("Показать только избранные игры")
                        .clicked()
                    {
                        self.filter_only_favorites = !self.filter_only_favorites;
                        self.last_search_query.clear();
                    }

                    let dur_label = if self.interval_minutes >= 60 && self.interval_minutes % 60 == 0 {
                        format!("{} Ч", self.interval_minutes / 60)
                    } else {
                        format!("{} М", self.interval_minutes)
                    };
                    egui::ComboBox::from_id_salt("artcraft_duration_combo")
                        .selected_text(RichText::new(dur_label).size(10.5).color(theme::TEXT_PRIMARY))
                        .width(56.0)
                        .height(230.0)
                        .show_ui(ui, |ui| {
                            for (lbl, mins) in [("2 мин", 2), ("15 мин", 15), ("30 мин", 30), ("1 ч", 60), ("2 ч", 120)] {
                                ui.selectable_value(&mut self.interval_minutes, mins, lbl);
                            }
                            ui.separator();
                            render_compact_slider(ui, &mut self.interval_minutes, 1..=120, " м", 72.0);
                        });

                    let rnd_btn = theme::white_primary_button("СЛУЧАЙНАЯ");
                    if ui
                        .add_sized(Vec2::new(84.0, 28.0), rnd_btn)
                        .on_hover_text("Запустить случайную игру из каталога на выбранное время")
                        .clicked()
                        && !self.all_games.is_empty()
                    {
                        let mut rng = rand::thread_rng();
                        let idx = rand::Rng::gen_range(&mut rng, 0..self.all_games.len());
                        let g = self.all_games[idx].clone();
                        self.launch_single_game(g.name, g.exe, g.id);
                    }

                    // Правая часть: СОРТИРОВКА + ComboBox + Поиск + Вид
                    let left_used = 158.0 + 34.0 + 54.0 + 66.0 + 90.0;
                    let combo_w = 120.0;
                    let view_w = 56.0;
                    let right_fixed = combo_w + view_w + 48.0;
                    let search_w = (avail_w - left_used - right_fixed - 12.0).clamp(110.0, 220.0);
                    let spacer = (avail_w - left_used - (right_fixed + search_w)).max(0.0);
                    if spacer > 4.0 {
                        ui.add_space(spacer);
                    }

                    let sort_label = match self.sort_mode {
                        GameSortMode::NameAsc => "Имя (А–Я)",
                        GameSortMode::NameDesc => "Имя (Я–А)",
                        GameSortMode::FavoritesFirst => "Избранные сверху",
                        GameSortMode::DetectableExeFirst => "С .EXE сверху",
                    };
                    egui::ComboBox::from_id_salt("artcraft_sort_combo")
                        .selected_text(RichText::new(sort_label).size(10.5).color(theme::TEXT_PRIMARY))
                        .width(combo_w)
                        .height(220.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.sort_mode, GameSortMode::NameAsc, "Имя (А–Я)");
                            ui.selectable_value(&mut self.sort_mode, GameSortMode::NameDesc, "Имя (Я–А)");
                            ui.selectable_value(&mut self.sort_mode, GameSortMode::FavoritesFirst, "Избранные сверху");
                            ui.selectable_value(&mut self.sort_mode, GameSortMode::DetectableExeFirst, "С .EXE сверху");
                        });

                    let (search_rect, _) = ui.allocate_exact_size(Vec2::new(search_w, 28.0), egui::Sense::hover());
                    ui.painter().rect_filled(search_rect, Rounding::same(2.0), theme::BG_INPUT);
                    ui.painter().rect_stroke(search_rect, Rounding::same(2.0), Stroke::new(theme::BTN_STROKE_W, theme::BORDER_STRONG));
                    let search_inner = search_rect.shrink2(Vec2::new(8.0, 4.0));
                    let mut search_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(search_inner)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    let inner_w = if self.search_query.is_empty() {
                        search_inner.width()
                    } else {
                        (search_inner.width() - 20.0).max(40.0)
                    };
                    search_ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text(t.search_placeholder)
                            .frame(false)
                            .desired_width(inner_w),
                    );
                    if !self.search_query.is_empty() {
                        let x_btn = egui::Button::new(
                            RichText::new("✕").size(10.0).color(theme::TEXT_MUTED),
                        )
                        .fill(Color32::TRANSPARENT)
                        .stroke(Stroke::NONE);
                        if search_ui.add_sized(Vec2::new(16.0, 18.0), x_btn).clicked() {
                            self.search_query.clear();
                        }
                    }

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        if draw_view_toggle_btn(ui, true, self.view_mode == GameViewMode::Grid).clicked() {
                            self.view_mode = GameViewMode::Grid;
                        }
                        if draw_view_toggle_btn(ui, false, self.view_mode == GameViewMode::List).clicked() {
                            self.view_mode = GameViewMode::List;
                        }
                    });
                }
            });
        });
    }

    fn render_single_game_tab(&mut self, ui: &mut egui::Ui, t: &I18n) {
        // Обновление кэша фильтра и сортировки (сетка начинается сразу под тулбаром, как в ArtCraft)
        let clean_query = self.search_query.trim().to_lowercase();
        let sort_tag = match self.sort_mode {
            GameSortMode::NameAsc => 0,
            GameSortMode::NameDesc => 1,
            GameSortMode::FavoritesFirst => 2,
            GameSortMode::DetectableExeFirst => 3,
        };
        let filter_state_key = format!(
            "{}:{}:{}:{}",
            clean_query,
            self.filter_only_favorites,
            sort_tag,
            self.favorite_games.len()
        );
        let is_default_unfiltered = clean_query.is_empty()
            && !self.filter_only_favorites
            && self.sort_mode == GameSortMode::NameAsc;

        if filter_state_key != self.last_search_query {
            self.last_search_query = filter_state_key;
            if is_default_unfiltered {
                self.filtered_indices.clear();
            } else {
                let mut indices: Vec<usize> = self
                    .all_games
                    .iter()
                    .enumerate()
                    .filter(|(_, g)| {
                        if self.filter_only_favorites && !self.favorite_games.contains(&g.name) {
                            return false;
                        }
                        if clean_query.is_empty() {
                            return true;
                        }
                        game_matches_query(&g.name, &g.exe, &clean_query)
                    })
                    .map(|(i, _)| i)
                    .collect();

                match self.sort_mode {
                    GameSortMode::NameAsc => {}
                    GameSortMode::NameDesc => indices.reverse(),
                    GameSortMode::FavoritesFirst => {
                        indices.sort_by_key(|&i| !self.favorite_games.contains(&self.all_games[i].name));
                    }
                    GameSortMode::DetectableExeFirst => {
                        indices.sort_by_key(|&i| !Self::is_detectable_win_exe(&self.all_games[i].exe));
                    }
                }
                self.filtered_indices = indices;
            }
        }

        let total_items = if is_default_unfiltered {
            self.all_games.len()
        } else {
            self.filtered_indices.len()
        };

        let mut game_to_launch: Option<(String, String, String)> = None;
        let mut fav_to_toggle: Option<String> = None;
        let mut should_stop = false;
        let running_exes: HashSet<String> = self
            .running_processes
            .iter()
            .map(|rp| rp.game.exe_name.to_lowercase())
            .collect();

        if total_items == 0 {
            ui.add_space(24.0);
            let empty_text = if self.filter_only_favorites {
                t.fav_empty_msg
            } else {
                t.no_results
            };
            ui.label(
                RichText::new(empty_text)
                    .size(12.5)
                    .color(theme::TEXT_SECONDARY),
            );
        } else if self.view_mode == GameViewMode::Grid {
            let avail_w = ui.available_width();
            let cols = if avail_w >= 760.0 {
                3
            } else if avail_w >= 480.0 {
                2
            } else {
                1
            };
            let total_rows = total_items.div_ceil(cols);
            let card_h = 196.0;

            ui.scope(|ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                egui::ScrollArea::vertical()
                    .id_salt("games_grid_scroll")
                    .auto_shrink([false, false])
                    .show_rows(ui, card_h, total_rows, |ui, row_range| {
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        let full_w = (ui.available_width() - 2.0).max(200.0);
                        let cell_w = (full_w / cols as f32).floor();
                        let mut highlight_borders: Vec<(egui::Rect, Color32)> = Vec::new();

                        for grid_row in row_range {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing = Vec2::ZERO;
                                ui.add_space(1.0);
                                for col in 0..cols {
                                    let item_pos = grid_row * cols + col;
                                    if item_pos >= total_items {
                                        break;
                                    }
                                    let db_idx = if is_default_unfiltered {
                                        item_pos
                                    } else {
                                        self.filtered_indices[item_pos]
                                    };
                                    let game = self.all_games[db_idx].clone();
                                    let is_fav = self.favorite_games.contains(&game.name);
                                    let is_active = if !game.exe.is_empty() {
                                        running_exes.contains(&game.exe.to_lowercase())
                                    } else {
                                        self.running_processes.iter().any(|rp| rp.game.name == game.name)
                                    };

                                    self.icon_cache.ensure_requested(&game.id);

                                    render_game_grid_card(
                                        ui,
                                        cell_w,
                                        card_h,
                                        &game.name,
                                        &game.exe,
                                        &game.id,
                                        is_fav,
                                        is_active,
                                        self.interval_minutes,
                                        self.icon_cache.get_texture(&game.id),
                                        self.icon_cache.get_description(&game.id),
                                        &mut game_to_launch,
                                        &mut fav_to_toggle,
                                        &mut should_stop,
                                        &mut highlight_borders,
                                    );
                                }
                            });
                        }

                        // Рисуем рамки активных/наведённых карточек поверх соседних границ
                        let painter = ui.painter();
                        for (rect, color) in highlight_borders {
                            painter.rect_stroke(rect, Rounding::ZERO, Stroke::new(1.0_f32, color));
                        }
                    });
            });
        } else {
            let row_h = 44.0;
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                egui::ScrollArea::vertical()
                    .id_salt("games_list_scroll")
                    .auto_shrink([false, false])
                    .show_rows(ui, row_h, total_items, |ui, row_range| {
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        let full_w = ui.available_width() - 2.0;

                        for item_pos in row_range {
                            let db_idx = if is_default_unfiltered {
                                item_pos
                            } else {
                                self.filtered_indices[item_pos]
                            };
                            let game = self.all_games[db_idx].clone();
                            let is_fav = self.favorite_games.contains(&game.name);
                            let is_active = if !game.exe.is_empty() {
                                running_exes.contains(&game.exe.to_lowercase())
                            } else {
                                self.running_processes.iter().any(|rp| rp.game.name == game.name)
                            };

                            self.icon_cache.ensure_requested(&game.id);

                            render_game_list_row(
                                ui,
                                full_w,
                                row_h,
                                item_pos,
                                &game.name,
                                &game.exe,
                                &game.id,
                                is_fav,
                                is_active,
                                self.interval_minutes,
                                self.icon_cache.get_texture(&game.id),
                                &mut game_to_launch,
                                &mut fav_to_toggle,
                                &mut should_stop,
                            );
                        }
                    });
            });
        }

        if let Some(game_name) = fav_to_toggle {
            if self.favorite_games.contains(&game_name) {
                self.favorite_games.remove(&game_name);
                self.add_log(&format!("Удалено из избранного: '{}'", game_name));
            } else {
                self.favorite_games.insert(game_name.clone());
                self.add_log(&format!("Добавлено в избранное: '{}'", game_name));
            }
            self.last_search_query.clear();
        }
        if should_stop {
            self.stop_farming();
        }
        if let Some((name, exe, id)) = game_to_launch {
            self.launch_single_game(name, exe, id);
        }
    }

    fn render_random_batch_tab(&mut self, ui: &mut egui::Ui, t: &I18n) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let panel_frame = egui::Frame::none()
                    .fill(theme::BG_SURFACE)
                    .stroke(Stroke::new(1.0_f32, theme::BORDER_SUBTLE))
                    .rounding(Rounding::same(2.0))
                    .inner_margin(egui::Margin::symmetric(18.0, 16.0));

                panel_frame.show(ui, |ui| {
                    ui.label(
                        RichText::new(t.random_title.to_uppercase())
                            .size(13.0)
                            .strong()
                            .color(Color32::WHITE),
                    );
                    ui.add_space(3.0);
                    ui.label(
                        RichText::new(t.random_desc.replace("{}", &self.interval_minutes.to_string()))
                            .size(11.5)
                            .color(theme::TEXT_SECONDARY),
                    );

                    draw_h_divider(ui);

                    // 1. Пул игр
                    let sec1 = match self.language {
                        Language::Ru => "ПУЛ ИГР ДЛЯ РОТАЦИИ",
                        Language::Uk => "ПУЛ ІГОР ДЛЯ РОТАЦІЇ",
                        Language::En => "ROTATION GAME POOL",
                    };
                    ui.label(RichText::new(sec1).size(10.0).strong().color(theme::TEXT_MUTED));
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        let pool_w = ((ui.available_width() - 8.0) / 2.0).clamp(140.0, 210.0);
                        let all_selected = !self.random_only_favorites;
                        let all_label = t.fav_mode_all.to_uppercase();
                        let all_btn = if all_selected {
                            theme::white_primary_button(&all_label)
                        } else {
                            theme::outline_button(&all_label, theme::TEXT_SECONDARY)
                        };
                        if ui.add_sized(Vec2::new(pool_w, 30.0), all_btn).clicked() {
                            self.random_only_favorites = false;
                        }

                        let fav_count = self.favorite_games.len();
                        let fav_label = format!("★ {} ({})", t.fav_mode_only.to_uppercase(), fav_count);
                        let fav_btn = if self.random_only_favorites {
                            theme::white_primary_button(&fav_label)
                        } else {
                            theme::outline_button(&fav_label, theme::TEXT_SECONDARY)
                        };
                        if ui.add_sized(Vec2::new(pool_w, 30.0), fav_btn).clicked() {
                            if fav_count == 0 {
                                self.add_log(t.fav_empty_msg);
                            } else {
                                self.random_only_favorites = true;
                            }
                        }
                    });

                    draw_h_divider(ui);

                    // 2. Интервал ротации
                    let sec2 = match self.language {
                        Language::Ru => "ИНТЕРВАЛ РОТАЦИИ",
                        Language::Uk => "ІНТЕРВАЛ РОТАЦІЇ",
                        Language::En => "ROTATION INTERVAL",
                    };
                    ui.label(RichText::new(sec2).size(10.0).strong().color(theme::TEXT_MUTED));
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for (lbl, val) in [("5M", 5), ("10M", 10), ("15M", 15), ("20M", 20)] {
                            let is_sel = self.interval_minutes == val;
                            let btn = if is_sel {
                                theme::white_primary_button(lbl)
                            } else {
                                theme::outline_button(lbl, theme::TEXT_SECONDARY)
                            };
                            if ui.add_sized(Vec2::new(48.0, 26.0), btn).clicked() {
                                self.interval_minutes = val;
                            }
                        }
                        render_compact_slider(ui, &mut self.interval_minutes, 1..=20, " мин", 64.0);
                    });

                    if self.interval_minutes < 6 {
                        ui.add_space(6.0);
                        let warn_msg = match self.language {
                            Language::Ru => "Интервал Discord heartbeat: 5 мин. Рекомендуется 10–15 мин.",
                            Language::Uk => "Інтервал Discord heartbeat: 5 хв. Рекомендується 10–15 хв.",
                            Language::En => "Discord heartbeat interval is 5 min. Recommended: 10–15 min.",
                        };
                        theme::draw_badge(
                            ui,
                            warn_msg,
                            theme::BADGE_AMBER_BG,
                            theme::BADGE_AMBER_BORDER,
                            theme::BADGE_AMBER_TEXT,
                        );
                    }

                    draw_h_divider(ui);

                    // 3. Количество игр
                    let sec3 = match self.language {
                        Language::Ru => "КОЛИЧЕСТВО ОДНОВРЕМЕННЫХ ИГР",
                        Language::Uk => "КІЛЬКІСТЬ ОДНОЧАСНИХ ІГОР",
                        Language::En => "SIMULTANEOUS GAMES",
                    };
                    ui.label(RichText::new(sec3).size(10.0).strong().color(theme::TEXT_MUTED));
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for (lbl, val) in [("4", 4), ("6", 6), ("8", 8), ("12", 12), ("16", 16)] {
                            let is_sel = self.batch_size == val;
                            let btn = if is_sel {
                                theme::white_primary_button(lbl)
                            } else {
                                theme::outline_button(lbl, theme::TEXT_SECONDARY)
                            };
                            if ui.add_sized(Vec2::new(44.0, 26.0), btn).clicked() {
                                self.batch_size = val;
                            }
                        }
                        render_compact_slider(ui, &mut self.batch_size, 1..=16, "", 64.0);
                    });

                    draw_h_divider(ui);

                    // 4. Кнопки запуска и прогресс
                    ui.horizontal_wrapped(|ui| {
                        let is_batch_active = self.is_running && self.app_mode == AppMode::RandomBatch;
                        let start_title = if is_batch_active {
                            t.btn_change_random.to_uppercase()
                        } else {
                            t.btn_start_random.to_uppercase()
                        };
                        let start_btn = theme::white_primary_button(&start_title);
                        let start_w = ui.available_width().clamp(160.0, 250.0);
                        if ui.add_sized(Vec2::new(start_w, 34.0), start_btn).clicked() {
                            self.launch_random_batch();
                        }

                        if is_batch_active {
                            let stop_label = t.btn_stop.to_uppercase();
                            let stop_btn = theme::gray_stop_button(&stop_label);
                            if ui.add_sized(Vec2::new(115.0, 34.0), stop_btn).clicked() {
                                self.stop_farming();
                            }
                        }

                        if !self.farmed_history_names.is_empty() {
                            if ui
                                .add_sized(Vec2::new(82.0, 34.0), theme::outline_button(&t.btn_reset_farmed.to_uppercase(), theme::TEXT_SECONDARY))
                                .clicked()
                            {
                                self.farmed_history_names.clear();
                                self.add_log("История прокачанных игр сброшена.");
                            }
                        }

                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(t.farmed_count.replace("{}", &self.farmed_history_names.len().to_string()))
                                .size(11.5)
                                .strong()
                                .color(theme::ACTIVE_GREEN_TEXT),
                        );
                    });

                    // 5. Активная сетка процессов
                    if self.is_running && !self.running_processes.is_empty() {
                        draw_h_divider(ui);

                        let avail_w = ui.available_width();
                        let cols = if avail_w >= 720.0 { 3 } else if avail_w >= 460.0 { 2 } else { 1 };
                        let gap = 8.0_f32;
                        let cell_w = ((avail_w - gap * (cols - 1) as f32) / cols as f32)
                            .floor()
                            .max(180.0);
                        let card_h = 56.0_f32;
                        let replace_label = t.btn_replace_game.to_uppercase();
                        let mut game_to_replace: Option<usize> = None;

                        ui.scope(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::splat(gap);
                            for row in (0..self.running_processes.len()).step_by(cols) {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing = Vec2::new(gap, 0.0);
                                    for c in 0..cols {
                                        let idx = row + c;
                                        if idx >= self.running_processes.len() {
                                            break;
                                        }
                                        let rp_name = self.running_processes[idx].game.name.clone();
                                        let rp_id = self.running_processes[idx].game.app_id.clone();
                                        let is_ipc_live = idx == self.current_ipc_index;

                                        self.icon_cache.ensure_requested(&rp_id);
                                        let icon_tex = self.icon_cache.get_texture(&rp_id);

                                        if render_active_batch_card(
                                            ui,
                                            cell_w,
                                            card_h,
                                            &rp_name,
                                            is_ipc_live,
                                            t.broadcasting_badge,
                                            t.running_badge,
                                            &replace_label,
                                            icon_tex,
                                        ) {
                                            game_to_replace = Some(idx);
                                        }
                                    }
                                });
                            }
                        });

                        if let Some(replace_idx) = game_to_replace {
                            self.replace_batch_game(replace_idx);
                        }
                    }
                });
            });
    }

    fn render_settings_tab(&mut self, ui: &mut egui::Ui, t: &I18n) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let panel_frame = egui::Frame::none()
                    .fill(theme::BG_SURFACE)
                    .stroke(Stroke::new(1.0_f32, theme::BORDER_SUBTLE))
                    .rounding(Rounding::same(2.0))
                    .inner_margin(egui::Margin::symmetric(18.0, 16.0));

                panel_frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(t.settings_title.to_uppercase())
                                .size(13.0)
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(theme::outline_button("← НАЗАД", theme::TEXT_PRIMARY)).clicked() {
                                self.app_mode = self.prev_mode;
                            }
                        });
                    });

                    draw_h_divider(ui);

                    // 1. Язык
                    ui.label(RichText::new(t.settings_lang.to_uppercase()).size(10.0).strong().color(theme::TEXT_MUTED));
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        for (lang, label) in [
                            (Language::Ru, "RU · РУССКИЙ"),
                            (Language::Uk, "UA · УКРАЇНСЬКА"),
                            (Language::En, "EN · ENGLISH"),
                        ] {
                            let is_cur = self.language == lang;
                            let btn = if is_cur {
                                theme::white_primary_button(label)
                            } else {
                                theme::outline_button(label, theme::TEXT_SECONDARY)
                            };
                            if ui.add_sized(Vec2::new(132.0, 28.0), btn).clicked() {
                                self.language = lang;
                            }
                        }
                    });

                    draw_h_divider(ui);

                    // 2. Масштаб интерфейса
                    ui.label(RichText::new(t.settings_scale.to_uppercase()).size(10.0).strong().color(theme::TEXT_MUTED));
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        for (scale, label) in [
                            (UiScale::Normal, t.scale_100),
                            (UiScale::Medium, t.scale_115),
                            (UiScale::Large, t.scale_130),
                            (UiScale::Huge, t.scale_145),
                        ] {
                            let is_cur = self.ui_scale == scale;
                            let btn = if is_cur {
                                theme::white_primary_button(label)
                            } else {
                                theme::outline_button(label, theme::TEXT_SECONDARY)
                            };
                            if ui.add_sized(Vec2::new(126.0, 28.0), btn).clicked() {
                                self.ui_scale = scale;
                            }
                        }
                    });

                    draw_h_divider(ui);

                    // 3. Безопасность и поведение
                    ui.label(RichText::new(t.settings_security.to_uppercase()).size(10.0).strong().color(theme::TEXT_MUTED));
                    ui.add_space(6.0);
                    ui.checkbox(
                        &mut self.exclude_running_pc_games,
                        RichText::new(t.protect_pc).size(11.5).color(theme::TEXT_PRIMARY),
                    );
                    ui.add_space(4.0);
                    ui.checkbox(
                        &mut self.minimize_to_tray,
                        RichText::new(t.minimize_to_tray).size(11.5).color(theme::TEXT_PRIMARY),
                    );

                    draw_h_divider(ui);

                    // 4. Анти-пауза видео Discord
                    ui.label(RichText::new("АНТИ-ПАУЗА ВИДЕО DISCORD").size(10.0).strong().color(theme::TEXT_MUTED));
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if self.anti_pause_active {
                            let off_btn = theme::gray_stop_button("ОТКЛЮЧИТЬ АНТИ-ПАУЗУ");
                            if ui.add_sized(Vec2::new(220.0, 28.0), off_btn).clicked() {
                                match anti_pause::uninstall_anti_pause() {
                                    Ok(_) => {
                                        self.anti_pause_active = false;
                                        self.add_log("Анти-пауза видео отключена. Перезапустите Discord.");
                                    }
                                    Err(e) => self.add_log(&format!("Ошибка отключения анти-паузы: {}", e)),
                                }
                            }
                        } else if ui
                            .add_sized(Vec2::new(220.0, 28.0), theme::white_primary_button("ВКЛЮЧИТЬ АНТИ-ПАУЗУ"))
                            .clicked()
                        {
                            match anti_pause::install_anti_pause() {
                                Ok(_) => {
                                    self.anti_pause_active = true;
                                    self.add_log("Анти-пауза видео включена. Перезапустите Discord.");
                                }
                                Err(e) => self.add_log(&format!("Ошибка включения анти-паузы: {}", e)),
                            }
                        }

                        if ui
                            .add_sized(Vec2::new(175.0, 28.0), theme::outline_button("ПЕРЕЗАПУСТИТЬ DISCORD", theme::TEXT_PRIMARY))
                            .on_hover_text("Перезапустить процессы Discord для применения настроек")
                            .clicked()
                        {
                            match anti_pause::restart_discord() {
                                Ok(_) => self.add_log("Discord перезапущен."),
                                Err(e) => self.add_log(&format!("Ошибка перезапуска Discord: {}", e)),
                            }
                        }

                        if self.anti_pause_active {
                            theme::draw_badge(ui, "АКТИВНО", theme::BG_INPUT, theme::ACTIVE_GREEN_BORDER, theme::ACTIVE_GREEN_TEXT);
                        } else {
                            theme::draw_badge(ui, "ОТКЛЮЧЕНО", theme::BG_INPUT, theme::BORDER_STRONG, theme::TEXT_MUTED);
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Видео в квестах не ставится на паузу при сворачивании Discord. Для применения перезапустите Discord.")
                            .size(10.5)
                            .color(theme::TEXT_SECONDARY),
                    );

                    draw_h_divider(ui);

                    // 5. Обновления
                    ui.label(RichText::new(t.settings_updates.to_uppercase()).size(10.0).strong().color(theme::TEXT_MUTED));
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_sized(Vec2::new(165.0, 28.0), theme::outline_button(&t.btn_check_updates.to_uppercase(), theme::TEXT_PRIMARY))
                            .clicked()
                        {
                            self.updater.check_github_update();
                            self.add_log("Запрос проверки обновлений на GitHub...");
                        }

                        match &self.updater.status {
                            UpdateStatus::Checking => {
                                ui.label(RichText::new("Проверка GitHub...").size(11.0).color(theme::BADGE_AMBER_TEXT));
                            }
                            UpdateStatus::UpToDate => {
                                ui.label(RichText::new(t.status_up_to_date).size(11.0).strong().color(theme::ACTIVE_GREEN_TEXT));
                            }
                            UpdateStatus::Available { version, .. } => {
                                ui.label(RichText::new(format!("Доступна {}", version)).size(11.0).strong().color(Color32::from_rgb(90, 195, 255)));
                            }
                            UpdateStatus::Error(err) => {
                                ui.label(RichText::new(format!("Ошибка: {}", err)).size(11.0).color(theme::STOP_RED_BORDER));
                            }
                            _ => {}
                        }
                    });
                });
            });
    }
}

#[allow(clippy::too_many_arguments)]
fn render_game_grid_card(
    ui: &mut egui::Ui,
    cell_w: f32,
    card_h: f32,
    name: &str,
    exe: &str,
    app_id: &str,
    is_fav: bool,
    is_active: bool,
    interval_minutes: u32,
    icon_tex: Option<&egui::TextureHandle>,
    rpc_desc: Option<&str>,
    game_to_launch: &mut Option<(String, String, String)>,
    fav_to_toggle: &mut Option<String>,
    should_stop: &mut bool,
    highlight_borders: &mut Vec<(egui::Rect, Color32)>,
) {
    let (card_rect, card_resp) = ui.allocate_exact_size(Vec2::new(cell_w, card_h), egui::Sense::hover());
    let hovered = card_resp.hovered();

    let bg = if hovered {
        theme::BG_SURFACE_HOVER
    } else {
        theme::BG_SURFACE
    };
    let highlight_border = if is_active {
        Some(theme::ACTIVE_GREEN_BORDER)
    } else if hovered {
        Some(Color32::from_rgb(100, 175, 95))
    } else {
        None
    };

    let painter = ui.painter();
    let draw_rect = card_rect.shrink(0.5);
    painter.rect_filled(draw_rect, Rounding::ZERO, bg);
    painter.rect_stroke(draw_rect, Rounding::ZERO, Stroke::new(1.0_f32, theme::BORDER_SUBTLE));
    if let Some(color) = highlight_border {
        highlight_borders.push((draw_rect, color));
    }

    // Верхняя полоса карточки: [цветной .EXE / статус]
    let header_h = 30.0;
    let header_bottom_y = draw_rect.min.y + header_h;
    painter.line_segment(
        [egui::pos2(draw_rect.min.x, header_bottom_y), egui::pos2(draw_rect.max.x, header_bottom_y)],
        Stroke::new(1.0_f32, theme::BORDER_SUBTLE),
    );

    let parent_clip = ui.clip_rect();
    let header_rect = egui::Rect::from_min_max(
        egui::pos2(draw_rect.min.x + 16.0, draw_rect.min.y),
        egui::pos2(draw_rect.max.x - 16.0, header_bottom_y),
    );
    let mut header_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(header_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    header_ui.set_clip_rect(parent_clip.intersect(header_rect.expand(2.0)));
    header_ui.horizontal_centered(|ui| {
        let exe_short = Path::new(exe)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or(exe)
            .to_uppercase();
        let top_left = if exe_short.is_empty() {
            "ТОЛЬКО IPC".to_string()
        } else {
            exe_short
        };
        let top_color = if is_active {
            theme::ACTIVE_GREEN_TEXT
        } else if exe.is_empty() {
            theme::BADGE_AMBER_TEXT
        } else {
            Color32::from_rgb(56, 189, 248) // Циановый тег как V0.18.1 в ArtCraft
        };
        let left_w = header_rect.width().max(50.0);
        ui.allocate_ui_with_layout(
            Vec2::new(left_w, 20.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.add(
                    egui::Label::new(RichText::new(top_left).size(10.0).monospace().color(top_color))
                        .truncate(),
                );
            },
        );
    });

    // Основное тело карточки с просторными отступами как в ArtCraft
    let body_rect = egui::Rect::from_min_max(
        egui::pos2(draw_rect.min.x + 18.0, header_bottom_y + 14.0),
        egui::pos2(draw_rect.max.x - 18.0, draw_rect.max.y - 14.0),
    );
    let actions_h = 28.0;
    let actions_rect = egui::Rect::from_min_max(
        egui::pos2(body_rect.min.x, body_rect.max.y - actions_h),
        egui::pos2(body_rect.max.x, body_rect.max.y),
    );
    let content_rect = egui::Rect::from_min_max(
        body_rect.min,
        egui::pos2(body_rect.max.x, (actions_rect.min.y - 8.0).max(body_rect.min.y)),
    );

    let mut body_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
    );
    body_ui.set_clip_rect(parent_clip.intersect(content_rect.expand(2.0)));
    body_ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);

    body_ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);
        // 1. Шапка: 46x46 иконка + крупный заголовок 15.5px + подзаголовок
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(12.0, 0.0);
            if let Some(tex) = icon_tex {
                ui.add(
                    egui::Image::new(tex)
                        .fit_to_exact_size(Vec2::splat(46.0))
                        .rounding(Rounding::same(8.0)),
                );
            } else {
                theme::draw_monogram_icon(ui, 46.0, name);
            }

            let info_w = (content_rect.width() - 58.0).max(50.0);
            ui.allocate_ui_with_layout(
                Vec2::new(info_w, 46.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(name)
                                .size(15.5)
                                .strong()
                                .color(if is_active { theme::ACTIVE_GREEN_TEXT } else { Color32::WHITE }),
                        )
                        .truncate(),
                    );

                    if is_active {
                        theme::draw_badge(ui, "АКТИВНО", theme::BG_INPUT, theme::ACTIVE_GREEN_BORDER, theme::ACTIVE_GREEN_TEXT);
                    } else {
                        let sub = if exe.is_empty() { "Discord IPC" } else { exe };
                        ui.add(
                            egui::Label::new(RichText::new(sub).size(11.0).color(theme::TEXT_SECONDARY))
                                .truncate(),
                        );
                    }
                },
            );
        });

        // 2. Описание (2 строки как в ArtCraft)
        if let Some(desc) = rpc_desc {
            ui.add_space(12.0);
            ui.add(
                egui::Label::new(RichText::new(desc).size(11.5).color(theme::TEXT_SECONDARY))
                    .wrap(),
            );
        }
    });

    // 3. Нижняя строка действий, прижатая к низу карточки и растянутая на всю ширину тела
    let mut actions_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(actions_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    actions_ui.set_clip_rect(parent_clip.intersect(actions_rect.expand(4.0)));
    actions_ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);

    let star_btn_w = 26.0;
    let btn_gap = 6.0;
    let main_btn_w = (actions_rect.width() - star_btn_w - btn_gap - 1.0).max(80.0);

    if is_active {
        let stop_btn = theme::gray_stop_button("■ ОСТАНОВИТЬ");
        if actions_ui.add_sized(Vec2::new(main_btn_w, 26.0), stop_btn).clicked() {
            *should_stop = true;
        }
    } else {
        let launch_label = if interval_minutes >= 60 && interval_minutes % 60 == 0 {
            format!("ЗАПУСТИТЬ ({} Ч)", interval_minutes / 60)
        } else {
            format!("ЗАПУСТИТЬ ({} М)", interval_minutes)
        };
        let launch_btn = if is_fav {
            theme::white_primary_button(&launch_label)
        } else {
            theme::outline_button(&launch_label, theme::TEXT_PRIMARY)
        };
        if actions_ui.add_sized(Vec2::new(main_btn_w, 26.0), launch_btn).clicked() {
            *game_to_launch = Some((name.to_string(), exe.to_string(), app_id.to_string()));
        }
    }

    let star_btn = theme::outline_button(
        if is_fav { "★" } else { "☆" },
        if is_fav { theme::BADGE_AMBER_TEXT } else { theme::TEXT_PRIMARY },
    );
    if actions_ui.add_sized(Vec2::new(star_btn_w, 26.0), star_btn).clicked() {
        *fav_to_toggle = Some(name.to_string());
    }
}

#[allow(clippy::too_many_arguments)]
fn render_game_list_row(
    ui: &mut egui::Ui,
    full_w: f32,
    row_h: f32,
    row_idx: usize,
    name: &str,
    exe: &str,
    app_id: &str,
    is_fav: bool,
    is_active: bool,
    interval_minutes: u32,
    icon_tex: Option<&egui::TextureHandle>,
    game_to_launch: &mut Option<(String, String, String)>,
    fav_to_toggle: &mut Option<String>,
    should_stop: &mut bool,
) {
    let row_frame = egui::Frame::none()
        .fill(theme::BG_SURFACE)
        .stroke(Stroke::new(1.0_f32, if is_active { theme::ACTIVE_GREEN_BORDER } else { theme::BORDER_SUBTLE }))
        .rounding(Rounding::ZERO)
        .inner_margin(egui::Margin::symmetric(12.0, 6.0));

    row_frame.show(ui, |ui| {
        ui.set_width(full_w - 24.0);
        ui.set_height(row_h - 14.0);
        ui.spacing_mut().item_spacing = Vec2::new(10.0, 0.0);

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let btn_w = 132.0;
            if is_active {
                let stop_btn = theme::gray_stop_button("■ ОСТАНОВИТЬ");
                if ui.add_sized(Vec2::new(btn_w, 26.0), stop_btn).clicked() {
                    *should_stop = true;
                }
            } else {
                let label = if interval_minutes >= 60 && interval_minutes % 60 == 0 {
                    format!("ЗАПУСТИТЬ ({}Ч)", interval_minutes / 60)
                } else {
                    format!("ЗАПУСТИТЬ ({}М)", interval_minutes)
                };
                let btn = if is_fav {
                    theme::white_primary_button(&label)
                } else {
                    theme::outline_button(&label, theme::TEXT_PRIMARY)
                };
                if ui.add_sized(Vec2::new(btn_w, 26.0), btn).clicked() {
                    *game_to_launch = Some((name.to_string(), exe.to_string(), app_id.to_string()));
                }
            }

            let exe_short = Path::new(exe)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(exe)
                .to_uppercase();
            ui.label(
                RichText::new(if exe_short.is_empty() { "IPC" } else { &exe_short })
                    .size(9.5)
                    .monospace()
                    .color(theme::TEXT_MUTED),
            );

            let left_w = ui.available_width().max(60.0);
            ui.allocate_ui_with_layout(
                Vec2::new(left_w, 28.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(10.0, 0.0);
                    ui.scope(|ui| {
                        ui.spacing_mut().button_padding = Vec2::ZERO;
                        let star_btn = theme::outline_button(
                            if is_fav { "★" } else { "☆" },
                            if is_fav { theme::BADGE_AMBER_TEXT } else { theme::TEXT_SECONDARY },
                        );
                        if ui.add_sized(Vec2::new(26.0, 26.0), star_btn).clicked() {
                            *fav_to_toggle = Some(name.to_string());
                        }
                    });

                    if let Some(tex) = icon_tex {
                        ui.add(
                            egui::Image::new(tex)
                                .fit_to_exact_size(Vec2::splat(26.0))
                                .rounding(Rounding::same(4.0)),
                        );
                    } else {
                        theme::draw_monogram_icon(ui, 26.0, name);
                    }

                    ui.label(
                        RichText::new(format!("#{}", row_idx + 1))
                            .size(10.0)
                            .monospace()
                            .color(theme::TEXT_MUTED),
                    );
                    ui.add(
                        egui::Label::new(
                            RichText::new(name)
                                .size(12.0)
                                .strong()
                                .color(if is_active { theme::ACTIVE_GREEN_TEXT } else { Color32::WHITE }),
                        )
                        .truncate(),
                    );
                },
            );
        });
    });
}

fn draw_v_sep(ui: &mut egui::Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, height), egui::Sense::hover());
    ui.painter().line_segment(
        [
            egui::pos2(rect.center().x, rect.min.y + 4.0),
            egui::pos2(rect.center().x, rect.max.y - 4.0),
        ],
        Stroke::new(1.0_f32, theme::BORDER_SUBTLE),
    );
}

fn draw_h_divider(ui: &mut egui::Ui) {
    ui.add_space(10.0);
    let avail = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(avail, 1.0), egui::Sense::hover());
    ui.painter().line_segment(
        [rect.left_center(), rect.right_center()],
        Stroke::new(1.0_f32, theme::BORDER_SUBTLE),
    );
    ui.add_space(10.0);
}

fn draw_slim_progress_bar(ui: &mut egui::Ui, width: f32, progress: f32, track: Color32, fill: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width.max(20.0), 3.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, Rounding::same(1.0), track);
    let fill_w = rect.width() * progress.clamp(0.0, 1.0);
    if fill_w > 0.0 {
        let fill_rect = egui::Rect::from_min_size(rect.min, Vec2::new(fill_w, rect.height()));
        p.rect_filled(fill_rect, Rounding::same(1.0), fill);
    }
}

/// Кнопка переключателя вида (сетка / список) с векторной иконкой единого размера 10x10 px.
fn draw_view_toggle_btn(ui: &mut egui::Ui, is_grid: bool, active: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(28.0, 28.0), egui::Sense::click());
    let rounding = if is_grid {
        Rounding { nw: 2.0, sw: 2.0, ne: 0.0, se: 0.0 }
    } else {
        Rounding { nw: 0.0, sw: 0.0, ne: 2.0, se: 2.0 }
    };
    let bg = if active {
        theme::WHITE_BTN_BG
    } else if resp.hovered() {
        theme::BG_SURFACE_HOVER
    } else {
        theme::BG_INPUT
    };
    let border = if resp.hovered() && !active {
        theme::BORDER_HOVER
    } else {
        theme::BORDER_STRONG
    };
    let fg = if active { theme::WHITE_BTN_FG } else { theme::TEXT_PRIMARY };

    let p = ui.painter();
    let draw_rect = rect.shrink(0.5);
    p.rect_filled(draw_rect, rounding, bg);
    p.rect_stroke(draw_rect, rounding, Stroke::new(theme::BTN_STROKE_W, border));

    let c = rect.center();
    if is_grid {
        for dx in [-5.0_f32, 1.0_f32] {
            for dy in [-5.0_f32, 1.0_f32] {
                let cell = egui::Rect::from_min_size(
                    egui::pos2(c.x + dx, c.y + dy),
                    Vec2::splat(4.0),
                );
                p.rect_stroke(cell, Rounding::ZERO, Stroke::new(1.1_f32, fg));
            }
        }
    } else {
        let stroke = Stroke::new(1.3_f32, fg);
        for dy in [-3.5_f32, 0.0_f32, 3.5_f32] {
            p.line_segment(
                [egui::pos2(c.x - 5.0, c.y + dy), egui::pos2(c.x + 5.0, c.y + dy)],
                stroke,
            );
        }
    }
    resp
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WindowCaptionIcon {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Векторная кнопка управления окном с единым размером иконки (9x9 px) и центрированием.
fn draw_window_control_btn(ui: &mut egui::Ui, kind: WindowCaptionIcon) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(40.0, 42.0), egui::Sense::click());
    let hovered = resp.hovered();
    let bg = if hovered {
        if kind == WindowCaptionIcon::Close {
            Color32::from_rgb(196, 43, 28)
        } else {
            Color32::from_rgb(24, 26, 32)
        }
    } else {
        theme::BG_BASE
    };
    let fg = if hovered { Color32::WHITE } else { theme::TEXT_SECONDARY };
    let p = ui.painter();
    if hovered {
        p.rect_filled(rect, Rounding::ZERO, bg);
    }

    let c = rect.center();
    let half = 4.5_f32;
    let stroke = Stroke::new(1.1_f32, fg);
    match kind {
        WindowCaptionIcon::Minimize => {
            p.line_segment(
                [egui::pos2(c.x - half, c.y), egui::pos2(c.x + half, c.y)],
                stroke,
            );
        }
        WindowCaptionIcon::Maximize => {
            p.rect_stroke(
                egui::Rect::from_center_size(c, Vec2::splat(half * 2.0)),
                Rounding::ZERO,
                stroke,
            );
        }
        WindowCaptionIcon::Restore => {
            let back = egui::Rect::from_center_size(
                egui::pos2(c.x + 1.2, c.y - 1.2),
                Vec2::splat(7.0),
            );
            let front = egui::Rect::from_center_size(
                egui::pos2(c.x - 1.2, c.y + 1.2),
                Vec2::splat(7.0),
            );
            p.rect_stroke(back, Rounding::ZERO, stroke);
            p.rect_filled(front, Rounding::ZERO, bg);
            p.rect_stroke(front, Rounding::ZERO, stroke);
        }
        WindowCaptionIcon::Close => {
            p.line_segment(
                [egui::pos2(c.x - half, c.y - half), egui::pos2(c.x + half, c.y + half)],
                stroke,
            );
            p.line_segment(
                [egui::pos2(c.x - half, c.y + half), egui::pos2(c.x + half, c.y - half)],
                stroke,
            );
        }
    }
    resp
}

/// Компактный слайдер с видимой дорожкой, располагающийся вплотную к пресетам.
fn render_compact_slider<Num: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut Num,
    range: std::ops::RangeInclusive<Num>,
    suffix: &str,
    slider_width: f32,
) {
    ui.scope(|ui| {
        ui.spacing_mut().slider_width = slider_width;
        let visuals = ui.visuals_mut();
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(58, 62, 74);
        visuals.selection.bg_fill = Color32::from_rgb(205, 210, 220);
        let mut slider = egui::Slider::new(value, range).trailing_fill(true);
        if !suffix.is_empty() {
            slider = slider.suffix(suffix);
        }
        ui.add(slider);
    });
}

/// Компактная карточка активной игры фиксированного размера для сетки автофарма.
#[allow(clippy::too_many_arguments)]
fn render_active_batch_card(
    ui: &mut egui::Ui,
    cell_w: f32,
    card_h: f32,
    name: &str,
    is_ipc_live: bool,
    broadcasting_badge: &str,
    running_badge: &str,
    replace_label: &str,
    icon_tex: Option<&egui::TextureHandle>,
) -> bool {
    let (card_rect, card_resp) = ui.allocate_exact_size(Vec2::new(cell_w, card_h), egui::Sense::hover());
    let hovered = card_resp.hovered();

    let bg = if hovered {
        theme::BG_SURFACE_HOVER
    } else {
        theme::BG_INPUT
    };
    let border = if is_ipc_live {
        theme::ACTIVE_GREEN_BORDER
    } else if hovered {
        theme::BORDER_STRONG
    } else {
        theme::BORDER_SUBTLE
    };

    let draw_rect = card_rect.shrink(0.5);
    let painter = ui.painter();
    painter.rect_filled(draw_rect, Rounding::same(2.0), bg);
    painter.rect_stroke(draw_rect, Rounding::same(2.0), Stroke::new(1.0_f32, border));

    let inner_rect = draw_rect.shrink2(Vec2::new(10.0, 8.0));
    let mut child_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child_ui.set_clip_rect(ui.clip_rect().intersect(inner_rect.expand(1.0)));
    child_ui.spacing_mut().item_spacing = Vec2::new(10.0, 0.0);

    let icon_sz = 32.0_f32;
    if let Some(tex) = icon_tex {
        child_ui.add(
            egui::Image::new(tex)
                .fit_to_exact_size(Vec2::splat(icon_sz))
                .rounding(Rounding::same(4.0)),
        );
    } else {
        theme::draw_monogram_icon(&mut child_ui, icon_sz, name);
    }

    let btn_w = 78.0_f32;
    let btn_h = 26.0_f32;
    let text_w = (inner_rect.width() - icon_sz - btn_w - 20.0).max(50.0);

    child_ui.allocate_ui_with_layout(
        Vec2::new(text_w, inner_rect.height()),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
            ui.add_space(3.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(
                egui::Label::new(RichText::new(name).size(11.5).strong().color(Color32::WHITE))
                    .truncate(),
            )
            .on_hover_text(name);

            let status_text = if is_ipc_live {
                format!("● {}", broadcasting_badge)
            } else {
                running_badge.to_string()
            };
            ui.add(
                egui::Label::new(
                    RichText::new(status_text)
                        .size(9.5)
                        .color(if is_ipc_live { theme::ACTIVE_GREEN_TEXT } else { theme::TEXT_MUTED }),
                )
                .truncate(),
            );
        },
    );

    let mut replace_clicked = false;
    child_ui.allocate_ui_with_layout(
        Vec2::new(btn_w, inner_rect.height()),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            if ui
                .add_sized(Vec2::new(btn_w, btn_h), theme::outline_button(replace_label, theme::TEXT_PRIMARY))
                .clicked()
            {
                replace_clicked = true;
            }
        },
    );

    replace_clicked
}
