#![windows_subsystem = "windows"]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use chrono::Local;
use eframe::egui;
use serde::{Deserialize, Serialize};

// Вшитая база данных всех 25 887 игр Discord
const GAMES_DB_RAW: &str = include_str!("../games_db.json");

// Вшитый исполняемый файл процесса-заглушки (для автономной работы без зависимостей)
const EMBEDDED_DUMMY_GAME: &[u8] = include_bytes!("../target/release/dummy_game.exe");

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
    app_id: String,
    #[allow(dead_code)]
    window_title: String,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum AppMode {
    SingleGame,  // 1: Запустить 1 игру через поиск
    RandomBatch, // 2: Случайные игры (Рандом)
    Settings,    // 3: Настройки
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum GameViewMode {
    Grid,
    List,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum GameSortMode {
    NameAsc,
    NameDesc,
    FavoritesFirst,
    DetectableExeFirst,
}

mod i18n;
use i18n::*;

mod discord_ipc;
use discord_ipc::DiscordIpc;

mod updater;
use updater::AutoUpdater;

mod quest_api;
mod anti_pause;
mod tray;
mod theme;
mod icon_cache;
mod ui;

const ICON_RGBA: &[u8] = include_bytes!("../app_icon_64.rgba");

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
    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
    #[serde(default)]
    pub farmed_history: Vec<String>,
    #[serde(default)]
    pub favorite_games: Vec<String>,
    #[serde(default)]
    pub random_only_favorites: bool,
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
            minimize_to_tray: true,
            farmed_history: Vec::new(),
            favorite_games: Vec::new(),
            random_only_favorites: false,
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

#[cfg(windows)]
pub(crate) fn is_window_visible_and_unminimized(main_hwnd: isize) -> bool {
    if main_hwnd == 0 {
        return false;
    }
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
    unsafe { IsWindowVisible(main_hwnd) != 0 && IsIconic(main_hwnd) == 0 }
}

#[cfg(not(windows))]
pub(crate) fn is_window_visible_and_unminimized(_main_hwnd: isize) -> bool {
    true
}

#[cfg(windows)]
pub(crate) fn wake_and_show_window(main_hwnd: isize, ctx: &eframe::egui::Context) {
    if main_hwnd != 0 {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            IsIconic, SetForegroundWindow, ShowWindowAsync, SW_RESTORE, SW_SHOW,
        };
        unsafe {
            let cmd = if IsIconic(main_hwnd) != 0 {
                SW_RESTORE
            } else {
                SW_SHOW
            };
            ShowWindowAsync(main_hwnd, cmd);
            SetForegroundWindow(main_hwnd);
        }
    }
    ctx.request_repaint();
}

#[cfg(not(windows))]
pub(crate) fn wake_and_show_window(_main_hwnd: isize, ctx: &eframe::egui::Context) {
    ctx.request_repaint();
}

#[cfg(windows)]
struct SingleInstanceGuard {
    mutex_handle: windows_sys::Win32::Foundation::HANDLE,
    show_event: windows_sys::Win32::Foundation::HANDLE,
    show_requested: Arc<AtomicBool>,
    stop_watcher: Arc<AtomicBool>,
    watcher_thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(windows)]
impl SingleInstanceGuard {
    fn acquire_or_signal_existing() -> Option<Self> {
        use std::ptr::null;
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
        use windows_sys::Win32::System::Threading::{
            CreateEventW, CreateMutexW, OpenEventW, SetEvent, EVENT_MODIFY_STATE,
        };

        let mutex_name: Vec<u16> = "Local\\DiscordGameFarmerSingleInstance\0".encode_utf16().collect();
        let event_name: Vec<u16> = "Local\\DiscordGameFarmerShowEvent\0".encode_utf16().collect();

        unsafe {
            let mutex = CreateMutexW(null(), 0, mutex_name.as_ptr());
            if mutex != 0 && GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(mutex);
                let ev = OpenEventW(EVENT_MODIFY_STATE, 0, event_name.as_ptr());
                if ev != 0 {
                    SetEvent(ev);
                    CloseHandle(ev);
                }
                return None;
            }
            let show_event = CreateEventW(null(), 0, 0, event_name.as_ptr());
            Some(Self {
                mutex_handle: mutex,
                show_event,
                show_requested: Arc::new(AtomicBool::new(false)),
                stop_watcher: Arc::new(AtomicBool::new(false)),
                watcher_thread: None,
            })
        }
    }

    fn start_watcher(&mut self, main_hwnd: isize, ctx: egui::Context) {
        if self.show_event == 0 || self.watcher_thread.is_some() {
            return;
        }
        let event_handle = self.show_event;
        let show_req = Arc::clone(&self.show_requested);
        let stop = Arc::clone(&self.stop_watcher);
        self.watcher_thread = std::thread::Builder::new()
            .name("single-instance-watcher".to_string())
            .spawn(move || {
                use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
                use windows_sys::Win32::System::Threading::{WaitForSingleObject, INFINITE};
                loop {
                    let wait = unsafe { WaitForSingleObject(event_handle, INFINITE) };
                    if stop.load(Ordering::Acquire) || wait != WAIT_OBJECT_0 {
                        break;
                    }
                    show_req.store(true, Ordering::Release);
                    wake_and_show_window(main_hwnd, &ctx);
                }
            })
            .ok();
    }

    fn poll_show_requested(&self) -> bool {
        if self.show_requested.swap(false, Ordering::AcqRel) {
            return true;
        }
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::WaitForSingleObject;

        if self.show_event == 0 {
            return false;
        }
        unsafe { WaitForSingleObject(self.show_event, 0) == WAIT_OBJECT_0 }
    }
}

