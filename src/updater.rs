use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    pub changelog: Option<String>,
    pub file_path: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        changelog: String,
        target_path: PathBuf,
    },
    Applying,
    Error(String),
}

pub struct AutoUpdater {
    pub current_version: String,
    pub status: UpdateStatus,
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
        Self {
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            status: UpdateStatus::Idle,
        }
    }

    /// Удаляет оставшийся .exe.old от предыдущего обновления при старте программы
    pub fn cleanup_previous_update() {
        if let Ok(current_exe) = std::env::current_exe() {
            let old_exe = current_exe.with_extension("exe.old");
            if old_exe.exists() {
                let _ = fs::remove_file(old_exe);
            }
        }
    }

    /// Поиск пути к файлу update_check.json
    fn find_manifest_path() -> PathBuf {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(parent) = exe_path.parent() {
                let candidate = parent.join("update_check.json");
                if candidate.exists() {
                    return candidate;
                }
            }
        }
        // Fallback: общий манифест в каталоге разработки на ПК
        let dev_manifest = PathBuf::from(r"d:\pton\pythonProject\discord\update_check.json");
        if dev_manifest.exists() {
            return dev_manifest;
        }

        PathBuf::from("update_check.json")
    }

    /// Проверка обновления через локальный манифест update_check.json
    pub fn check_local_update(&mut self) {
        self.status = UpdateStatus::Checking;
        let manifest_path = Self::find_manifest_path();

        if !manifest_path.exists() {
            self.status = UpdateStatus::UpToDate;
            return;
        }

        let content = match fs::read_to_string(&manifest_path) {
            Ok(c) => c,
            Err(e) => {
                self.status = UpdateStatus::Error(format!("Ошибка чтения: {}", e));
                return;
            }
        };

        let manifest: UpdateManifest = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(e) => {
                self.status = UpdateStatus::Error(format!("Ошибка JSON: {}", e));
                return;
            }
        };

        if is_newer_version(&manifest.version, &self.current_version) {
            let base_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
            let target_path = if Path::new(&manifest.file_path).is_absolute() {
                PathBuf::from(&manifest.file_path)
            } else {
                base_dir.join(&manifest.file_path)
            };

            if !target_path.exists() {
                self.status = UpdateStatus::Error(format!("Файл обновления не найден: {}", target_path.display()));
                return;
            }

            self.status = UpdateStatus::Available {
                version: manifest.version,
                changelog: manifest.changelog.unwrap_or_else(|| "Обновление готово к установке".to_string()),
                target_path,
            };
        } else {
            self.status = UpdateStatus::UpToDate;
        }
    }

    /// Atomic In-Place Swap: переименование текущего exe в .old, копирование нового и перезапуск
    pub fn apply_update(&mut self, new_binary_path: &Path) -> Result<(), String> {
        self.status = UpdateStatus::Applying;
        let current_exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let old_exe = current_exe.with_extension("exe.old");

        if old_exe.exists() {
            let _ = fs::remove_file(&old_exe);
        }

        // 1. Переименовываем текущий запущенный exe в .old (разрешено Windows)
        fs::rename(&current_exe, &old_exe)
            .map_err(|e| format!("Не удалось переместить текущий exe: {}", e))?;

        // 2. Копируем новый бинарник на место исходного exe
        if let Err(e) = fs::copy(new_binary_path, &current_exe) {
            // Откат в случае ошибки
            let _ = fs::rename(&old_exe, &current_exe);
            return Err(format!("Не удалось скопировать новый файл: {}", e));
        }

        // 3. Запускаем обновлённый исполняемый файл
        let mut cmd = Command::new(&current_exe);
        if let Some(parent) = current_exe.parent() {
            cmd.current_dir(parent);
        }

        cmd.spawn().map_err(|e| format!("Ошибка перезапуска: {}", e))?;

        // 4. Завершаем текущий процесс
        std::process::exit(0);
    }
}

