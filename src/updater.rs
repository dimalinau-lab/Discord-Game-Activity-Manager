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
    Installer,
    PortableExe,
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

/// Определяет, запущена ли программа из установленной папки или как portable .exe.
pub fn detect_update_mode(exe_path: &Path, localappdata: &str) -> UpdateMode {
    if let Some(parent) = exe_path.parent() {
        if parent.join("unins000.exe").exists() {
            return UpdateMode::Installer;
        }
        if !localappdata.is_empty() {
            let default_install_dir = Path::new(localappdata).join(r"Programs\DiscordGameFarmer");
            let parent_norm = parent.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
            let default_norm = default_install_dir
                .to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .to_lowercase();
            if parent_norm == default_norm {
                return UpdateMode::Installer;
            }
        }
    }
    UpdateMode::PortableExe
}

fn current_update_mode() -> UpdateMode {
    let localappdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
    std::env::current_exe()
        .map(|p| detect_update_mode(&p, &localappdata))
        .unwrap_or(UpdateMode::Installer)
}

/// Строгий выбор ассета под режим: установщик (*_Setup.exe) или portable-бинарник (discord-game-farmer.exe).
pub fn select_release_asset(assets: &[GitHubAsset], mode: UpdateMode) -> Option<&GitHubAsset> {
    match mode {
        UpdateMode::Installer => assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case("DiscordGameFarmer_Setup.exe"))
            .or_else(|| {
                assets.iter().find(|a| {
                    let lower = a.name.to_ascii_lowercase();
                    lower.ends_with("_setup.exe") || lower.ends_with("setup.exe")
                })
            }),
        UpdateMode::PortableExe => assets
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case("discord-game-farmer.exe"))
            .or_else(|| {
                assets.iter().find(|a| {
                    let lower = a.name.to_ascii_lowercase();
                    lower.ends_with(".exe") && !lower.contains("setup")
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

fn ps_single_quote(path: &Path) -> String {
    let raw = path.to_string_lossy();
    format!("'{}'", raw.replace('\'', "''"))
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

    /// Очистка старых файлов .exe.old, если остались от предыдущих обновлений
    pub fn cleanup_previous_update() {
        if let Ok(current_exe) = std::env::current_exe() {
            let old_exe = current_exe.with_extension("exe.old");
            if old_exe.exists() {
                let _ = std::fs::remove_file(old_exe);
            }
            let new_exe = current_exe.with_extension("exe.new");
            if new_exe.exists() {
                let _ = std::fs::remove_file(new_exe);
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
                UpdateMode::Installer => {
                    std::env::temp_dir().join(format!("DiscordGameFarmer_Setup_v{}.exe", clean_ver))
                }
                UpdateMode::PortableExe => match std::env::current_exe() {
                    Ok(cur) => cur.with_extension("exe.new"),
                    Err(e) => {
                        let _ = tx.send(UpdaterMessage::DownloadError(format!(
                            "Не удалось определить путь exe: {}",
                            e
                        )));
                        return;
                    }
                },
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

                // Отправляем прогресс каждые 128 КБ
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
    pub fn poll(&mut self) -> Option<String> {
        let mut log_message = None;

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

                    if let Err(err) = Self::apply_update_and_restart(&installer_path) {
                        self.status = UpdateStatus::Error(err.clone());
                        log_message = Some(format!("Ошибка установки обновления: {}", err));
                    }
                }
                UpdaterMessage::DownloadError(e) => {
                    self.is_downloading = false;
                    self.status = UpdateStatus::Error(e.clone());
                    log_message = Some(format!("Ошибка обновления: {}", e));
                }
            }
        }

        log_message
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

    fn apply_update_and_restart(downloaded_path: &Path) -> Result<(), String> {
        let pid = std::process::id();
        let mode = current_update_mode();

        match mode {
            UpdateMode::Installer => {
                let localappdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
                let default_installed = PathBuf::from(&localappdata)
                    .join(r"Programs\DiscordGameFarmer\discord-game-farmer.exe");
                let target_app = match std::env::current_exe() {
                    Ok(cur) if detect_update_mode(&cur, &localappdata) == UpdateMode::Installer => {
                        cur
                    }
                    _ => default_installed,
                };

                let script = format!(
                    "Wait-Process -Id {pid} -Timeout 10 -ErrorAction SilentlyContinue; \
                     Start-Process -FilePath {setup} -ArgumentList '/SILENT','/CLOSEAPPLICATIONS' -Wait; \
                     if (Test-Path {target}) {{ Start-Process -FilePath {target} }}",
                    pid = pid,
                    setup = ps_single_quote(downloaded_path),
                    target = ps_single_quote(&target_app),
                );
                Self::spawn_hidden_powershell(&script)?;
                std::process::exit(0);
            }
            UpdateMode::PortableExe => {
                let current_exe = std::env::current_exe()
                    .map_err(|e| format!("Не удалось определить текущий exe: {}", e))?;
                let old_exe = current_exe.with_extension("exe.old");
                if old_exe.exists() {
                    let _ = std::fs::remove_file(&old_exe);
                }

                std::fs::rename(&current_exe, &old_exe)
                    .map_err(|e| format!("Не удалось переименовать текущий exe: {}", e))?;

                if let Err(e) = std::fs::rename(downloaded_path, &current_exe) {
                    let _ = std::fs::rename(&old_exe, &current_exe);
                    return Err(format!("Не удалось заменить exe на новую версию: {}", e));
                }

                let script = format!(
                    "Wait-Process -Id {pid} -Timeout 10 -ErrorAction SilentlyContinue; \
                     Start-Process -FilePath {target}",
                    pid = pid,
                    target = ps_single_quote(&current_exe),
                );
                Self::spawn_hidden_powershell(&script)?;
                std::process::exit(0);
            }
        }
    }
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
                name: "DiscordGameFarmer-v0.2.15-portable.zip".to_string(),
                browser_download_url: "https://example.com/portable.zip".to_string(),
                size: 8_000_000,
            },
            GitHubAsset {
                name: "DiscordGameFarmer-v0.2.15-x64.msi".to_string(),
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
    fn installer_mode_selects_setup_exe_not_app_exe() {
        let assets = sample_assets();
        let chosen = select_release_asset(&assets, UpdateMode::Installer).unwrap();
        assert_eq!(chosen.name, "DiscordGameFarmer_Setup.exe");
    }

    #[test]
    fn portable_mode_selects_app_exe_not_setup_exe() {
        let assets = sample_assets();
        let chosen = select_release_asset(&assets, UpdateMode::PortableExe).unwrap();
        assert_eq!(chosen.name, "discord-game-farmer.exe");
    }

    #[test]
    fn detect_mode_recognizes_localappdata_install_and_portable() {
        let localappdata = r"C:\Users\Test\AppData\Local";
        let installed = Path::new(r"C:\Users\Test\AppData\Local\Programs\DiscordGameFarmer\discord-game-farmer.exe");
        assert_eq!(detect_update_mode(installed, localappdata), UpdateMode::Installer);

        let portable = Path::new(r"D:\Games\DiscordFarmer\discord-game-farmer.exe");
        assert_eq!(detect_update_mode(portable, localappdata), UpdateMode::PortableExe);
    }
}
