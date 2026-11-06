#![windows_subsystem = "windows"]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use chrono::Local;
use eframe::egui::{self, Color32, RichText, Rounding, Stroke, Vec2};
use serde::{Deserialize, Serialize};

// Вшитая база данных всех 25 887 игр Discord
const GAMES_DB_RAW: &str = include_str!("../games_db.json");

#[cfg(windows)]
fn get_running_process_names() -> HashSet<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut names = HashSet::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot != INVALID_HANDLE_VALUE {
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snapshot, &mut entry) != 0 {
                loop {
                    let len = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let name = String::from_utf16_lossy(&entry.szExeFile[..len])
                        .to_lowercase()
                        .trim()
                        .to_string();
                    if !name.is_empty() {
                        names.insert(name);
                    }
                    if Process32NextW(snapshot, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snapshot);
        }
    }
    names
}

#[cfg(not(windows))]
fn get_running_process_names() -> HashSet<String> {
    HashSet::new()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DbGame {
    name: String,
    exe: String,
    #[serde(default)]
    id: String,
}

#[derive(Clone, Debug)]
struct ActiveGame {
    name: String,
    exe_name: String,
    #[allow(dead_code)]
    window_title: String,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum AppMode {
    SingleGame,  // 1: Запустить 1 игру через поиск
    RandomBatch, // 2: Случайные игры (Рандом)
    Settings,    // 3: Настройки
}

mod i18n;
use i18n::*;

mod discord_ipc;
use discord_ipc::DiscordIpc;

mod updater;
use updater::{AutoUpdater, UpdateStatus};

#[derive(PartialEq, Clone, Debug, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_language")]
    pub language: Language,
    #[serde(default = "default_ui_scale")]
    pub ui_scale: UiScale,
    #[serde(default = "default_interval_minutes")]
    pub interval_minutes: u32,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_true")]
    pub exclude_running_pc_games: bool,
}

fn default_language() -> Language { Language::Ru }
fn default_ui_scale() -> UiScale { UiScale::Normal }
fn default_interval_minutes() -> u32 { 15 }
fn default_batch_size() -> usize { 8 }
fn default_true() -> bool { true }

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: default_language(),
            ui_scale: default_ui_scale(),
            interval_minutes: default_interval_minutes(),
            batch_size: default_batch_size(),
            exclude_running_pc_games: true,
        }
    }
}

impl AppSettings {
    fn file_path() -> PathBuf {
        if let Ok(curr_exe) = std::env::current_exe() {
            if let Some(parent) = curr_exe.parent() {
                return parent.join("settings.json");
            }
        }
        PathBuf::from("settings.json")
    }

    pub fn load() -> Self {
        let path = Self::file_path();
        if path.exists() {
            if let Ok(data) = std::fs::read_to_string(&path) {
                if let Ok(settings) = serde_json::from_str(&data) {
                    return settings;
                }
            }
        }
        let def = Self::default();
        def.save();
        def
    }

    pub fn save(&self) {
        let path = Self::file_path();
        if let Ok(data) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, data);
        }
    }
}

struct RunningProcess {
    game: ActiveGame,
    child: Child,
    #[allow(dead_code)]
    stub_path: PathBuf,
}

struct GameFarmerApp {
    // Настройки интерфейса
    language: Language,
    ui_scale: UiScale,

    // Режим работы
    app_mode: AppMode,

    // База игр
    all_games: Vec<DbGame>,
    search_query: String,
    last_search_query: String,
    filtered_indices: Vec<usize>,

    // Время работы (в минутах)
    interval_minutes: u32,

    // Настройки для режима рандома
    batch_size: usize,
    exclude_running_pc_games: bool,
    farmed_history_names: HashSet<String>,
    detected_pc_processes: HashSet<String>,

    // Текущее состояние работы
    is_running: bool,
    batch_started_at: Option<Instant>,
    total_seconds_farmed: u64,
    last_second_tick: Instant,

    // Список активных запущенных процессов
    running_processes: Vec<RunningProcess>,

    // Журнал
    logs: Vec<String>,

    // Сохраненное состояние настроек для автосохранения
    saved_settings: AppSettings,

    // IPC клиент Discord Rich Presence
    discord_ipc: DiscordIpc,

    // Автообновление
    updater: AutoUpdater,
}

impl Default for GameFarmerApp {
    fn default() -> Self {
        AutoUpdater::cleanup_previous_update();
        let mut updater = AutoUpdater::new();
        updater.check_github_update();

        let loaded_settings = AppSettings::load();
        let mut all_games: Vec<DbGame> = serde_json::from_str(GAMES_DB_RAW).unwrap_or_default();
        all_games.retain(|g| !g.name.trim().is_empty() && (!g.exe.trim().is_empty() || !g.id.trim().is_empty()));
        // Заменяем непечатаемые иероглифы на читаемые названия
        for g in &mut all_games {
            g.name = clean_game_name(&g.name, &g.exe);
        }
        all_games.retain(|g| !g.name.trim().is_empty());
        // Сортировка по алфавиту (A -> Z)
        all_games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        // Дедупликация по названию
        let mut seen = HashSet::new();
        all_games.retain(|g| seen.insert(g.name.to_lowercase()));
        let total_db_count = all_games.len();

        let logs = vec![format!(
            "[{}] База Discord: {} игр загружена. Готово к работе.",
            Local::now().format("%H:%M:%S"),
            total_db_count
        )];

        Self {
            language: loaded_settings.language,
            ui_scale: loaded_settings.ui_scale,
            app_mode: AppMode::SingleGame,
            all_games,
            search_query: String::new(),
            last_search_query: String::new(),
            filtered_indices: Vec::new(),
            interval_minutes: loaded_settings.interval_minutes,
            batch_size: loaded_settings.batch_size,
            exclude_running_pc_games: loaded_settings.exclude_running_pc_games,
            farmed_history_names: HashSet::new(),
            detected_pc_processes: HashSet::new(),
            is_running: false,
            batch_started_at: None,
            total_seconds_farmed: 0,
            last_second_tick: Instant::now(),
            running_processes: Vec::new(),
            logs,
            saved_settings: loaded_settings,
            discord_ipc: DiscordIpc::new(),
            updater,
        }
    }
}

