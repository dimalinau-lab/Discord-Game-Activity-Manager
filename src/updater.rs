use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use serde::Deserialize;

const GITHUB_REPO: &str = "dimalinau-lab/Discord-Game-Activity-Manager";
const USER_AGENT: &str = "DiscordGameActivityManager";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateMode {
    DirectSelfReplace,
    Installer,
}

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
pub struct GitHubAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    #[serde(default)]
    pub assets: Vec<GitHubAsset>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        changelog: String,
        download_url: String,
    },
    Downloading {
        version: String,
        progress: f32, // 0.0 .. 1.0
        downloaded: u64,
        total: u64,
    },
    Installing,
    Error(String),
}

#[allow(dead_code)]
pub enum UpdaterMessage {
    CheckResult(Result<Option<(String, String, String)>, String>),
    DownloadProgress { version: String, downloaded: u64, total: u64 },
    DownloadFinished { version: String, installer_path: PathBuf },
    DownloadError(String),
}

pub struct PollOutcome {
    pub log: Option<String>,
    pub restart_ready: bool,
}

pub struct AutoUpdater {
    pub current_version: String,
    pub status: UpdateStatus,
    sender: Sender<UpdaterMessage>,
    receiver: Receiver<UpdaterMessage>,
    pub is_checking: bool,
    pub is_downloading: bool,
}

pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let clean = v.trim().trim_start_matches('v');
    let parts: Vec<&str> = clean.split('.').collect();
    if parts.len() >= 2 {
        let major = parts[0].parse::<u32>().ok()?;
        let minor = parts[1].parse::<u32>().ok()?;
        let patch = if parts.len() >= 3 {
            parts[2].split(|c: char| !c.is_numeric()).next()?.parse::<u32>().ok()?
        } else {
            0
        };
        Some((major, minor, patch))
    } else {
        None
    }
}

pub fn is_newer_version(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(c), Some(cur)) => c > cur,
        _ => candidate.trim() != current.trim(),
    }
}

/// Проверяет, доступна ли папка с текущим .exe для записи (чтобы заменить файл ярлыка напрямую).
pub fn is_dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".dgf_write_probe_{}", std::process::id()));
    match File::create(&probe) {
        Ok(f) => {
            drop(f);
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Для любой доступной на запись папки (включая установленную) обновляем сам exe на месте,
/// чтобы 100% заменить файл, на который указывает ярлык; Setup.exe используется только как fallback для read-only папок.
pub fn detect_update_mode(exe_path: &Path) -> UpdateMode {
    if let Some(parent) = exe_path.parent() {
        if is_dir_writable(parent) {
            return UpdateMode::DirectSelfReplace;
        }
    }
    UpdateMode::Installer
}

fn current_update_mode() -> UpdateMode {
    std::env::current_exe()
        .map(|p| detect_update_mode(&p))
        .unwrap_or(UpdateMode::Installer)
}

/// Строгий выбор ассета: прямой бинарник `discord-game-farmer.exe` или инсталлятор `*_Setup.exe`.
pub fn select_release_asset(assets: &[GitHubAsset], mode: UpdateMode) -> Option<&GitHubAsset> {
    match mode {
        UpdateMode::DirectSelfReplace => assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case("discord-game-farmer.exe"))
            .or_else(|| {
                assets.iter().find(|a| {
                    let lower = a.name.to_ascii_lowercase();
                    lower.ends_with(".exe") && !lower.contains("setup")
                })
            }),
        UpdateMode::Installer => assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case("DiscordGameFarmer_Setup.exe"))
            .or_else(|| {
                assets.iter().find(|a| {
                    let lower = a.name.to_ascii_lowercase();
                    lower.ends_with("_setup.exe") || lower.ends_with("setup.exe")
                })
            }),
    }
}

fn is_valid_pe_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() < 100_000 {
        return false;
    }
    let Ok(mut f) = File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 2];
    f.read_exact(&mut magic).is_ok() && magic == *b"MZ"
}

fn ps_single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

impl AutoUpdater {
    pub fn new() -> Self {
        let (sender, receiver) = channel();
        Self {
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            status: UpdateStatus::Idle,
            sender,
            receiver,
            is_checking: false,
            is_downloading: false,
        }
    }