#[cfg(windows)]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::SetEvent;
        self.stop_watcher.store(true, Ordering::Release);
        unsafe {
            if self.show_event != 0 {
                SetEvent(self.show_event);
            }
        }
        if let Some(handle) = self.watcher_thread.take() {
            let _ = handle.join();
        }
        unsafe {
            if self.show_event != 0 {
                CloseHandle(self.show_event);
            }
            if self.mutex_handle != 0 {
                CloseHandle(self.mutex_handle);
            }
        }
    }
}

#[cfg(not(windows))]
struct SingleInstanceGuard;

#[cfg(not(windows))]
impl SingleInstanceGuard {
    fn acquire_or_signal_existing() -> Option<Self> {
        Some(Self)
    }
    fn start_watcher(&mut self, _main_hwnd: isize, _ctx: egui::Context) {}
    fn poll_show_requested(&self) -> bool {
        false
    }
}

#[cfg(windows)]
struct ProcessJob {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl ProcessJob {
    fn new() -> Self {
        use std::ptr::null;
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let handle = CreateJobObjectW(null(), null());
            if handle != 0 {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
            }
            Self { handle }
        }
    }

    fn assign(&self, raw_handle: std::os::windows::io::RawHandle) {
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        if self.handle != 0 {
            unsafe {
                AssignProcessToJobObject(self.handle, raw_handle as _);
            }
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessJob {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        if self.handle != 0 {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(not(windows))]
struct ProcessJob;

#[cfg(not(windows))]
impl ProcessJob {
    fn new() -> Self {
        Self
    }
    #[allow(dead_code)]
    fn assign(&self, _raw_handle: std::os::windows::io::RawHandle) {}
}

struct RunningProcess {
    game: ActiveGame,
    child: Child,
    #[allow(dead_code)]
    stub_path: PathBuf,
}

struct GameFarmerApp {
    // Windows Job Object для гарантированного закрытия игр при выходе
    job_object: ProcessJob,

    // Настройки интерфейса
    language: Language,
    ui_scale: UiScale,

    // Режим работы
    app_mode: AppMode,
    prev_mode: AppMode,
    view_mode: GameViewMode,
    sort_mode: GameSortMode,
    show_logs_drawer: bool,

    // База игр и кэш иконок Discord
    all_games: Vec<DbGame>,
    search_query: String,
    last_search_query: String,
    filtered_indices: Vec<usize>,
    icon_cache: icon_cache::IconCache,

    // Время работы (в минутах)
    interval_minutes: u32,

    // Настройки для режима рандома
    batch_size: usize,
    exclude_running_pc_games: bool,
    farmed_history_names: HashSet<String>,
    favorite_games: HashSet<String>,
    random_only_favorites: bool,
    filter_only_favorites: bool,
    detected_pc_processes: HashSet<String>,

    // Текущее состояние работы
    is_running: bool,
    batch_started_at: Option<Instant>,
    total_seconds_farmed: u64,
    last_second_tick: Instant,

    // Список активных запущенных процессов
    running_processes: Vec<RunningProcess>,

    // Циклическая ротация Discord IPC внутри запущенной пачки
    current_ipc_index: usize,
    last_ipc_rotation: Instant,

    // Журнал
    logs: Vec<String>,

    // Сохраненное состояние настроек для автосохранения
    saved_settings: AppSettings,

    // IPC клиент Discord Rich Presence
    discord_ipc: DiscordIpc,

    // Автоматический Heartbeat для квестов Discord
    discord_token: Option<String>,
    quest_stop_signal: Option<Arc<AtomicBool>>,
    quest_log_sender: std::sync::mpsc::Sender<String>,
    quest_log_receiver: std::sync::mpsc::Receiver<String>,
    active_quest_status: Option<String>,
    anti_pause_active: bool,

    // Автообновление
    updater: AutoUpdater,

    // Системный трей и single-instance guard
    minimize_to_tray: bool,
    is_quitting: bool,
    is_hidden_to_tray: bool,
    main_hwnd: isize,
    tray_manager: Option<tray::TrayManager>,
    single_instance: Option<SingleInstanceGuard>,
}

impl Default for GameFarmerApp {
    fn default() -> Self {
        AutoUpdater::cleanup_previous_update();
        let _ = Self::find_dummy_executable();
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

        let (quest_log_sender, quest_log_receiver) = std::sync::mpsc::channel();

        Self {
            job_object: ProcessJob::new(),
            language: loaded_settings.language,
            ui_scale: loaded_settings.ui_scale,
            app_mode: AppMode::SingleGame,
            prev_mode: AppMode::SingleGame,
            view_mode: GameViewMode::Grid,
            sort_mode: GameSortMode::NameAsc,
            show_logs_drawer: false,
            all_games,
            search_query: String::new(),
            last_search_query: String::new(),
            filtered_indices: Vec::new(),
            icon_cache: icon_cache::IconCache::new(),
            interval_minutes: loaded_settings.interval_minutes,
            batch_size: loaded_settings.batch_size,
            exclude_running_pc_games: loaded_settings.exclude_running_pc_games,
            farmed_history_names: loaded_settings.farmed_history.iter().cloned().collect(),
            favorite_games: loaded_settings.favorite_games.iter().cloned().collect(),
            random_only_favorites: loaded_settings.random_only_favorites,
            filter_only_favorites: false,
            detected_pc_processes: HashSet::new(),
            is_running: false,
            batch_started_at: None,
            total_seconds_farmed: 0,
            last_second_tick: Instant::now(),
            running_processes: Vec::new(),
            current_ipc_index: 0,
            last_ipc_rotation: Instant::now(),
            logs,
            minimize_to_tray: loaded_settings.minimize_to_tray,
            is_quitting: false,
            is_hidden_to_tray: false,
            main_hwnd: 0,
            tray_manager: None,
            single_instance: None,
            discord_ipc: DiscordIpc::new(),
            discord_token: quest_api::extract_discord_token(),
            quest_stop_signal: None,
            quest_log_sender,
            quest_log_receiver,
            active_quest_status: None,
            anti_pause_active: anti_pause::is_anti_pause_installed(),
            updater,
            saved_settings: loaded_settings,
        }
    }
}

impl Drop for GameFarmerApp {
    fn drop(&mut self) {
        self.stop_all_active_games();
        self.discord_ipc.clear_activity();
        self.saved_settings.save();
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
            "where winds meet", "wwm", "basketball null", "basketball", "firefox browser", "firefox",
            "99 nights in the forest", "doors on roblox", "clean crew on roblox", "illegal soccer on roblox", "illegal soccer", "robloxplayerbeta",
            "kobayashi", "magic: the gathering", "mtga",
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

    if q.contains("баскет") || q.contains("basket") {
        if name_lower.contains("basketball") {
            return true;
        }
    }

    if q.contains("soccer") || q.contains("соккер") || q.contains("illegal") || q.contains("иллегал") || q.contains("футбол") {
        if name_lower.contains("illegal soccer") {
            return true;
        }
    }

    if q.contains("roblox") || q.contains("роблокс") {
        if name_lower.contains("roblox") || exe_lower.contains("roblox") {
            return true;
        }
    }

    if q == "лиса" || q.contains("фаерфокс") || q.contains("файрфокс") || q.contains("firefox") {
        if name_lower.contains("firefox") || exe_lower.contains("firefox") {
            return true;
        }
    }

    if q.contains("koba") || q.contains("коба") || q.contains("maru") || q.contains("мару")
        || q.contains("star trek") || q.contains("стартрек") || q.contains("стар трек")
        || q.contains("trek") || q.contains("трек")
        || q == "mtg" || q == "мтг" || q.contains("magic") || q.contains("мэджик")
    {
        if name_lower.contains("kobayashi") || name_lower.contains("magic: the gathering") || exe_lower.contains("mtga") {
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
    if q.starts_with("гта") || q.starts_with("gta") {
        if name_lower.contains("grand theft auto") || exe_lower.contains("gta") {
            let digits: String = q.chars().filter(|c| c.is_numeric()).collect();
            if digits == "6" || q.ends_with("vi") {
                return name_lower.contains(" vi") || name_lower.ends_with("vi") || exe_lower.contains("gta6");
            }
            if digits == "5" || q.ends_with(" v") {
                return name_lower.contains(" v") || exe_lower.contains("gta5") || name_lower.contains("online");
            }
            if digits == "4" || q.ends_with("iv") {
                return name_lower.contains(" iv") || exe_lower.contains("gta4") || exe_lower.contains("gtaiv");
            }
            if digits == "3" || q.ends_with("iii") {
                return name_lower.contains(" iii") || exe_lower.contains("gta3");
            }
            if !digits.is_empty() {
                return name_lower.contains(&digits) || exe_lower.contains(&digits);
            }
            return true;
        }
    }
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
    pub fn init_tray(&mut self, ctx: egui::Context, main_hwnd: isize) {
        self.main_hwnd = main_hwnd;
        let ipc_for_quit = self.discord_ipc.clone();
        let on_hidden_quit = Arc::new(move || {
            ipc_for_quit.clear_activity_and_wait(Duration::from_millis(300));
            std::process::exit(0);
        });
        match tray::TrayManager::new(ICON_RGBA, 64, 64, ctx, main_hwnd, on_hidden_quit) {
            Ok(tm) => {
                self.tray_manager = Some(tm);
            }
            Err(e) => {
                self.add_log(&format!("⚠️ Системный трей: {}", e));
            }
        }
    }

    pub(crate) fn hide_to_tray(&mut self, ctx: &egui::Context) {
        self.is_hidden_to_tray = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    pub(crate) fn show_from_tray(&mut self, ctx: &egui::Context) {
        self.is_hidden_to_tray = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// Передаёт процессу-заглушке путь к PNG иконке игры (и запасную палитру) для окна в панели задач.
    fn configure_stub_icon_env(&mut self, cmd: &mut Command, game_name: &str, app_id: &str) {
        if !app_id.is_empty() {
            self.icon_cache.ensure_requested(app_id);
            cmd.env("DGF_ICON_PNG", self.icon_cache.png_path(app_id));
        }
        let (bg, fg) = theme::monogram_palette(game_name);
        cmd.env("DGF_ICON_BG", format!("{},{},{}", bg.r(), bg.g(), bg.b()));
        cmd.env("DGF_ICON_FG", format!("{},{},{}", fg.r(), fg.g(), fg.b()));
    }

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
                    // Синхронизируем локальный dummy_game.exe с вшитым при обновлении portable-версии
                    if std::fs::read(&candidate).ok().as_deref() != Some(EMBEDDED_DUMMY_GAME) {
                        let _ = std::fs::write(&candidate, EMBEDDED_DUMMY_GAME);
                    }
                    return Some(candidate);
                }
                // Автоматическое самоизвлечение встроенного dummy_game.exe
                if std::fs::write(&candidate, EMBEDDED_DUMMY_GAME).is_ok() {
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

    fn clean_exe_name(exe: &str) -> String {
        exe.trim().replace(['<', '>'], "")
    }

    /// Проверяет, что запись указывает на исполняемый .exe, распознаваемый Discord RunningGameStore.
    fn is_detectable_win_exe(exe: &str) -> bool {
        let clean = Self::clean_exe_name(exe);
        !clean.is_empty() && clean.to_lowercase().ends_with(".exe")
    }

    /// Всегда хранит stubs рядом с исполняемым файлом, независимо от текущей рабочей директории.
    fn resolve_stubs_dir() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("stubs")))
            .unwrap_or_else(|| PathBuf::from("stubs"))
    }

    /// Проверяет побайтовое совпадение двух файлов (при равном размере).
    fn files_identical(a: &Path, b: &Path) -> bool {
        let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
            return false;
        };
        if ma.len() != mb.len() {
            return false;
        }
        matches!((std::fs::read(a), std::fs::read(b)), (Ok(ba), Ok(bb)) if ba == bb)
    }

    /// Копирует `dummy_exe` в `stub_path`, если заглушка отсутствует или отличается от актуального билда.
    fn sync_stub_file(dummy_exe: &Path, stub_path: &Path) -> bool {
        if let Some(parent) = stub_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if Self::files_identical(stub_path, dummy_exe) {
            return false;
        }
        std::fs::copy(dummy_exe, stub_path).is_ok()
    }

    /// Готовит копию dummy_game.exe с именем целевого Win32 .exe игры; иначе возвращает dummy_exe.
    fn prepare_game_stub(dummy_exe: &Path, clean_exe: &str) -> PathBuf {
        if !Self::is_detectable_win_exe(clean_exe) {
            return dummy_exe.to_path_buf();
        }
        let stubs_dir = Self::resolve_stubs_dir();
        let stub_path = stubs_dir.join(clean_exe);
        let _ = Self::sync_stub_file(dummy_exe, &stub_path);
        stub_path
    }

    /// Подбирает кандидатов для пачки: только игры с валидным App ID и Win32 .exe.
    fn batch_candidates(&self, exclude_names: Option<&HashSet<String>>) -> (Vec<DbGame>, Vec<String>) {
        let mut excluded_on_pc = Vec::new();
        let candidates = self
            .all_games
            .iter()
            .filter(|g| exclude_names.map_or(true, |set| !set.contains(&g.name)))
            .filter(|g| {
                if self.random_only_favorites
                    && !self.favorite_games.is_empty()
                    && !self.favorite_games.contains(&g.name)
                {
                    return false;
                }
                if g.id.is_empty() || !Self::is_detectable_win_exe(&g.exe) {
                    return false;
                }
                let clean = Self::clean_exe_name(&g.exe);
                let exe_lower = Path::new(&clean)
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or(&clean)
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
        (candidates, excluded_on_pc)
    }

    /// Остановка ВСЕХ запущенных процессов без блокировки UI-потока
    fn stop_all_active_games(&mut self) {
        self.active_quest_status = None;
        if let Some(sig) = self.quest_stop_signal.take() {
            sig.store(true, Ordering::Relaxed);
        }
        let mut drained: Vec<RunningProcess> = self.running_processes.drain(..).collect();
        for rp in &mut drained {
            let _ = rp.child.kill();
        }
        if !drained.is_empty() {
            std::thread::spawn(move || {
                for mut rp in drained {
                    let _ = rp.child.wait();
                }
            });
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

        // 1. Запускаем stub-процесс (для Windows-хуков Discord и прохождения квестов)
        let dummy_exe = match Self::find_dummy_executable() {
            Some(p) => p,
            None => {
                if !app_id.is_empty() {
                    self.discord_ipc.set_activity(&app_id, &clean_title, "In Game", None);
                }
                self.add_log("Внимание: dummy_game.exe не найден (активность установлена через Discord IPC)");
                self.is_running = true;
                self.batch_started_at = Some(Instant::now());
                return;
            }
        };

        let clean_exe = Self::clean_exe_name(&exe);
        let target_stub = Self::prepare_game_stub(&dummy_exe, &clean_exe);
        if !Self::is_detectable_win_exe(&clean_exe) {
            self.add_log(&format!(
                "Внимание: у '{}' нет Win32 .exe ('{}') — Discord увидит только IPC статус, но не процесс для значка/квеста.",
                name, clean_exe
            ));
        }

        let mut cmd = Command::new(&target_stub);
        if let Some(parent) = target_stub.parent() {
            cmd.current_dir(parent);
        }
        self.configure_stub_icon_env(&mut cmd, &clean_title, &app_id);
        cmd.arg(&clean_title);

        match cmd.spawn() {
            Ok(child) => {
                #[cfg(windows)]
                {
                    use std::os::windows::io::AsRawHandle;
                    self.job_object.assign(child.as_raw_handle());
                }
                let child_pid = child.id();
                if !app_id.is_empty() {
                    self.discord_ipc.set_activity(&app_id, &clean_title, "In Game", Some(child_pid));
                }
                self.running_processes.push(RunningProcess {
                    game: ActiveGame {
                        name: clean_title.clone(),
                        exe_name: clean_exe.clone(),
                        app_id: app_id.clone(),
                        window_title: clean_title.clone(),
                    },
                    child,
                    stub_path: target_stub,
                });
                self.is_running = true;
                self.batch_started_at = Some(Instant::now());
                self.add_log(&format!(
                    "Запущена игра '{}' на {} мин. (Процесс [PID {}] запущен скрытно в фоне, 0% CPU)",
                    name, self.interval_minutes, child_pid
                ));

                // 2. Асинхронная проверка и запуск Heartbeat-воркера квеста в фоне (без задержки UI)
                if !app_id.is_empty() {
                    let cached_token = self.discord_token.clone();
                    let stop_sig = Arc::new(AtomicBool::new(false));
                    self.quest_stop_signal = Some(Arc::clone(&stop_sig));
                    let log_tx = self.quest_log_sender.clone();
                    std::thread::spawn(move || {
                        let Some(token) = cached_token.or_else(quest_api::extract_discord_token) else {
                            return;
                        };
                        if stop_sig.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Some(quest) = quest_api::get_enrolled_quest(&token, &app_id, &clean_title) {
                            if stop_sig.load(Ordering::Relaxed) {
                                return;
                            }
                            if quest.completed {
                                let _ = log_tx.send(format!(
                                    "✨ Квест Discord '{}' уже завершен на 100%! Заберите награду в Discord.",
                                    quest.quest_name
                                ));
                            } else {
                                let _ = log_tx.send(format!(
                                    "🎯 Квест Discord '{}' активен! Включена авто-отправка прогресса (Heartbeat API, текущий: {}/{} сек).",
                                    quest.quest_name, quest.current_seconds, quest.target_seconds
                                ));
                                let handle = quest_api::spawn_quest_heartbeat_worker(
                                    token,
                                    quest,
                                    stop_sig,
                                    log_tx,
                                );
                                let _ = handle.join();
                            }
                        } else if let Some(un_quest_name) = quest_api::get_unenrolled_quest_name(&token, &app_id, &clean_title) {
                            if !stop_sig.load(Ordering::Relaxed) {
                                let _ = log_tx.send(format!(
                                    "⚠️ Внимание: Для игры '{}' есть квест в Discord ('{}'), но он еще не принят! Нажмите «Принять задание» в Discord и перезапустите таймер.",
                                    clean_title, un_quest_name
                                ));
                            }
                        }
                    });
                }
            }
            Err(e) => {
                if !app_id.is_empty() {
                    self.discord_ipc.set_activity(&app_id, &clean_title, "In Game", None);
                }
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

        let (candidates, excluded_on_pc) = self.batch_candidates(None);

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
            if self.random_only_favorites && !self.favorite_games.is_empty() {
                self.add_log("В избранном нет игр с .exe из базы Discord (без .exe значок Discord не засчитывает).");
            } else {
                self.add_log("Не найдено подходящих игр с .exe для запуска.");
            }
            self.is_running = false;
            return;
        }

        let mut launched_names = Vec::new();
        let mut first_game_id = None;
        let mut first_game_name = None;

        for db_game in picked {
            self.farmed_history_names.insert(db_game.name.clone());
            if first_game_id.is_none() && !db_game.id.is_empty() {
                first_game_id = Some(db_game.id.clone());
                first_game_name = Some(db_game.name.clone());
            }

            let clean_exe = Self::clean_exe_name(&db_game.exe);
            let target_stub = Self::prepare_game_stub(&dummy_exe, &clean_exe);

            let mut cmd = Command::new(&target_stub);
            if let Some(parent) = target_stub.parent() {
                cmd.current_dir(parent);
            }
            self.configure_stub_icon_env(&mut cmd, &db_game.name, &db_game.id);
            cmd.arg(&db_game.name);

            match cmd.spawn() {
                Ok(child) => {
                    #[cfg(windows)]
                    {
                        use std::os::windows::io::AsRawHandle;
                        self.job_object.assign(child.as_raw_handle());
                    }
                    launched_names.push(db_game.name.clone());
                    self.running_processes.push(RunningProcess {
                        game: ActiveGame {
                            name: db_game.name.clone(),
                            exe_name: clean_exe,
                            app_id: db_game.id.clone(),
                            window_title: db_game.name,
                        },
                        child,
                        stub_path: target_stub,
                    });
                }
                Err(e) => {
                    self.add_log(&format!("Ошибка запуска '{}' ({}): {}", db_game.name, clean_exe, e));
                }
            }
        }

        if self.interval_minutes < 6 {
            self.add_log("Внимание: Discord отправляет heartbeat раз в 5 мин — интервал < 6 мин может не засчитаться в значок.");
        }

        self.current_ipc_index = 0;
        self.last_ipc_rotation = Instant::now();

        if let (Some(app_id), Some(game_name)) = (first_game_id, first_game_name) {
            let first_pid = self.running_processes.first().map(|rp| rp.child.id());
            self.discord_ipc.set_activity(&app_id, &game_name, "In Game", first_pid);
        }

        self.is_running = true;
        self.batch_started_at = Some(Instant::now());
        self.add_log(&format!(
            "Запущен рандом: {} игр параллельно на {} мин. Все процессы запущены скрытно в фоне (0% CPU, проверить можно в Диспетчере задач)",
            self.running_processes.len(),
            self.interval_minutes
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

    /// Замена одной конкретной игры в запущенной пачке
    fn replace_batch_game(&mut self, idx: usize) {
        if idx >= self.running_processes.len() {
            return;
        }

        let mut old_proc = self.running_processes.remove(idx);
        let old_name = old_proc.game.name.clone();
        let _ = old_proc.child.kill();
        let _ = old_proc.child.wait();

        let dummy_exe = match Self::find_dummy_executable() {
            Some(p) => p,
            None => return,
        };

        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();

        let running_names: HashSet<String> = self.running_processes.iter().map(|rp| rp.game.name.clone()).collect();
        let (candidates, _) = self.batch_candidates(Some(&running_names));

        if let Some(new_game) = candidates.choose(&mut rng) {
            let clean_exe = Self::clean_exe_name(&new_game.exe);
            let target_stub = Self::prepare_game_stub(&dummy_exe, &clean_exe);

            let mut cmd = Command::new(&target_stub);
            if let Some(parent) = target_stub.parent() {
                cmd.current_dir(parent);
            }
            cmd.arg(&new_game.name);

            if let Ok(new_child) = cmd.spawn() {
                #[cfg(windows)]
                {
                    use std::os::windows::io::AsRawHandle;
                    self.job_object.assign(new_child.as_raw_handle());
                }
                let new_pid = new_child.id();
                self.farmed_history_names.insert(new_game.name.clone());
                self.running_processes.insert(idx, RunningProcess {
                    game: ActiveGame {
                        name: new_game.name.clone(),
                        exe_name: clean_exe,
                        app_id: new_game.id.clone(),
                        window_title: new_game.name.clone(),
                    },
                    child: new_child,
                    stub_path: target_stub,
                });

                self.add_log(&format!("Игра '{}' заменена на '{}' (PID {})", old_name, new_game.name, new_pid));

                if idx == self.current_ipc_index && !new_game.id.is_empty() {
                    self.discord_ipc.set_activity(&new_game.id, &new_game.name, "In Game", Some(new_pid));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_stub_file_refreshes_same_size_stale_stub() {
        let dir = std::env::temp_dir().join(format!("dgf_stub_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let dummy = dir.join("dummy_game.exe");
        let stub = dir.join("sub/hero_siege.exe");
        std::fs::write(&dummy, b"NEW_STUB_DATA_V2").unwrap();
        std::fs::create_dir_all(stub.parent().unwrap()).unwrap();
        std::fs::write(&stub, b"OLD_STUB_DATA_V1").unwrap();

        // Same 16-byte length, different bytes: must overwrite
        assert!(GameFarmerApp::sync_stub_file(&dummy, &stub));
        assert_eq!(std::fs::read(&stub).unwrap(), b"NEW_STUB_DATA_V2");

        // Identical content: no redundant copy
        assert!(!GameFarmerApp::sync_stub_file(&dummy, &stub));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detectable_win_exe_filter() {
        assert!(GameFarmerApp::is_detectable_win_exe("hero_siege.exe"));
        assert!(GameFarmerApp::is_detectable_win_exe("bin/x64/cyberpunk2077.exe"));
        assert!(!GameFarmerApp::is_detectable_win_exe(""));
        assert!(!GameFarmerApp::is_detectable_win_exe("game.app/Contents/MacOS/game"));
        assert!(GameFarmerApp::is_detectable_win_exe("gta6.exe"));
    }

    #[test]
    fn gta_queries_matching() {
        assert!(game_matches_query("Grand Theft Auto VI", "gta6.exe", "гта 6"));
        assert!(game_matches_query("Grand Theft Auto VI", "gta6.exe", "гта6"));
        assert!(game_matches_query("Grand Theft Auto VI", "gta6.exe", "gta 6"));
        assert!(game_matches_query("Grand Theft Auto VI", "gta6.exe", "gta6"));
        assert!(game_matches_query("Grand Theft Auto VI", "gta6.exe", "gta vi"));
        assert!(!game_matches_query("Grand Theft Auto V Legacy", "gta5.exe", "гта 6"));
        assert!(!game_matches_query("Grand Theft Auto V Legacy", "gta5.exe", "gta 6"));
        assert!(game_matches_query("Grand Theft Auto V Legacy", "gta5.exe", "гта 5"));
        assert!(game_matches_query("Grand Theft Auto VI", "gta6.exe", "гта"));
        assert!(game_matches_query("Grand Theft Auto VI", "gta6.exe", "gta"));
    }

    #[test]
    fn custom_fonts_support_all_ui_emojis() {
        let fonts = egui::epaint::text::Fonts::new(1.0, 2048, build_custom_font_definitions());
        let mut lock = fonts.lock();
        let samples = [
            "🎮", "🎯", "✨", "⚠️", "⏹", "★", "☆", "●", "○", "💥", "⚡", "📦", "🎨",
            "↻", "☰", "⊞", "❐", "□", "✕", "⚙", "▲", "▼", "↗", "←", "■", "—",
        ];
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let font_id = egui::FontId::new(14.0, family.clone());
            for sample in samples {
                assert!(
                    lock.fonts.has_glyphs(&font_id, sample),
                    "Шрифт {:?} не поддерживает символ {:?}",
                    family,
                    sample
                );
            }
        }
    }
}

impl eframe::App for GameFarmerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Обработка закрытия окна (сворачивание в трей при клике на крестик)
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.minimize_to_tray && !self.is_quitting {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide_to_tray(ctx);
            }
        }

        // Опрос событий системного трея
        if let Some(tray) = &self.tray_manager {
            let tooltip = if self.is_running {
                if let Some(first_rp) = self.running_processes.first() {
                    format!("Discord Farmer: {} (активно: {})", first_rp.game.name, self.running_processes.len())
                } else {
                    "Discord Game Farmer (Активен)".to_string()
                }
            } else {
                "Discord Game Activity Manager (Остановлен)".to_string()
            };
            tray.set_tooltip(&tooltip);

            if let Some(action) = tray.update() {
                match action {
                    tray::TrayAction::Show => {
                        self.show_from_tray(ctx);
                    }
                    tray::TrayAction::Hide => {
                        self.hide_to_tray(ctx);
                    }
                    tray::TrayAction::NextGame => {
                        if self.app_mode == AppMode::RandomBatch {
                            self.launch_random_batch();
                        } else {
                            let candidates: Vec<&DbGame> = self.all_games.iter()
                                .filter(|g| !self.farmed_history_names.contains(&g.name))
                                .collect();
                            let chosen = if !candidates.is_empty() {
                                let mut rng = rand::thread_rng();
                                candidates[rand::Rng::gen_range(&mut rng, 0..candidates.len())]
                            } else if !self.all_games.is_empty() {
                                let mut rng = rand::thread_rng();
                                &self.all_games[rand::Rng::gen_range(&mut rng, 0..self.all_games.len())]
                            } else {
                                return;
                            };
                            self.launch_single_game(chosen.name.clone(), chosen.exe.clone(), chosen.id.clone());
                        }
                    }
                    tray::TrayAction::StopGames => {
                        self.stop_farming();
                    }
                    tray::TrayAction::Quit => {
                        self.is_quitting = true;
                        self.stop_all_active_games();
                        self.discord_ipc.clear_activity_and_wait(Duration::from_millis(300));
                        self.saved_settings.save();
                        self.tray_manager = None;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        return;
                    }
                }
            }
        }

        // Повторный запуск exe выводит уже открытый экземпляр вместо дубля в трее
        let show_from_second_instance = self
            .single_instance
            .as_ref()
            .is_some_and(|guard| guard.poll_show_requested());
        if show_from_second_instance {
            self.show_from_tray(ctx);
        }

        // Прием логов из фонового Heartbeat API воркера квестов
        while let Ok(msg) = self.quest_log_receiver.try_recv() {
            self.active_quest_status = Some(msg.clone());
            self.add_log(&msg);
        }

        // Опрос статуса автообновления
        let update_outcome = self.updater.poll();
        if let Some(log_msg) = update_outcome.log {
            self.add_log(&log_msg);
        }
        if update_outcome.restart_ready {
            self.is_quitting = true;
            self.stop_all_active_games();
            self.discord_ipc.clear_activity_and_wait(Duration::from_millis(300));
            self.saved_settings.save();
            self.tray_manager = None;
            self.single_instance = None;
            std::process::exit(0);
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

            // Динамическая ротация Discord IPC внутри пачки каждые 2 минуты
            if self.app_mode == AppMode::RandomBatch && !self.running_processes.is_empty() {
                if self.last_ipc_rotation.elapsed() >= Duration::from_secs(120) {
                    self.last_ipc_rotation = Instant::now();
                    self.current_ipc_index = (self.current_ipc_index + 1) % self.running_processes.len();
                    let rp = &self.running_processes[self.current_ipc_index];
                    if !rp.game.app_id.is_empty() {
                        let pid = rp.child.id();
                        self.discord_ipc.set_activity(&rp.game.app_id, &rp.game.name, "In Game", Some(pid));
                        self.add_log(&format!("Discord статус переключен: '{}' (PID {})", rp.game.name, pid));
                    }
                }
            }
        }

        self.render_artcraft_ui(ctx);

        // Автоматическое сохранение настроек при их изменении
        let current_settings = AppSettings {
            language: self.language,
            ui_scale: self.ui_scale,
            interval_minutes: self.interval_minutes,
            batch_size: self.batch_size,
            exclude_running_pc_games: self.exclude_running_pc_games,
            minimize_to_tray: self.minimize_to_tray,
            farmed_history: self.farmed_history_names.iter().cloned().collect(),
            favorite_games: self.favorite_games.iter().cloned().collect(),
            random_only_favorites: self.random_only_favorites,
        };
        if current_settings != self.saved_settings {
            current_settings.save();
            self.saved_settings = current_settings;
        }

        // Не запрашиваем периодическую перерисовку, когда окно скрыто в трей:
        // на Windows скрытый HWND не получает WM_PAINT, и таймер зациклит winit в ControlFlow::Poll.
        if !self.is_hidden_to_tray {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_all_active_games();
        self.discord_ipc.clear_activity();
        self.saved_settings.save();
        self.tray_manager = None;
    }
}

fn build_custom_font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();

    // Порядок: основной UI-шрифт, затем Windows Emoji (включая U+FE0F для составных эмодзи) и Symbol.
    let system_fonts: [(&str, &str, bool); 3] = [
        ("segoe_ui", r"C:\Windows\Fonts\segoeui.ttf", false),
        ("segoe_emj", r"C:\Windows\Fonts\seguiemj.ttf", true),
        ("segoe_sym", r"C:\Windows\Fonts\seguisym.ttf", true),
    ];

    let mut prop_idx = 0;
    let mut mono_idx = fonts
        .families
        .get(&egui::FontFamily::Monospace)
        .map_or(0, |list| usize::from(!list.is_empty()));

    for (name, path, add_to_monospace) in system_fonts {
        if let Ok(data) = std::fs::read(path) {
            fonts
                .font_data
                .insert(name.to_owned(), egui::FontData::from_owned(data));

            let proportional = fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default();
            proportional.insert(prop_idx.min(proportional.len()), name.to_owned());
            prop_idx += 1;

            if add_to_monospace {
                let monospace = fonts
                    .families
                    .entry(egui::FontFamily::Monospace)
                    .or_default();
                monospace.insert(mono_idx.min(monospace.len()), name.to_owned());
                mono_idx += 1;
            }
        }
    }

    fonts
}

fn setup_custom_fonts(ctx: &egui::Context) {
    ctx.set_fonts(build_custom_font_definitions());
}

fn main() -> eframe::Result<()> {
    if updater::handle_legacy_temp_updater_bootstrap(EMBEDDED_DUMMY_GAME) {
        return Ok(());
    }
    let Some(instance_guard) = SingleInstanceGuard::acquire_or_signal_existing() else {
        return Ok(());
    };
    let autostart = std::env::args().any(|a| a == "--autostart");
    let icon_data = egui::IconData {
        rgba: ICON_RGBA.to_vec(),
        width: 64,
        height: 64,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_inner_size([1160.0, 860.0])
            .with_min_inner_size([620.0, 480.0])
            .with_title("Discord Game Activity Manager")
            .with_icon(icon_data)
            .with_resizable(true),
        ..Default::default()
    };

    let mut instance_guard = Some(instance_guard);
    eframe::run_native(
        "Discord Game Activity Manager",
        options,
        Box::new(move |cc| {
            setup_custom_fonts(&cc.egui_ctx);
            theme::apply_artcraft_visuals(&cc.egui_ctx);
            #[cfg(windows)]
            let main_hwnd = {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                cc.window_handle()
                    .ok()
                    .and_then(|h| match h.as_raw() {
                        RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
                        _ => None,
                    })
                    .unwrap_or(0)
            };
            #[cfg(not(windows))]
            let main_hwnd = 0isize;

            let mut app = GameFarmerApp::default();
            if let Some(mut guard) = instance_guard.take() {
                guard.start_watcher(main_hwnd, cc.egui_ctx.clone());
                app.single_instance = Some(guard);
            }
            app.init_tray(cc.egui_ctx.clone(), main_hwnd);
            if autostart {
                app.launch_random_batch();
            }
            Ok(Box::new(app))
        }),
    )
}