fn game_matches_query(name: &str, exe: &str, raw_query: &str) -> bool {
    let q = raw_query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }

    let name_lower = name.to_lowercase();
    let exe_lower = exe.to_lowercase();

    // 0. Если запрос чисто числовой (например "4", "3", "2"):
    // Ищем ТОЛЬКО по названию игры (отсекая x64, win64, wow-64 в путях файлов)!
    if q.chars().all(|c| c.is_numeric()) {
        let clean_name = name_lower.replace(':', " ").replace('-', " ");
        let has_word = clean_name.split_whitespace().any(|w| w == q);
        let has_contain = name_lower.contains(&format!(" {}", q)) || name_lower.ends_with(&q);
        return has_word || has_contain;
    }

    // 1. Прямое вхождение подстроки в название игры
    if name_lower.contains(&q) || exe_lower.contains(&q) {
        return true;
    }

    // 1.5. Warcraft: "варкрафт", "вахи", "wow", "вовка", "warcraft"
    if q.starts_with("варкрафт") || q.starts_with("ворлд оф варкрафт") || q == "warcraft" || q == "wow" || q == "вов" || q == "вовка" {
        if name_lower.contains("warcraft") || exe_lower.contains("wow") || name_lower.contains("frozen throne") {
            let digits: String = q.chars().filter(|c| c.is_numeric()).collect();
            if !digits.is_empty() {
                return name_lower.contains(&digits);
            }
            return true;
        }
    }

    // 1.6. Modern Warfare 4: "modern warfare 4", "модерн варфейр 4", "модернварфеер 4", "mw 4", "mw4"
    if q.contains("modern warfare 4") || q.contains("модерн варфейр 4") || q.contains("модернварфеер 4") || q.contains("mw4") || q.contains("mw 4") {
        if name_lower.contains("modern warfare") && name_lower.contains("4") {
            return true;
        }
    }

    // 2. Обработка запросов для гача-игр: "гачи", "гача", "gacha", "аниме", "anime"
    if q.contains("гач") || q.contains("gacha") || q == "аниме" || q == "anime" {
        let gacha_keywords = [
            "genshin", "honkai", "star rail", "zenless", "wuthering", "nikke",
            "blue archive", "infinity nikki", "reverse: 1999", "solo leveling",
            "endfield", "arknights", "fate", "pgr", "gray raven", "azur lane",
            "snowbreak", "tower of fantasy", "brown dust", "yuanshen"
        ];
        if gacha_keywords.iter().any(|k| name_lower.contains(k) || exe_lower.contains(k)) {
            return true;
        }
    }

    // 2.5. Обработка запросов на квесты и орбсы Discord: "орбсы", "орб", "orbs", "квесты", "quests"
    if q.contains("орб") || q.contains("orb") || q.contains("квест") || q.contains("quest") {
        let quest_titles = [
            "modern warfare 4", "sp26-cod", "world of warcraft: forever", "wowb",
            "where winds meet", "wwm", "99 nights in the forest", "robloxplayerbeta",
            "delta force", "arc raiders", "pioneer", "arena breakout",
            "escape the backrooms", "the finals", "marvel rivals", "monster hunter wilds",
            "black ops 6", "genshin", "honkai", "star rail", "zenless", "wuthering",
            "fallout 4", "albion online", "endfield", "overwatch", "apex", "fortnite",
            "valorant", "league of legends"
        ];
        if quest_titles.iter().any(|k| name_lower.contains(k) || exe_lower.contains(k)) {
            return true;
        }
    }

    // 3. Обработка синонимов Call of Duty: "cod", "колда", "кол оф дьюти", "код", "call of duty", "mw"
    if q.starts_with("cod") || q.starts_with("код") || q.starts_with("колд") || q.starts_with("кол оф дь") || q == "call of duty" || q.starts_with("call of duty") || q.starts_with("modern warfare") || q == "mw4" || q == "mw 4" {
        if name_lower.contains("call of duty") || exe_lower.contains("cod") || name_lower.contains("modern warfare") {
            let digits: String = q.chars().filter(|c| c.is_numeric()).collect();
            if !digits.is_empty() {
                return name_lower.contains(&digits) || exe_lower.contains(&digits);
            }
            return true;
        }
    }

    // 4. Пословное совпадение (все слова из запроса должны быть в названии или пути)
    // Например: "call of duty modern" -> проверяет, что в названии есть "call", "of", "duty", "modern"
    // Это находит "Call of Duty 4: Modern Warfare"!
    let words: Vec<&str> = q.split_whitespace().collect();
    if words.len() > 1 {
        let all_words_match = words.iter().all(|w| {
            name_lower.contains(w) || exe_lower.contains(w)
        });
        if all_words_match {
            return true;
        }
    }

    // 5. Русские запросы для популярных игр
    if (q == "геншин" || q.starts_with("генш")) && (name_lower.contains("genshin") || exe_lower.contains("genshin")) { return true; }
    if (q == "хонкай" || q == "хср" || q == "стар рейл" || q.starts_with("хонк")) && (name_lower.contains("honkai") || name_lower.contains("star rail")) { return true; }
    if (q == "зенлесс" || q == "ззз" || q == "zzz" || q.starts_with("зенл")) && name_lower.contains("zenless") { return true; }
    if (q == "вува" || q == "wuwa" || q == "вуву" || q.starts_with("вув")) && (name_lower.contains("wuthering") || exe_lower.contains("wuthering")) { return true; }
    if (q == "никке" || q.starts_with("никк")) && (name_lower.contains("nikke") || exe_lower.contains("nikke")) { return true; }
    if (q == "блю архив" || q == "блу архив" || q == "ba") && name_lower.contains("blue archive") { return true; }
    if (q == "варзон" || q == "варзона") && (name_lower.contains("warzone") || name_lower.contains("modern warfare")) { return true; }
    if (q == "гта" || q == "гта 5" || q == "gta") && (name_lower.contains("grand theft auto") || exe_lower.contains("gta")) { return true; }
    if (q == "кс" || q == "ксочка" || q == "контра" || q == "кс2") && (name_lower.contains("counter-strike") || exe_lower.contains("cs")) { return true; }
    if (q == "дота" || q == "дотка") && name_lower.contains("dota") { return true; }
    if (q == "ведьмак") && name_lower.contains("witcher") { return true; }
    if (q == "киберпанк") && name_lower.contains("cyberpunk") { return true; }

    false
}

fn sanitize_title_symbols(s: &str) -> String {
    let mut out = s.to_string();
    out = out.replace(['\u{200B}', '\u{FEFF}'], "");
    out = out.replace(['™', '®', '©'], "");
    out = out.replace(['–', '—', '−'], "-");
    out = out.replace(['‘', '’', '`'], "'");
    out = out.replace(['“', '”', '„', '«', '»'], "\"");
    out = out.replace('＆', "&");
    out = out.replace('•', " - ");
    out = out.replace('⁵', "5");
    out = out.replace('Ⅲ', "3");
    out = out.replace('∞', "Infinity");
    out = out.replace('π', "Pi");
    out = out.replace('💥', "");

    // Удаляем ведущие спецсимволы (#, ', ., // и т.д.) в начале названия игры
    let trimmed = out.trim_start_matches(|c: char| !c.is_alphanumeric());
    if !trimmed.trim().is_empty() {
        out = trimmed.to_string();
    }
    out
}