    /// Очистка временных файлов (.exe.old / .exe.new) и старых инсталляторов из TEMP
    pub fn cleanup_previous_update() {
        if let Ok(current_exe) = std::env::current_exe() {
            if let Some(parent) = current_exe.parent() {
                if let Ok(entries) = std::fs::read_dir(parent) {
                    let stem = current_exe
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("discord-game-farmer")
                        .to_ascii_lowercase();
                    for entry in entries.flatten() {
                        let path = entry.path();
                        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                        if name.starts_with(&stem) && (name.ends_with(".old") || name.ends_with(".new")) {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                }
            }
        }
        if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if name.starts_with("discordgamefarmer_setup_v") && name.ends_with(".exe") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    /// Асинхронная проверка релизов на GitHub через Releases API
    pub fn check_github_update(&mut self) {
        if self.is_checking || self.is_downloading {
            return;
        }

        self.is_checking = true;
        self.status = UpdateStatus::Checking;

        let tx = self.sender.clone();
        let cur_version = self.current_version.clone();
        let mode = current_update_mode();

        thread::spawn(move || {
            let url = format!("https://api.github.com/repos/{}/releases/latest", GITHUB_REPO);
            let resp = ureq::get(&url)
                .set("User-Agent", USER_AGENT)
                .set("Accept", "application/vnd.github+json")
                .timeout(std::time::Duration::from_secs(10))
                .call();

            let response = match resp {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(UpdaterMessage::CheckResult(Err(format!("Ошибка сети: {}", e))));
                    return;
                }
            };

            let release: GitHubRelease = match response.into_json() {
                Ok(rel) => rel,
                Err(e) => {
                    let _ = tx.send(UpdaterMessage::CheckResult(Err(format!("Ошибка JSON: {}", e))));
                    return;
                }
            };

            if is_newer_version(&release.tag_name, &cur_version) {
                if let Some(asset) = select_release_asset(&release.assets, mode) {
                    let changelog = release
                        .body
                        .as_deref()
                        .unwrap_or_default()
                        .lines()
                        .find(|l| {
                            let t = l.trim().trim_start_matches('#').trim();
                            !t.is_empty() && !t.starts_with("---")
                        })
                        .map(|l| l.trim().trim_start_matches('#').trim().to_string())
                        .unwrap_or_else(|| format!("Доступно обновление {}", release.tag_name));

                    let _ = tx.send(UpdaterMessage::CheckResult(Ok(Some((
                        release.tag_name,
                        changelog,
                        asset.browser_download_url.clone(),
                    )))));
                } else {
                    let _ = tx.send(UpdaterMessage::CheckResult(Err(
                        "Файл обновления не найден в релизе".to_string(),
                    )));
                }
            } else {
                let _ = tx.send(UpdaterMessage::CheckResult(Ok(None)));
            }
        });
    }

    /// Запуск скачивания обновления с отображением прогресса и автоперезапуском
    pub fn start_download_and_install(&mut self, download_url: String, version: String) {
        if self.is_downloading {
            return;
        }

        self.is_downloading = true;
        self.status = UpdateStatus::Downloading {
            version: version.clone(),
            progress: 0.0,
            downloaded: 0,
            total: 0,
        };

        let tx = self.sender.clone();
        let mode = current_update_mode();

        thread::spawn(move || {
            let clean_ver = version.trim_start_matches('v');
            let target_path = match mode {
                UpdateMode::DirectSelfReplace => match std::env::current_exe() {
                    Ok(cur) => cur.with_extension("exe.new"),
                    Err(e) => {
                        let _ = tx.send(UpdaterMessage::DownloadError(format!(
                            "Не удалось определить путь exe: {}",
                            e
                        )));
                        return;
                    }
                },
                UpdateMode::Installer => {
                    std::env::temp_dir().join(format!("DiscordGameFarmer_Setup_v{}.exe", clean_ver))
                }
            };

            let resp = match ureq::get(&download_url)
                .set("User-Agent", USER_AGENT)
                .timeout(std::time::Duration::from_secs(60))
                .call()
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(UpdaterMessage::DownloadError(format!("Ошибка скачивания: {}", e)));
                    return;
                }
            };

            let total_bytes: u64 = resp
                .header("Content-Length")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            let mut reader = resp.into_reader();
            let mut file = match File::create(&target_path) {
                Ok(f) => f,
                Err(e) => {
                    let _ = tx.send(UpdaterMessage::DownloadError(format!(
                        "Ошибка создания файла: {}",
                        e
                    )));
                    return;
                }
            };

            let mut buffer = [0u8; 32768];
            let mut downloaded: u64 = 0;
            let mut last_progress_report = 0u64;

            loop {
                let bytes_read = match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) => {
                        let _ = std::fs::remove_file(&target_path);
                        let _ = tx.send(UpdaterMessage::DownloadError(format!(
                            "Ошибка загрузки данных: {}",
                            e
                        )));
                        return;
                    }
                };

                if let Err(e) = file.write_all(&buffer[..bytes_read]) {
                    let _ = std::fs::remove_file(&target_path);
                    let _ = tx.send(UpdaterMessage::DownloadError(format!(
                        "Ошибка записи на диск: {}",
                        e
                    )));
                    return;
                }

                downloaded += bytes_read as u64;

                if downloaded - last_progress_report >= 131072
                    || (total_bytes > 0 && downloaded == total_bytes)
                {
                    last_progress_report = downloaded;
                    let _ = tx.send(UpdaterMessage::DownloadProgress {
                        version: version.clone(),
                        downloaded,
                        total: total_bytes,
                    });
                }
            }

            let _ = file.flush();
            drop(file);

            if !is_valid_pe_file(&target_path) {
                let _ = std::fs::remove_file(&target_path);
                let _ = tx.send(UpdaterMessage::DownloadError(
                    "Скачанный файл повреждён или не является exe".to_string(),
                ));
                return;
            }

            let _ = tx.send(UpdaterMessage::DownloadFinished {
                version,
                installer_path: target_path,
            });
        });
    }

    /// Опрос очереди сообщений от фоновых потоков (вызывается в каждом кадре UI)
    pub fn poll(&mut self) -> PollOutcome {
        let mut log_message = None;
        let mut restart_ready = false;

        while let Ok(msg) = self.receiver.try_recv() {
            match msg {
                UpdaterMessage::CheckResult(res) => {
                    self.is_checking = false;
                    match res {
                        Ok(Some((ver, changelog, url))) => {
                            self.status = UpdateStatus::Available {
                                version: ver.clone(),
                                changelog,
                                download_url: url,
                            };
                            log_message = Some(format!(
                                "Найдено обновление: v{}. Доступно для установки!",
                                ver
                            ));
                        }
                        Ok(None) => {
                            self.status = UpdateStatus::UpToDate;
                            log_message = Some("Установлена актуальная версия программы.".to_string());
                        }
                        Err(e) => {
                            self.status = UpdateStatus::Error(e.clone());
                            log_message = Some(format!("Проверка обновлений: {}", e));
                        }
                    }
                }
                UpdaterMessage::DownloadProgress {
                    version,
                    downloaded,
                    total,
                } => {
                    let progress = if total > 0 {
                        (downloaded as f32 / total as f32).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    self.status = UpdateStatus::Downloading {
                        version,
                        progress,
                        downloaded,
                        total,
                    };
                }
                UpdaterMessage::DownloadFinished {
                    version: _,
                    installer_path,
                } => {
                    self.is_downloading = false;
                    self.status = UpdateStatus::Installing;

                    match Self::prepare_update_and_spawn_restart(&installer_path) {
                        Ok(()) => {
                            restart_ready = true;
                            log_message = Some("Обновление применено, выполняется перезапуск...".to_string());
                        }
                        Err(err) => {
                            self.status = UpdateStatus::Error(err.clone());
                            log_message = Some(format!("Ошибка установки обновления: {}", err));
                        }
                    }
                }
                UpdaterMessage::DownloadError(e) => {
                    self.is_downloading = false;
                    self.status = UpdateStatus::Error(e.clone());
                    log_message = Some(format!("Ошибка обновления: {}", e));
                }
            }
        }

        PollOutcome {
            log: log_message,
            restart_ready,
        }
    }

    fn spawn_hidden_powershell(script: &str) -> Result<(), String> {
        let mut cmd = Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", script]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd.spawn()
            .map(|_| ())
            .map_err(|e| format!("Не удалось запустить скрипт обновления: {}", e))
    }

    fn prepare_update_and_spawn_restart(downloaded_path: &Path) -> Result<(), String> {
        let pid = std::process::id();
        let mode = current_update_mode();

        match mode {
            UpdateMode::DirectSelfReplace => {
                let current_exe = std::env::current_exe()
                    .map_err(|e| format!("Не удалось определить текущий exe: {}", e))?;
                swap_executable_files(&current_exe, downloaded_path, pid)?;

                let workdir = current_exe
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| ".".to_string());

                let script = format!(
                    "Wait-Process -Id {pid} -Timeout 15 -ErrorAction SilentlyContinue; \
                     Start-Process -FilePath {target} -WorkingDirectory {workdir}",
                    pid = pid,
                    target = ps_single_quote(&current_exe.to_string_lossy()),
                    workdir = ps_single_quote(&workdir),
                );
                Self::spawn_hidden_powershell(&script)?;
                Ok(())
            }
            UpdateMode::Installer => {
                let current_exe = std::env::current_exe()
                    .map_err(|e| format!("Не удалось определить текущий exe: {}", e))?;
                let install_dir = current_exe
                    .parent()
                    .ok_or_else(|| "Не удалось определить папку установки".to_string())?;
                let dir_arg = format!("/DIR={}", install_dir.to_string_lossy());

                let script = format!(
                    "Wait-Process -Id {pid} -Timeout 15 -ErrorAction SilentlyContinue; \
                     Start-Process -FilePath {setup} -ArgumentList '/SILENT','/CLOSEAPPLICATIONS',{dir_arg} -Wait; \
                     if (Test-Path {target}) {{ Start-Process -FilePath {target} -WorkingDirectory {workdir} }}",
                    pid = pid,
                    setup = ps_single_quote(&downloaded_path.to_string_lossy()),
                    dir_arg = ps_single_quote(&dir_arg),
                    target = ps_single_quote(&current_exe.to_string_lossy()),
                    workdir = ps_single_quote(&install_dir.to_string_lossy()),
                );
                Self::spawn_hidden_powershell(&script)?;
                Ok(())
            }
        }
    }
}

