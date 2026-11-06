use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use serde::Deserialize;

const GITHUB_REPO: &str = "dimalinau-lab/Discord-Game-Activity-Manager";
const USER_AGENT: &str = "DiscordGameActivityManager";

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

    /// Очистка старых файлов .exe.old, если остались от предыдущих версий
    pub fn cleanup_previous_update() {
        if let Ok(current_exe) = std::env::current_exe() {
            let old_exe = current_exe.with_extension("exe.old");
            if old_exe.exists() {
                let _ = std::fs::remove_file(old_exe);
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
                // Ищем файл инсталлятора среди assets
                let installer = release.assets.iter().find(|a| {
                    a.name.ends_with("_Setup.exe") || a.name.ends_with("Setup.exe") || a.name.ends_with(".exe")
                });

                if let Some(asset) = installer {
                    let changelog = release.body
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
                    let _ = tx.send(UpdaterMessage::CheckResult(Err("Инсталлятор обновления не найден в релизе".to_string())));
                }
            } else {
                let _ = tx.send(UpdaterMessage::CheckResult(Ok(None)));
            }
        });
    }

    /// Запуск скачивания инсталлятора с отображением прогресса и автозапуском
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

        thread::spawn(move || {
            let clean_ver = version.trim_start_matches('v');
            let temp_installer = std::env::temp_dir().join(format!("DiscordGameFarmer_Setup_v{}.exe", clean_ver));

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
            let mut file = match File::create(&temp_installer) {
                Ok(f) => f,
                Err(e) => {
                    let _ = tx.send(UpdaterMessage::DownloadError(format!("Ошибка создания файла: {}", e)));
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
                        let _ = tx.send(UpdaterMessage::DownloadError(format!("Ошибка загрузки данных: {}", e)));
                        return;
                    }
                };

                if let Err(e) = file.write_all(&buffer[..bytes_read]) {
                    let _ = tx.send(UpdaterMessage::DownloadError(format!("Ошибка записи на диск: {}", e)));
                    return;
                }

                downloaded += bytes_read as u64;

                // Отправляем прогресс каждые 128 КБ
                if downloaded - last_progress_report >= 131072 || (total_bytes > 0 && downloaded == total_bytes) {
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

            let _ = tx.send(UpdaterMessage::DownloadFinished {
                version,
                installer_path: temp_installer,
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
                            log_message = Some(format!("Найдено обновление: v{}. Доступно для установки!", ver));
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
                UpdaterMessage::DownloadProgress { version, downloaded, total } => {
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
                UpdaterMessage::DownloadFinished { version: _, installer_path } => {
                    self.is_downloading = false;
                    self.status = UpdateStatus::Installing;

                    // Запуск установщика в тихом режиме с закрытием приложения и автозапуском
                    Self::launch_silent_installer(&installer_path);
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

    /// Запуск инсталлятора через cmd: ждёт завершения установки и запускает обновлённое приложение
    fn launch_silent_installer(installer_path: &PathBuf) {
        let localappdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
        let installed_app = PathBuf::from(&localappdata).join(r"Programs\DiscordGameFarmer\discord-game-farmer.exe");

        let target_app = if installed_app.exists() {
            installed_app
        } else if let Ok(current) = std::env::current_exe() {
            current
        } else {
            PathBuf::from("discord-game-farmer.exe")
        };

        let setup_path_str = installer_path.to_string_lossy().to_string();
        let target_app_str = target_app.to_string_lossy().to_string();

        let _ = Command::new("cmd.exe")
            .args(&[
                "/c",
                "start",
                "/wait",
                "",
                &setup_path_str,
                "/SILENT",
                "/CLOSEAPPLICATIONS",
                "&",
                "start",
                "",
                &target_app_str,
            ])
            .spawn();

        std::process::exit(0);
    }
}