fn is_cjk_or_unsupported(c: char) -> bool {
    matches!(c,
        '\u{4e00}'..='\u{9fff}' |
        '\u{3400}'..='\u{4dbf}' |
        '\u{3040}'..='\u{309f}' |
        '\u{30a0}'..='\u{30ff}' |
        '\u{ac00}'..='\u{d7af}' |
        '\u{1100}'..='\u{11ff}' |
        '\u{3000}'..='\u{303f}' |
        '\u{ff00}'..='\u{ffef}'
    )
}

fn clean_game_name(raw_name: &str, exe: &str) -> String {
    let sanitized = sanitize_title_symbols(raw_name);

    if !sanitized.chars().any(is_cjk_or_unsupported) {
        let clean = sanitized.trim().to_string();
        if !clean.is_empty() {
            return clean;
        }
    }

    // Попытка 1: извлечь латинскую или кириллическую часть из исходного названия
    let filtered_chars: String = sanitized
        .chars()
        .map(|c| if is_cjk_or_unsupported(c) { ' ' } else { c })
        .collect();

    let cleaned_part = filtered_chars
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| !c.is_alphanumeric())
        .trim()
        .to_string();

    if cleaned_part.chars().filter(|c| c.is_alphabetic()).count() >= 3 {
        return cleaned_part;
    }

    // Попытка 2: извлечь читаемое имя из пути или имени .exe файла
    let normalized = exe.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').collect();

    let mut candidate = String::new();
    for p in &parts {
        let clean = p.trim_end_matches(".exe")
            .replace('_', " ")
            .replace('-', " ")
            .trim()
            .to_string();
        let clean_lower = clean.to_lowercase();
        if !clean.is_empty()
            && !clean_lower.starts_with("win64")
            && !clean_lower.starts_with("binaries")
            && !clean_lower.starts_with("shipping")
        {
            candidate = clean;
            break;
        }
    }

    if candidate.is_empty() {
        if let Some(last) = parts.last() {
            candidate = last.trim_end_matches(".exe").replace('_', " ").replace('-', " ").trim().to_string();
        }
    }

    // Title Case (каждое слово с большой буквы)
    let title_cased = candidate
        .split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    if title_cased.is_empty() {
        raw_name.trim().to_string()
    } else {
        title_cased
    }
}

impl GameFarmerApp {
    fn add_log(&mut self, text: &str) {
        let time_str = Local::now().format("%H:%M:%S").to_string();
        self.logs.push(format!("[{}] {}", time_str, text));
        if self.logs.len() > 100 {
            self.logs.remove(0);
        }
    }

    /// Поиск исходного файла dummy_game.exe
    fn find_dummy_executable() -> Option<PathBuf> {
        if let Ok(curr_exe) = std::env::current_exe() {
            if let Some(parent) = curr_exe.parent() {
                let candidate = parent.join("dummy_game.exe");
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
        let debug_path = Path::new("target/debug/dummy_game.exe");
        if debug_path.exists() {
            return Some(debug_path.to_path_buf());
        }
        let release_path = Path::new("target/release/dummy_game.exe");
        if release_path.exists() {
            return Some(release_path.to_path_buf());
        }
        let root_path = Path::new("dummy_game.exe");
        if root_path.exists() {
            return Some(root_path.to_path_buf());
        }
        None
    }

    /// Остановка ВСЕХ запущенных процессов
    fn stop_all_active_games(&mut self) {
        for mut rp in self.running_processes.drain(..) {
            let _ = rp.child.kill();
            let _ = rp.child.wait();
        }
    }

    /// Запуск одной конкретной игры на выбранное время
    fn launch_single_game(&mut self, name: String, exe: String, app_id: String) {
        self.stop_all_active_games();
        self.discord_ipc.clear_activity();
        self.app_mode = AppMode::SingleGame;

        let clean_title = if let Some(idx) = name.find('[') {
            name[..idx].trim().to_string()
        } else {
            name.clone()
        };

        // 1. Отправляем активность в Discord через локальный IPC по ID игры
        if !app_id.is_empty() {
            self.discord_ipc.set_activity(&app_id, &clean_title, "In Game");
        }

        // 2. Запускаем stub-процесс (для Windows-хуков Discord и прохождения квестов)
        let dummy_exe = match Self::find_dummy_executable() {
            Some(p) => p,
            None => {
                self.add_log("Внимание: dummy_game.exe не найден (активность установлена через Discord IPC)");
                self.is_running = true;
                self.batch_started_at = Some(Instant::now());
                return;
            }
        };

        let clean_exe = exe.trim().replace(['<', '>'], "");
        let target_stub = if !clean_exe.is_empty() {
            let stubs_dir = Path::new("stubs");
            let stub_path = stubs_dir.join(&clean_exe);
            if let Some(parent) = stub_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let need_copy = !stub_path.exists()
                || std::fs::metadata(&stub_path).map(|m| m.len()).unwrap_or(0)
                    != std::fs::metadata(&dummy_exe).map(|m| m.len()).unwrap_or(0);
            if need_copy {
                let _ = std::fs::copy(&dummy_exe, &stub_path);
            }
            stub_path
        } else {
            dummy_exe
        };

        let mut cmd = Command::new(&target_stub);
        if let Some(parent) = target_stub.parent() {
            cmd.current_dir(parent);
        }
        cmd.arg(&clean_title);

        match cmd.spawn() {
            Ok(child) => {
                self.running_processes.push(RunningProcess {
                    game: ActiveGame {
                        name: clean_title.clone(),
                        exe_name: clean_exe.clone(),
                        window_title: clean_title.clone(),
                    },
                    child,
                    stub_path: target_stub,
                });
                self.is_running = true;
                self.batch_started_at = Some(Instant::now());
                self.add_log(&format!(
                    "Запущена игра '{}' на {} мин. (Процесс + Discord IPC активность)",
                    name, self.interval_minutes
                ));
            }
            Err(e) => {
                self.add_log(&format!("Ошибка запуска процесса {}: {}", clean_exe, e));
                self.is_running = true;
                self.batch_started_at = Some(Instant::now());
            }
        }
    }

    /// Запуск пачки случайных игр
    fn launch_random_batch(&mut self) {
        self.stop_all_active_games();
        self.discord_ipc.clear_activity();
        self.app_mode = AppMode::RandomBatch;

        let dummy_exe = match Self::find_dummy_executable() {
            Some(p) => p,
            None => {
                self.add_log("ОШИБКА: dummy_game.exe не найден!");
                self.is_running = false;
                return;
            }
        };

        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();

        self.detected_pc_processes = get_running_process_names();

        let mut excluded_on_pc = Vec::new();
        let candidates: Vec<DbGame> = self
            .all_games
            .iter()
            .filter(|g| {
                if g.exe.is_empty() {
                    return true;
                }
                let exe_lower = Path::new(&g.exe)
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or(&g.exe)
                    .to_lowercase();

                if self.exclude_running_pc_games && self.detected_pc_processes.contains(&exe_lower) {
                    if !excluded_on_pc.contains(&exe_lower) {
                        excluded_on_pc.push(exe_lower);
                    }
                    return false;
                }

                true
            })
            .cloned()
            .collect();

        if !excluded_on_pc.is_empty() {
            self.add_log(&format!(
                "Исключены запущенные на ПК игры: {}",
                excluded_on_pc.join(", ")
            ));
        }

        let mut unvisited: Vec<DbGame> = candidates
            .iter()
            .filter(|g| !self.farmed_history_names.contains(&g.name))
            .cloned()
            .collect();

        if unvisited.len() < self.batch_size {
            self.farmed_history_names.clear();
            unvisited = candidates.clone();
        }

        unvisited.shuffle(&mut rng);
        let picked: Vec<DbGame> = unvisited.into_iter().take(self.batch_size).collect();

        if picked.is_empty() {
            self.add_log("Не найдено подходящих игр для запуска.");
            self.is_running = false;
            return;
        }

        let stubs_dir = Path::new("stubs");
        let _ = std::fs::create_dir_all(stubs_dir);

        let mut launched_names = Vec::new();
        let mut first_game_id = None;
        let mut first_game_name = None;

        for db_game in picked {
            self.farmed_history_names.insert(db_game.name.clone());
            if first_game_id.is_none() && !db_game.id.is_empty() {
                first_game_id = Some(db_game.id.clone());
                first_game_name = Some(db_game.name.clone());
            }

            let clean_exe = db_game.exe.trim().replace(['<', '>'], "");
            let target_stub = if !clean_exe.is_empty() {
                let stub_path = stubs_dir.join(&clean_exe);
                if let Some(parent) = stub_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let need_copy = !stub_path.exists()
                    || std::fs::metadata(&stub_path).map(|m| m.len()).unwrap_or(0)
                        != std::fs::metadata(&dummy_exe).map(|m| m.len()).unwrap_or(0);
                if need_copy {
                    let _ = std::fs::copy(&dummy_exe, &stub_path);
                }
                stub_path
            } else {
                dummy_exe.clone()
            };

            let mut cmd = Command::new(&target_stub);
            if let Some(parent) = target_stub.parent() {
                cmd.current_dir(parent);
            }

            cmd.arg(&db_game.name);

            if let Ok(child) = cmd.spawn() {
                launched_names.push(db_game.name.clone());
                self.running_processes.push(RunningProcess {
                    game: ActiveGame {
                        name: db_game.name.clone(),
                        exe_name: clean_exe,
                        window_title: db_game.name,
                    },
                    child,
                    stub_path: target_stub,
                });
            }
        }

        if let (Some(app_id), Some(game_name)) = (first_game_id, first_game_name) {
            self.discord_ipc.set_activity(&app_id, &game_name, "In Game");
        }

        self.is_running = true;
        self.batch_started_at = Some(Instant::now());
        self.add_log(&format!(
            "Запущен рандом: {} игр параллельно на {} мин. ({})",
            self.running_processes.len(),
            self.interval_minutes,
            launched_names.join(", ")
        ));
    }

    /// Завершение сессии или смена пачки по истечении времени
    fn handle_timer_finished(&mut self) {
        match self.app_mode {
            AppMode::SingleGame | AppMode::Settings => {
                let game_name = self.running_processes.first().map(|rp| rp.game.name.clone()).unwrap_or_default();
                self.stop_farming();
                self.add_log(&format!(
                    "Время вышло ({} мин). Игра '{}' остановлена. Сессия завершена, активность в Discord зафиксирована!",
                    self.interval_minutes, game_name
                ));
            }
            AppMode::RandomBatch => {
                self.add_log("Время вышло! Ротация: выбор следующей пачки случайных игр...");
                self.launch_random_batch();
            }
        }
    }

    /// Остановка фарма
    fn stop_farming(&mut self) {
        self.is_running = false;
        self.stop_all_active_games();
        self.discord_ipc.clear_activity();
        self.batch_started_at = None;
        self.add_log("Все фоновые процессы и статус Discord остановлены.");
    }
}

impl eframe::App for GameFarmerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(200));