pub fn is_legacy_temp_updater_launch(exe_path: &Path, args: &[String]) -> bool {
    let name = exe_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let looks_like_setup_name = name.starts_with("discordgamefarmer_setup_v") && name.ends_with(".exe");
    let has_silent_flags = args.iter().any(|a| a.eq_ignore_ascii_case("/SILENT"))
        && args.iter().any(|a| a.eq_ignore_ascii_case("/CLOSEAPPLICATIONS"));
    looks_like_setup_name || has_silent_flags
}

pub fn install_binary_to_dir(source_exe: &Path, target_dir: &Path, dummy_bytes: &[u8]) -> Result<(), String> {
    if !target_dir.exists() {
        return Err("Целевая папка не существует".to_string());
    }
    let target_exe = target_dir.join("discord-game-farmer.exe");
    let staged_exe = target_dir.join("discord-game-farmer.exe.new");
    let pid = std::process::id();

    let mut last_err = String::new();
    let mut swapped = false;
    for _ in 0..15 {
        if let Err(e) = std::fs::copy(source_exe, &staged_exe) {
            last_err = format!("Ошибка копирования в {}: {}", staged_exe.display(), e);
            std::thread::sleep(std::time::Duration::from_millis(150));
            continue;
        }
        if target_exe.exists() {
            match swap_executable_files(&target_exe, &staged_exe, pid) {
                Ok(_) => {
                    swapped = true;
                    break;
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&staged_exe);
                    last_err = e;
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
            }
        } else if let Err(e) = std::fs::rename(&staged_exe, &target_exe) {
            let _ = std::fs::remove_file(&staged_exe);
            last_err = format!("Ошибка записи {}: {}", target_exe.display(), e);
            std::thread::sleep(std::time::Duration::from_millis(150));
        } else {
            swapped = true;
            break;
        }
    }

    if !swapped {
        return Err(last_err);
    }

    let target_dummy = target_dir.join("dummy_game.exe");
    if std::fs::read(&target_dummy).ok().as_deref() != Some(dummy_bytes) {
        let _ = std::fs::write(&target_dummy, dummy_bytes);
    }
    Ok(())
}