        // Опрос статуса автообновления
        if let Some(log_msg) = self.updater.poll() {
            self.add_log(&log_msg);
        }

        // Таймер активности
        if self.is_running {
            if self.last_second_tick.elapsed() >= Duration::from_secs(1) {
                self.total_seconds_farmed += 1;
                self.last_second_tick = Instant::now();
            }

            if let Some(started_at) = self.batch_started_at {
                let target_duration = Duration::from_secs((self.interval_minutes as u64) * 60);
                if started_at.elapsed() >= target_duration {
                    self.handle_timer_finished();
                }
            }
        }

        // Применяем масштаб интерфейса
        ctx.set_pixels_per_point(self.ui_scale.factor());
        let t = get_i18n(self.language);

        let bg_frame = egui::Frame::none()
            .fill(Color32::from_rgb(30, 30, 30))
            .inner_margin(egui::Margin::symmetric(14.0, 10.0));

        egui::CentralPanel::default().frame(bg_frame).show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);

            // ==========================================
            // 1. ХЕДЕР
            // ==========================================
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(t.title)
                        .size(13.0)
                        .strong()
                        .color(Color32::from_rgb(220, 220, 225)),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.is_running {
                        let count = self.running_processes.len();
                        let text = if count == 1 {
                            t.active_single.to_string()
                        } else {
                            t.active_multi.replace("{}", &count.to_string())
                        };
                        ui.label(
                            RichText::new(text)
                                .size(11.5)
                                .color(Color32::from_rgb(56, 172, 88))
                                .strong(),
                        );
                    } else {
                        ui.label(
                            RichText::new(t.stopped_badge)
                                .size(11.0)
                                .color(Color32::from_rgb(130, 130, 130)),
                        );
                    }
                });
            });

            // ==========================================
            // 1.5. УВЕДОМЛЕНИЕ ОБ ОБНОВЛЕНИИ (ИН-ПЛЕЙС БАННЕР)
            // ==========================================
            match self.updater.status.clone() {
                UpdateStatus::Available { version, changelog, download_url } => {
                    let banner_frame = egui::Frame::none()
                        .fill(Color32::from_rgb(18, 32, 45))
                        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(0, 140, 255)))
                        .rounding(Rounding::same(4.0))
                        .inner_margin(egui::Margin::symmetric(10.0, 6.0));

                    banner_frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // 1. Текст слева: название версии
                            let banner_text = t.update_available_banner.replace("{}", &version);
                            ui.label(
                                RichText::new(banner_text)
                                    .size(12.0)
                                    .strong()
                                    .color(Color32::from_rgb(90, 195, 255)),
                            );

                            // Описание (changelog), ограниченное так, чтобы оставалось 210px под кнопку
                            if !changelog.is_empty() {
                                let max_changelog_w = (ui.available_width() - 210.0).max(0.0);
                                if max_changelog_w > 40.0 {
                                    let changelog_label = egui::Label::new(
                                        RichText::new(format!("({})", changelog))
                                            .size(11.0)
                                            .color(Color32::from_rgb(180, 205, 220)),
                                    ).truncate();
                                    ui.add_sized(Vec2::new(max_changelog_w, 20.0), changelog_label);
                                }
                            }

                            // 2. Кнопка на правом краю
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let update_btn = egui::Button::new(
                                    RichText::new(t.btn_apply_update)
                                        .size(11.0)
                                        .strong()
                                        .color(Color32::WHITE),
                                )
                                .fill(Color32::from_rgb(0, 122, 204))
                                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(30, 150, 240)))
                                .rounding(Rounding::same(3.0));

                                if ui.add_sized(Vec2::new(190.0, 24.0), update_btn).clicked() {
                                    self.updater.start_download_and_install(download_url, version.clone());
                                    self.add_log(&format!("Загрузка инсталлятора обновления {} с GitHub...", version));
                                }
                            });
                        });
                    });
                    ui.add_space(2.0);
                }
                UpdateStatus::Downloading { version, progress, downloaded, total } => {
                    let banner_frame = egui::Frame::none()
                        .fill(Color32::from_rgb(18, 35, 30))
                        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(0, 180, 120)))
                        .rounding(Rounding::same(4.0))
                        .inner_margin(egui::Margin::symmetric(10.0, 6.0));

                    banner_frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let mb_down = downloaded as f64 / 1_048_576.0;
                            let mb_total = total as f64 / 1_048_576.0;
                            let text = format!("Скачивание v{}: {:.0}% ({:.1} / {:.1} МБ)", version, progress * 100.0, mb_down, mb_total);
                            ui.label(
                                RichText::new(text)
                                    .size(12.0)
                                    .strong()
                                    .color(Color32::from_rgb(100, 235, 160)),
                            );

                            ui.add(egui::ProgressBar::new(progress).desired_width(180.0));
                        });
                    });
                    ui.add_space(2.0);
                }
                UpdateStatus::Installing => {
                    let banner_frame = egui::Frame::none()
                        .fill(Color32::from_rgb(25, 25, 40))
                        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(120, 120, 240)))
                        .rounding(Rounding::same(4.0))
                        .inner_margin(egui::Margin::symmetric(10.0, 6.0));

                    banner_frame.show(ui, |ui| {
                        ui.label(
                            RichText::new("Запуск инсталлятора и перезапуск программы...")
                                .size(12.0)
                                .strong()
                                .color(Color32::from_rgb(160, 180, 255)),
                        );
                    });
                    ui.add_space(2.0);
                }
                _ => {}
            }

            // ==========================================
            // 2. ВКЛАДКИ (ANTIGRAVITY IDE EDITOR TABS)
            // ==========================================
            let tab_bar_frame = egui::Frame::none()
                .fill(Color32::from_rgb(24, 24, 24))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(40, 40, 40)))
                .rounding(Rounding::same(3.0))
                .inner_margin(egui::Margin::symmetric(4.0, 0.0));

            tab_bar_frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);

                    let tabs = [
                        (AppMode::SingleGame, t.tab_search),
                        (AppMode::RandomBatch, t.tab_random),
                        (AppMode::Settings, t.tab_settings),
                    ];

                    for (mode, label) in tabs {
                        let is_active = self.app_mode == mode;
                        let bg_color = if is_active {
                            Color32::from_rgb(30, 30, 30)
                        } else {
                            Color32::from_rgb(24, 24, 24)
                        };
                        let text_color = if is_active {
                            Color32::WHITE
                        } else {
                            Color32::from_rgb(150, 150, 150)
                        };

                        let btn = egui::Button::new(
                            RichText::new(label)
                                .size(11.5)
                                .strong()
                                .color(text_color),
                        )
                        .fill(bg_color)
                        .stroke(if is_active {
                            Stroke::new(1.0_f32, Color32::from_rgb(50, 50, 50))
                        } else {
                            Stroke::NONE
                        })
                        .rounding(Rounding { nw: 3.0, ne: 3.0, sw: 0.0, se: 0.0 });

                        let resp = ui.add_sized(Vec2::new(185.0, 30.0), btn);

                        // Top accent line on active tab
                        if is_active {
                            let rect = resp.rect;
                            let top_line = egui::Rect::from_min_max(
                                rect.min,
                                egui::pos2(rect.max.x, rect.min.y + 2.0),
                            );
                            ui.painter().rect_filled(top_line, Rounding::ZERO, Color32::from_rgb(0, 122, 204));
                        }

                        if resp.clicked() {
                            self.app_mode = mode;
                        }
                    }
                });
            });

            // ==========================================
            // 3. АКТИВНЫЙ СТАТУС (ЕСЛИ ИГРА ЗАПУЩЕНА)
            // ==========================================
            if self.is_running && !self.running_processes.is_empty() {
                let active_frame = egui::Frame::none()
                    .fill(Color32::from_rgb(20, 36, 24))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(35, 165, 90)))
                    .rounding(Rounding::same(4.0))
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0));

                active_frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            if self.running_processes.len() == 1 {
                                let proc = &self.running_processes[0];
                                ui.label(
                                    RichText::new(format!("{} {}", t.active_card_title, proc.game.name))
                                        .size(13.0)
                                        .strong()
                                        .color(Color32::from_rgb(70, 215, 115)),
                                );
                                ui.label(
                                    RichText::new(t.active_process_info.replace("{}", &proc.game.exe_name))
                                        .size(10.5)
                                        .color(Color32::from_rgb(160, 180, 160)),
                                );
                            } else {
                                ui.label(
                                    RichText::new(format!("[АКТИВНО: {} ИГР ПАРАЛЛЕЛЬНО В DISCORD]", self.running_processes.len()))
                                        .size(13.0)
                                        .strong()
                                        .color(Color32::from_rgb(70, 215, 115)),
                                );
                                ui.label(
                                    RichText::new("Все запущенные игры учитываются Discord")
                                        .size(10.5)
                                        .color(Color32::from_rgb(160, 180, 160)),
                                );
                            }
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let stop_btn = egui::Button::new(
                                RichText::new(t.btn_stop)
                                    .size(12.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .fill(Color32::from_rgb(218, 55, 60))
                            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(240, 80, 85)))
                            .rounding(Rounding::same(4.0));

                            if ui.add_sized(Vec2::new(140.0, 30.0), stop_btn).clicked() {
                                self.stop_farming();
                            }
                        });
                    });

                    // Countdown & progress bar
                    let total_secs = (self.interval_minutes as u64) * 60;
                    let elapsed_secs = self.batch_started_at.map_or(0, |t| t.elapsed().as_secs());
                    let progress = if total_secs > 0 {
                        (elapsed_secs as f32 / total_secs as f32).min(1.0)
                    } else {
                        0.0
                    };
                    let remaining_secs = total_secs.saturating_sub(elapsed_secs);
                    let rem_m = remaining_secs / 60;
                    let rem_s = remaining_secs % 60;

                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let countdown_text = if self.app_mode == AppMode::SingleGame {
                            t.remaining
                                .replace("{:02}:{:02}", &format!("{:02}:{:02}", rem_m, rem_s))
                                .replace("{}", &self.interval_minutes.to_string())
                        } else {
                            format!("До ротации: {:02}:{:02} (из {} мин)", rem_m, rem_s, self.interval_minutes)
                        };
                        ui.label(RichText::new(countdown_text).size(11.0).color(Color32::from_rgb(200, 215, 200)));

                        let total_h = self.total_seconds_farmed / 3600;
                        let total_m = (self.total_seconds_farmed % 3600) / 60;
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let text = t.total_farmed
                                .replacen("{}", &total_h.to_string(), 1)
                                .replacen("{}", &total_m.to_string(), 1);
                            ui.label(RichText::new(text).size(10.5).color(Color32::from_rgb(150, 165, 150)));
                        });
                    });

                    ui.add_space(2.0);
                    let pbar = egui::ProgressBar::new(progress)
                        .fill(Color32::from_rgb(35, 165, 90))
                        .show_percentage();
                    ui.add(pbar);
                });
            }

            // ==========================================
            // 4. ПАНЕЛЬ ВРЕМЕНИ РАБОТЫ (ДЛЯ РЕЖИМОВ 1 И 2)
            // ==========================================
            if self.app_mode != AppMode::Settings {
                let toolbar_frame = egui::Frame::none()
                    .fill(Color32::from_rgb(26, 26, 26))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(40, 40, 40)))
                    .rounding(Rounding::same(4.0))
                    .inner_margin(egui::Margin::symmetric(10.0, 6.0));

                toolbar_frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(t.duration_label)
                                .size(11.5)
                                .strong()
                                .color(Color32::from_rgb(180, 180, 180)),
                        );

                        let presets = [
                            (t.time_2m, 2),
                            (t.time_15m, 15),
                            (t.time_30m, 30),
                            (t.time_1h, 60),
                            (t.time_2h, 120),
                        ];

                        for (label, mins) in presets {
                            let is_active = self.interval_minutes == mins;
                            let btn = egui::Button::new(
                                RichText::new(label)
                                    .size(11.0)
                                    .strong()
                                    .color(if is_active { Color32::WHITE } else { Color32::from_rgb(180, 180, 180) }),
                            )
                            .fill(if is_active { Color32::from_rgb(0, 122, 204) } else { Color32::from_rgb(38, 38, 38) })
                            .stroke(Stroke::new(1.0_f32, if is_active { Color32::from_rgb(30, 144, 255) } else { Color32::from_rgb(50, 50, 50) }))
                            .rounding(Rounding::same(3.0));

                            if ui.add(btn).clicked() {
                                self.interval_minutes = mins;
                            }
                        }

                        ui.add_space(8.0);
                        ui.add(
                            egui::Slider::new(&mut self.interval_minutes, 1..=180)
                                .suffix(format!(" {}", t.min_unit))
                                .text(t.exact_label),
                        );
                    });
                });
            }

            // ==========================================
            // 5. КОНТЕНТ ВЫБРАННОЙ ВКЛАДКИ
            // ==========================================
            match self.app_mode {
                AppMode::SingleGame => {
                    // Строка поиска
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(t.search_label).size(12.0).strong().color(Color32::from_rgb(200, 200, 200)));
                        let search_box = egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text(t.search_placeholder)
                            .desired_width(ui.available_width() - 80.0);
                        ui.add(search_box);

                        let clear_btn = egui::Button::new(RichText::new(t.btn_clear).size(11.0))
                            .fill(Color32::from_rgb(38, 38, 38))
                            .rounding(Rounding::same(3.0));
                        if ui.add(clear_btn).clicked() {
                            self.search_query.clear();
                        }
                    });

                    ui.add_space(2.0);

                    // Обновляем фильтр при изменении поискового запроса
                    let clean_query = self.search_query.trim().to_lowercase();
                    if clean_query != self.last_search_query {
                        self.last_search_query = clean_query.clone();
                        if clean_query.is_empty() {
                            self.filtered_indices.clear();
                        } else {
                            self.filtered_indices = self.all_games
                                .iter()
                                .enumerate()
                                .filter(|(_, g)| game_matches_query(&g.name, &g.exe, &clean_query))
                                .map(|(i, _)| i)
                                .collect();
                        }
                    }

                    let is_empty_query = clean_query.is_empty();
                    let total_items = if is_empty_query {
                        self.all_games.len()
                    } else {
                        self.filtered_indices.len()
                    };

                    let sub_hint = if is_empty_query {
                        format!("Верифицированные игры Steam и Discord ({} игр):", total_items)
                    } else {
                        format!("{} (найдено: {} из {}):", t.search_title, total_items, self.all_games.len())
                    };
                    ui.label(RichText::new(sub_hint).size(11.0).color(Color32::from_rgb(130, 130, 130)));

                    let mut game_to_launch: Option<(String, String, String)> = None;
                    let mut should_stop = false;
                    let running_exes: HashSet<String> = self.running_processes.iter().map(|rp| rp.game.exe_name.to_lowercase()).collect();
                    let interval_min_str = self.interval_minutes.to_string();

                    if total_items == 0 {
                        ui.add_space(20.0);
                        ui.label(
                            RichText::new(t.no_results)
                                .size(12.0)
                                .color(Color32::from_rgb(220, 130, 130)),
                        );
                    } else {
                        let row_height = 36.0;
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show_rows(ui, row_height, total_items, |ui, row_range| {
                                for row_idx in row_range {
                                    let (g_name, g_exe, g_id) = if is_empty_query {
                                        let item = &self.all_games[row_idx];
                                        (item.name.clone(), item.exe.clone(), item.id.clone())
                                    } else {
                                        let item = &self.all_games[self.filtered_indices[row_idx]];
                                        (item.name.clone(), item.exe.clone(), item.id.clone())
                                    };

                                    let is_active = if !g_exe.is_empty() {
                                        running_exes.contains(&g_exe.to_lowercase())
                                    } else {
                                        self.running_processes.iter().any(|rp| rp.game.name == g_name)
                                    };

                                    let row_frame = egui::Frame::none()
                                        .fill(if is_active {
                                            Color32::from_rgb(24, 38, 28)
                                        } else if row_idx % 2 == 0 {
                                            Color32::from_rgb(30, 30, 30)
                                        } else {
                                            Color32::from_rgb(26, 26, 26)
                                        })
                                        .stroke(Stroke::new(1.0_f32, if is_active {
                                            Color32::from_rgb(35, 165, 90)
                                        } else {
                                            Color32::from_rgb(36, 36, 36)
                                        }))
                                        .rounding(Rounding::same(3.0))
                                        .inner_margin(egui::Margin::symmetric(10.0, 5.0));

                                    row_frame.show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            // Порядковый номер (#1, #2...)
                                            ui.label(
                                                RichText::new(format!("#{:<5}", row_idx + 1))
                                                    .size(11.0)
                                                    .monospace()
                                                    .color(Color32::from_rgb(120, 120, 130)),
                                            );

                                            // Название игры
                                            let text_color = if is_active {
                                                Color32::from_rgb(56, 172, 88)
                                            } else {
                                                Color32::from_rgb(235, 235, 235)
                                            };
                                            ui.label(RichText::new(&g_name).strong().size(12.5).color(text_color));

                                            if is_active {
                                                ui.label(
                                                    RichText::new(format!("[{}]", t.active_tag))
                                                        .size(10.0)
                                                        .strong()
                                                        .color(Color32::from_rgb(56, 172, 88)),
                                                );
                                            }

                                            // Кнопка справа
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                if is_active {
                                                    let stop_btn = egui::Button::new(
                                                        RichText::new(t.btn_stop)
                                                            .size(11.0)
                                                            .strong()
                                                            .color(Color32::WHITE),
                                                    )
                                                    .fill(Color32::from_rgb(218, 55, 60))
                                                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(240, 80, 85)))
                                                    .rounding(Rounding::same(3.0));

                                                    if ui.add_sized(Vec2::new(140.0, 24.0), stop_btn).clicked() {
                                                        should_stop = true;
                                                    }
                                                } else {
                                                    let label = t.btn_launch.replace("{}", &interval_min_str);
                                                    let launch_btn = egui::Button::new(
                                                        RichText::new(label)
                                                            .size(11.0)
                                                            .strong()
                                                            .color(Color32::WHITE),
                                                    )
                                                    .fill(Color32::from_rgb(35, 165, 90))
                                                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(45, 185, 105)))
                                                    .rounding(Rounding::same(3.0));

                                                    if ui.add_sized(Vec2::new(140.0, 24.0), launch_btn).clicked() {
                                                        game_to_launch = Some((g_name.clone(), g_exe.clone(), g_id.clone()));
                                                    }
                                                }
                                            });
                                        });
                                    });
                                    ui.add_space(1.0);
                                }
                            });
                    }

                    if should_stop {
                        self.stop_farming();
                    }
                    if let Some((name, exe, id)) = game_to_launch {
                        self.launch_single_game(name, exe, id);
                    }
                }

                AppMode::RandomBatch => {
                    let rand_frame = egui::Frame::none()
                        .fill(Color32::from_rgb(26, 26, 26))
                        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(40, 40, 40)))
                        .rounding(Rounding::same(4.0))
                        .inner_margin(egui::Margin::same(16.0));

                    rand_frame.show(ui, |ui| {
                        ui.label(
                            RichText::new(t.random_title)
                                .size(13.0)
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(t.random_desc.replace("{}", &self.interval_minutes.to_string()))
                                .size(11.5)
                                .color(Color32::from_rgb(160, 160, 160)),
                        );

                        ui.add_space(12.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(t.random_batch_label)
                                    .size(12.0)
                                    .strong()
                                    .color(Color32::from_rgb(200, 200, 200)),
                            );
                            ui.add(egui::Slider::new(&mut self.batch_size, 1..=16).suffix(t.random_batch_suffix));
                        });

                        ui.add_space(14.0);

                        ui.horizontal(|ui| {
                            let (btn_title, btn_color) = if self.is_running && self.app_mode == AppMode::RandomBatch {
                                (t.btn_change_random, Color32::from_rgb(50, 70, 100))
                            } else {
                                (t.btn_start_random, Color32::from_rgb(0, 122, 204))
                            };

                            let start_rand_btn = egui::Button::new(
                                RichText::new(btn_title)
                                    .size(12.5)
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .fill(btn_color)
                            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(0, 150, 255)))
                            .rounding(Rounding::same(4.0));

                            if ui.add_sized(Vec2::new(260.0, 36.0), start_rand_btn).clicked() {
                                self.launch_random_batch();
                            }

                            ui.add_space(12.0);
                            ui.label(
                                RichText::new(t.farmed_count.replace("{}", &self.farmed_history_names.len().to_string()))
                                    .size(12.0)
                                    .color(Color32::from_rgb(56, 172, 88))
                                    .strong(),
                            );
                        });

                        if self.is_running && self.running_processes.len() > 1 {
                            ui.add_space(12.0);
                            ui.separator();
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new("Текущие запущенные процессы:")
                                    .size(11.5)
                                    .strong()
                                    .color(Color32::from_rgb(180, 180, 180)),
                            );
                            ui.add_space(4.0);

                            let num_cols = if ui.available_width() > 700.0 { 4 } else { 2 };
                            egui::Grid::new("random_running_grid")
                                .num_columns(num_cols)
                                .spacing([8.0, 6.0])
                                .show(ui, |ui| {
                                    for (idx, rp) in self.running_processes.iter().enumerate() {
                                        let pill = egui::Frame::none()
                                            .fill(Color32::from_rgb(32, 32, 32))
                                            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(56, 172, 88)))
                                            .rounding(Rounding::same(3.0))
                                            .inner_margin(egui::Margin::symmetric(8.0, 4.0));

                                        pill.show(ui, |ui| {
                                            ui.set_width(180.0);
                                            let name_short = if rp.game.name.chars().count() > 22 {
                                                format!("{}...", rp.game.name.chars().take(20).collect::<String>())
                                            } else {
                                                rp.game.name.clone()
                                            };
                                            ui.label(
                                                RichText::new(format!(">> {}", name_short))
                                                    .size(11.0)
                                                    .strong()
                                                    .color(Color32::WHITE),
                                            );
                                        });

                                        if (idx + 1) % num_cols == 0 {
                                            ui.end_row();
                                        }
                                    }
                                    ui.end_row();
                                });
                        }
                    });
                }

                AppMode::Settings => {
                    let settings_frame = egui::Frame::none()
                        .fill(Color32::from_rgb(26, 26, 26))
                        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(40, 40, 40)))
                        .rounding(Rounding::same(4.0))
                        .inner_margin(egui::Margin::same(16.0));

                    settings_frame.show(ui, |ui| {
                        ui.label(
                            RichText::new(t.settings_title)
                                .size(13.5)
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(10.0);

                        // 1. Язык интерфейса
                        ui.label(
                            RichText::new(t.settings_lang)
                                .size(12.0)
                                .strong()
                                .color(Color32::from_rgb(200, 200, 200)),
                        );
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let langs = [
                                (Language::Ru, "RU Русский"),
                                (Language::Uk, "UA Українська"),
                                (Language::En, "EN English"),
                            ];

                            for (lang, label) in langs {
                                let is_cur = self.language == lang;
                                let btn = egui::Button::new(
                                    RichText::new(label)
                                        .size(11.5)
                                        .strong()
                                        .color(if is_cur { Color32::WHITE } else { Color32::from_rgb(170, 170, 170) }),
                                )
                                .fill(if is_cur { Color32::from_rgb(0, 122, 204) } else { Color32::from_rgb(36, 36, 36) })
                                .stroke(Stroke::new(1.0_f32, if is_cur { Color32::from_rgb(30, 144, 255) } else { Color32::from_rgb(50, 50, 50) }))
                                .rounding(Rounding::same(4.0));

                                if ui.add_sized(Vec2::new(130.0, 28.0), btn).clicked() {
                                    self.language = lang;
                                }
                            }
                        });

                        ui.add_space(16.0);
                        ui.separator();
                        ui.add_space(10.0);

                        // 2. Размер шрифта и масштаб
                        ui.label(
                            RichText::new(t.settings_scale)
                                .size(12.0)
                                .strong()
                                .color(Color32::from_rgb(200, 200, 200)),
                        );
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let scales = [
                                (UiScale::Normal, t.scale_100),
                                (UiScale::Medium, t.scale_115),
                                (UiScale::Large, t.scale_130),
                                (UiScale::Huge, t.scale_145),
                            ];

                            for (scale, label) in scales {
                                let is_cur = self.ui_scale == scale;
                                let btn = egui::Button::new(
                                    RichText::new(label)
                                        .size(11.0)
                                        .strong()
                                        .color(if is_cur { Color32::WHITE } else { Color32::from_rgb(170, 170, 170) }),
                                )
                                .fill(if is_cur { Color32::from_rgb(35, 165, 90) } else { Color32::from_rgb(36, 36, 36) })
                                .stroke(Stroke::new(1.0_f32, if is_cur { Color32::from_rgb(50, 200, 110) } else { Color32::from_rgb(50, 50, 50) }))
                                .rounding(Rounding::same(4.0));

                                if ui.add_sized(Vec2::new(130.0, 28.0), btn).clicked() {
                                    self.ui_scale = scale;
                                }
                            }
                        });

                        ui.add_space(16.0);
                        ui.separator();
                        ui.add_space(10.0);

                        // 3. Безопасность (Здесь находится [Защита])
                        ui.label(
                            RichText::new(t.settings_security)
                                .size(12.0)
                                .strong()
                                .color(Color32::from_rgb(200, 200, 200)),
                        );
                        ui.add_space(6.0);
                        ui.checkbox(
                            &mut self.exclude_running_pc_games,
                            RichText::new(t.protect_pc)
                                .size(11.5)
                                .color(Color32::from_rgb(210, 210, 210)),
                        );

                        ui.add_space(16.0);
                        ui.separator();
                        ui.add_space(10.0);

                        // 4. Обновления программы
                        ui.label(
                            RichText::new(t.settings_updates)
                                .size(12.0)
                                .strong()
                                .color(Color32::from_rgb(200, 200, 200)),
                        );
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("Версия: v{}", self.updater.current_version))
                                    .size(11.5)
                                    .color(Color32::from_rgb(170, 170, 170)),
                            );

                            let check_btn = egui::Button::new(
                                RichText::new(t.btn_check_updates)
                                    .size(11.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .fill(Color32::from_rgb(45, 45, 50))
                            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(70, 70, 75)))
                            .rounding(Rounding::same(3.0));

                            if ui.add(check_btn).clicked() {
                                self.updater.check_github_update();
                                self.add_log("Запрос проверки обновлений на GitHub...");
                            }

                            match &self.updater.status {
                                UpdateStatus::Checking => {
                                    ui.label(
                                        RichText::new("Проверка GitHub...")
                                            .size(11.0)
                                            .color(Color32::from_rgb(200, 200, 100)),
                                    );
                                }
                                UpdateStatus::UpToDate => {
                                    ui.label(
                                        RichText::new(t.status_up_to_date)
                                            .size(11.0)
                                            .strong()
                                            .color(Color32::from_rgb(56, 172, 88)),
                                    );
                                }
                                UpdateStatus::Available { version, .. } => {
                                    ui.label(
                                        RichText::new(format!("[Доступна v{}]", version))
                                            .size(11.0)
                                            .strong()
                                            .color(Color32::from_rgb(90, 195, 255)),
                                    );
                                }
                                UpdateStatus::Downloading { progress, .. } => {
                                    ui.label(
                                        RichText::new(format!("Скачивание: {:.0}%", progress * 100.0))
                                            .size(11.0)
                                            .color(Color32::from_rgb(100, 235, 160)),
                                    );
                                }
                                UpdateStatus::Installing => {
                                    ui.label(
                                        RichText::new("Установка...")
                                            .size(11.0)
                                            .color(Color32::from_rgb(160, 180, 255)),
                                    );
                                }
                                UpdateStatus::Error(err) => {
                                    ui.label(
                                        RichText::new(format!("[Ошибка] {}", err))
                                            .size(11.0)
                                            .color(Color32::from_rgb(220, 100, 100)),
                                    );
                                }
                                _ => {}
                            }
                        });
                    });
                }
            }
        });

        // Автоматическое сохранение настроек при их изменении
        let current_settings = AppSettings {
            language: self.language,
            ui_scale: self.ui_scale,
            interval_minutes: self.interval_minutes,
            batch_size: self.batch_size,
            exclude_running_pc_games: self.exclude_running_pc_games,
        };
        if current_settings != self.saved_settings {
            current_settings.save();
            self.saved_settings = current_settings;
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_all_active_games();
        self.discord_ipc.clear_activity();
        self.saved_settings.save();
    }
}

const ICON_RGBA: &[u8] = include_bytes!("../app_icon_64.rgba");

fn setup_custom_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    if let Ok(font_data) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
        fonts.font_data.insert(
            "segoe_ui".to_owned(),
            egui::FontData::from_owned(font_data),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "segoe_ui".to_owned());
    }

    if let Ok(sym_data) = std::fs::read(r"C:\Windows\Fonts\seguisym.ttf") {
        fonts.font_data.insert(
            "segoe_sym".to_owned(),
            egui::FontData::from_owned(sym_data),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .push("segoe_sym".to_owned());
    }

    ctx.set_fonts(fonts);
}

fn main() -> eframe::Result<()> {
    let autostart = std::env::args().any(|a| a == "--autostart");
    let icon_data = egui::IconData {
        rgba: ICON_RGBA.to_vec(),
        width: 64,
        height: 64,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([880.0, 740.0])
            .with_min_inner_size([720.0, 600.0])
            .with_title("Discord Game Activity Manager")
            .with_icon(icon_data)
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        "Discord Game Activity Manager",
        options,
        Box::new(move |cc| {
            setup_custom_fonts(&cc.egui_ctx);
            let mut app = GameFarmerApp::default();
            if autostart {
                app.launch_random_batch();
            }
            Ok(Box::new(app))
        }),
    )
}