#[cfg(windows)]
fn run_hidden_command(program: &str, args: &[&str]) -> Option<std::process::Output> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
}

#[cfg(windows)]
fn query_inno_install_location() -> Option<PathBuf> {
    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{5E99D631-4B28-4B2C-9852-7369B609E6D8}_is1";
    let out = run_hidden_command("reg.exe", &["query", key, "/v", "InstallLocation"])?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if let Some(pos) = line.find("REG_SZ") {
            let raw = line[pos + "REG_SZ".len()..].trim();
            if !raw.is_empty() {
                return Some(PathBuf::from(raw));
            }
        }
    }
    None
}

#[cfg(windows)]
fn update_inno_display_version(version: &str) {
    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{5E99D631-4B28-4B2C-9852-7369B609E6D8}_is1";
    if run_hidden_command("reg.exe", &["query", key, "/v", "InstallLocation"])
        .is_some_and(|o| o.status.success())
    {
        let _ = run_hidden_command(
            "reg.exe",
            &["add", key, "/v", "DisplayVersion", "/t", "REG_SZ", "/d", version, "/f"],
        );
    }
}

/// Если новая сборка запущена старым апдейтером как `%TEMP%\DiscordGameFarmer_Setup_v*.exe`,
/// она сама завершает старые процессы, переносит себя в папку установки/ярлыка и выходит,
/// чтобы `cmd /c start /wait ... & start ...` сразу запустил уже обновлённую версию.
pub fn handle_legacy_temp_updater_bootstrap(dummy_bytes: &[u8]) -> bool {
    let Ok(current_exe) = std::env::current_exe() else {
        return false;
    };
    let args: Vec<String> = std::env::args().collect();
    if !is_legacy_temp_updater_launch(&current_exe, &args) {
        return false;
    }

    #[cfg(windows)]
    {
        let pid_filter = format!("PID ne {}", std::process::id());
        let _ = run_hidden_command(
            "taskkill.exe",
            &["/F", "/FI", &pid_filter, "/IM", "discord-game-farmer.exe"],
        );
        let _ = run_hidden_command("taskkill.exe", &["/F", "/IM", "dummy_game.exe"]);
        std::thread::sleep(std::time::Duration::from_millis(250));

        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(reg_dir) = query_inno_install_location() {
            candidates.push(reg_dir);
        }
        let localappdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
        if !localappdata.is_empty() {
            candidates.push(PathBuf::from(&localappdata).join(r"Programs\DiscordGameFarmer"));
        }
        candidates.push(PathBuf::from(r"D:\DiscordGameFarmer"));
        candidates.push(PathBuf::from(r"C:\DiscordGameFarmer"));

        let mut seen = std::collections::HashSet::new();
        let mut updated_exe: Option<PathBuf> = None;

        for dir in candidates {
            let norm = dir
                .to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .to_ascii_lowercase();
            if norm.is_empty() || !seen.insert(norm) {
                continue;
            }
            if dir.join("discord-game-farmer.exe").exists() {
                if install_binary_to_dir(&current_exe, &dir, dummy_bytes).is_ok()
                    && updated_exe.is_none()
                {
                    updated_exe = Some(dir.join("discord-game-farmer.exe"));
                }
            }
        }

        if let Some(target_exe) = updated_exe {
            update_inno_display_version(env!("CARGO_PKG_VERSION"));
            let has_silent = args.iter().any(|a| a.eq_ignore_ascii_case("/SILENT"));
            if !has_silent {
                let mut cmd = Command::new(&target_exe);
                if let Some(parent) = target_exe.parent() {
                    cmd.current_dir(parent);
                }
                let _ = cmd.spawn();
            }
            return true;
        }
    }

    false
}

/// Атомарно заменяет `current_exe` на `new_exe` в той же папке с откатом при ошибке.
pub fn swap_executable_files(current_exe: &Path, new_exe: &Path, pid: u32) -> Result<PathBuf, String> {
    let default_old = current_exe.with_extension("exe.old");
    let backup_path = if default_old.exists() && std::fs::remove_file(&default_old).is_err() {
        current_exe.with_extension(format!("exe.{}.old", pid))
    } else {
        default_old
    };

    std::fs::rename(current_exe, &backup_path)
        .map_err(|e| format!("Не удалось переименовать текущий exe: {}", e))?;

    if let Err(e) = std::fs::rename(new_exe, current_exe) {
        let _ = std::fs::rename(&backup_path, current_exe);
        return Err(format!("Не удалось заменить exe на новую версию: {}", e));
    }

    Ok(backup_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_assets() -> Vec<GitHubAsset> {
        vec![
            GitHubAsset {
                name: "discord-game-farmer.exe".to_string(),
                browser_download_url: "https://example.com/discord-game-farmer.exe".to_string(),
                size: 11_000_000,
            },
            GitHubAsset {
                name: "DiscordGameFarmer-v0.2.16-portable.zip".to_string(),
                browser_download_url: "https://example.com/portable.zip".to_string(),
                size: 8_000_000,
            },
            GitHubAsset {
                name: "DiscordGameFarmer-v0.2.16-x64.msi".to_string(),
                browser_download_url: "https://example.com/app.msi".to_string(),
                size: 5_000_000,
            },
            GitHubAsset {
                name: "DiscordGameFarmer_Setup.exe".to_string(),
                browser_download_url: "https://example.com/DiscordGameFarmer_Setup.exe".to_string(),
                size: 5_600_000,
            },
        ]
    }

    #[test]
    fn direct_replace_selects_app_exe_not_setup_exe() {
        let assets = sample_assets();
        let chosen = select_release_asset(&assets, UpdateMode::DirectSelfReplace).unwrap();
        assert_eq!(chosen.name, "discord-game-farmer.exe");
    }

    #[test]
    fn installer_fallback_selects_setup_exe_not_app_exe() {
        let assets = sample_assets();
        let chosen = select_release_asset(&assets, UpdateMode::Installer).unwrap();
        assert_eq!(chosen.name, "DiscordGameFarmer_Setup.exe");
    }

    #[test]
    fn writable_temp_dir_uses_direct_self_replace() {
        let temp_exe = std::env::temp_dir().join("discord-game-farmer.exe");
        assert_eq!(detect_update_mode(&temp_exe), UpdateMode::DirectSelfReplace);
    }

    #[test]
    fn swap_executable_files_replaces_target_and_cleans_up() {
        let dir = std::env::temp_dir().join(format!("dgf_swap_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let target = dir.join("discord-game-farmer.exe");
        let new_file = dir.join("discord-game-farmer.exe.new");
        std::fs::write(&target, b"MZ_OLD_VERSION").unwrap();
        std::fs::write(&new_file, b"MZ_NEW_VERSION").unwrap();

        let backup = swap_executable_files(&target, &new_file, 1234).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"MZ_NEW_VERSION");
        assert_eq!(std::fs::read(&backup).unwrap(), b"MZ_OLD_VERSION");
        assert!(!new_file.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_temp_updater_launch_detection() {
        let temp_setup = Path::new(r"C:\Users\Test\AppData\Local\Temp\DiscordGameFarmer_Setup_v0.2.19.exe");
        assert!(is_legacy_temp_updater_launch(temp_setup, &[]));

        let normal_exe = Path::new(r"D:\DiscordGameFarmer\discord-game-farmer.exe");
        assert!(!is_legacy_temp_updater_launch(normal_exe, &[]));
        assert!(is_legacy_temp_updater_launch(
            normal_exe,
            &[
                "app".to_string(),
                "/SILENT".to_string(),
                "/CLOSEAPPLICATIONS".to_string()
            ]
        ));
    }

    #[test]
    fn install_binary_to_dir_replaces_exe_and_dummy_game() {
        let root = std::env::temp_dir().join(format!("dgf_bootstrap_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let install_dir = root.join("install");
        std::fs::create_dir_all(&install_dir).unwrap();

        let src_exe = root.join("DiscordGameFarmer_Setup_v0.2.19.exe");
        std::fs::write(&src_exe, b"MZ_NEW_APP_19").unwrap();
        std::fs::write(install_dir.join("discord-game-farmer.exe"), b"MZ_OLD_APP_8").unwrap();
        std::fs::write(install_dir.join("dummy_game.exe"), b"OLD_DUMMY").unwrap();

        install_binary_to_dir(&src_exe, &install_dir, b"NEW_DUMMY").unwrap();
        assert_eq!(
            std::fs::read(install_dir.join("discord-game-farmer.exe")).unwrap(),
            b"MZ_NEW_APP_19"
        );
        assert_eq!(
            std::fs::read(install_dir.join("dummy_game.exe")).unwrap(),
            b"NEW_DUMMY"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
