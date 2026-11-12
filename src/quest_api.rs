use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde_json::Value;

pub const DISCORD_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) discord/1.0.9267 Chrome/128.0.6613.186 Electron/32.2.8 Safari/537.36";

#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[link(name = "crypt32")]
extern "system" {
    fn CryptUnprotectData(
        p_data_in: *const DataBlob,
        ppsz_data_descr: *mut *mut u16,
        p_optional_entropy: *const DataBlob,
        pv_reserved: *mut std::ffi::c_void,
        p_prompt_struct: *mut std::ffi::c_void,
        dw_flags: u32,
        p_data_out: *mut DataBlob,
    ) -> i32;
    fn LocalFree(h_mem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

/// Извлечение токена Discord из зашифрованного локального хранилища Windows (DPAPI + AES-GCM)
pub fn extract_discord_token() -> Option<String> {
    let appdata = std::env::var("APPDATA").ok()?;
    let local_state_path = PathBuf::from(&appdata).join(r"discord\Local State");
    let local_state_content = fs::read_to_string(&local_state_path).ok()?;
    let local_state: Value = serde_json::from_str(&local_state_content).ok()?;
    
    let enc_key_b64 = local_state.get("os_crypt")?.get("encrypted_key")?.as_str()?;
    let enc_key = BASE64.decode(enc_key_b64).ok()?;
    if enc_key.len() <= 5 {
        return None;
    }
    
    // DPAPI расшифровка мастер-ключа
    let dpapi_bytes = &enc_key[5..];
    let in_blob = DataBlob {
        cb_data: dpapi_bytes.len() as u32,
        pb_data: dpapi_bytes.as_ptr() as *mut u8,
    };
    let mut out_blob = DataBlob {
        cb_data: 0,
        pb_data: std::ptr::null_mut(),
    };

    let master_key = unsafe {
        if CryptUnprotectData(&in_blob, std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), 0, &mut out_blob) != 0 {
            let key = std::slice::from_raw_parts(out_blob.pb_data, out_blob.cb_data as usize).to_vec();
            LocalFree(out_blob.pb_data as *mut std::ffi::c_void);
            key
        } else {
            return None;
        }
    };

    let leveldb_dir = PathBuf::from(&appdata).join(r"discord\Local Storage\leveldb");
    let entries = fs::read_dir(leveldb_dir).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "log" && ext != "ldb" {
            continue;
        }

        if let Ok(bytes) = fs::read(&path) {
            let prefix = b"dQw4w9WgXcQ:";
            let mut start = 0;
            while let Some(pos) = bytes[start..].windows(prefix.len()).position(|w| w == prefix) {
                let match_start = start + pos + prefix.len();
                let mut match_end = match_start;
                while match_end < bytes.len() && bytes[match_end] != b'"' && bytes[match_end] != b'\'' {
                    match_end += 1;
                }

                if let Ok(token_b64) = std::str::from_utf8(&bytes[match_start..match_end]) {
                    if let Ok(enc_payload) = BASE64.decode(token_b64.trim()) {
                        if enc_payload.len() > 15 {
                            let iv = &enc_payload[3..15];
                            let ciphertext = &enc_payload[15..];
                            if let Ok(cipher) = Aes256Gcm::new_from_slice(&master_key) {
                                #[allow(deprecated)]
                                let nonce = Nonce::from_slice(iv);
                                if let Ok(decrypted) = cipher.decrypt(nonce, ciphertext) {
                                    if let Ok(token_str) = String::from_utf8(decrypted) {
                                        if verify_token(&token_str) {
                                            return Some(token_str);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                start = match_end;
            }
        }
    }

    None
}

fn verify_token(token: &str) -> bool {
    let resp = ureq::get("https://discord.com/api/v9/users/@me")
        .set("Authorization", token)
        .set("User-Agent", DISCORD_USER_AGENT)
        .call();
    resp.is_ok()
}

#[derive(Debug, Clone)]
pub struct EnrolledQuest {
    pub quest_id: String,
    pub quest_name: String,
    pub app_id: String,
    pub target_seconds: u64,
    pub current_seconds: u64,
    pub completed: bool,
}

/// Поиск активного принятого квеста по application_id и названию игры
/// ВСЕГДА отдает предпочтение активным (не завершенным) заданиям!
pub fn get_enrolled_quest(token: &str, app_id: &str, game_name: &str) -> Option<EnrolledQuest> {
    let resp = ureq::get("https://discord.com/api/v9/quests/@me")
        .set("Authorization", token)
        .set("User-Agent", DISCORD_USER_AGENT)
        .call()
        .ok()?;

    let json: Value = resp.into_json().ok()?;
    let quests = json.get("quests")?.as_array()?;

    let mut matched_quests = Vec::new();

    for q in quests {
        let q_id = match q.get("id").and_then(|i| i.as_str()) {
            Some(id) => id,
            None => continue,
        };
        let cfg = match q.get("config") {
            Some(c) => c,
            None => continue,
        };
        let q_name = cfg.get("messages")
            .and_then(|m| m.get("quest_name"))
            .and_then(|n| n.as_str())
            .unwrap_or("Quest")
            .to_string();
        
        let mut matches_app = false;
        let mut target_secs = 900;

        if let Some(tasks) = cfg.get("task_config_v2").and_then(|t| t.get("tasks")) {
            if let Some(play_task) = tasks.get("PLAY_ON_DESKTOP") {
                if let Some(apps) = play_task.get("applications").and_then(|a| a.as_array()) {
                    for app in apps {
                        if app.get("id").and_then(|i| i.as_str()) == Some(app_id) {
                            matches_app = true;
                            target_secs = play_task.get("target").and_then(|t| t.as_u64()).unwrap_or(900);
                            break;
                        }
                    }
                }
            }
        }

        if matches_app {
            let user_status = q.get("user_status");
            let is_enrolled = user_status.map_or(false, |s| !s.is_null() && s.get("enrolled_at").map_or(false, |e| !e.is_null()));
            let is_completed = user_status.map_or(false, |s| !s.is_null() && s.get("completed_at").map_or(false, |c| !c.is_null()));
            
            let current_secs = user_status
                .and_then(|s| s.get("progress"))
                .and_then(|p| p.get("PLAY_ON_DESKTOP"))
                .and_then(|d| d.get("value"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            if is_enrolled {
                matched_quests.push(EnrolledQuest {
                    quest_id: q_id.to_string(),
                    quest_name: q_name,
                    app_id: app_id.to_string(),
                    target_seconds: target_secs,
                    current_seconds: current_secs,
                    completed: is_completed,
                });
            }
        }
    }

    if matched_quests.is_empty() {
        return None;
    }

    let gn_lower = game_name.to_lowercase();

    // 1. Приоритет №1: Ищем совпадение среди НЕЗАВЕРШЕННЫХ квестов по имени
    for q in matched_quests.iter().filter(|q| !q.completed) {
        let qn_lower = q.quest_name.to_lowercase();
        if gn_lower.contains(&qn_lower) || qn_lower.contains(&gn_lower) {
            return Some(q.clone());
        }
    }

    // 2. Приоритет №2: Любой незавершенный квест для этого app_id
    if let Some(uncompleted) = matched_quests.iter().find(|q| !q.completed) {
        return Some(uncompleted.clone());
    }

    // 3. Приоритет №3 (только если все завершены): совпадение по имени среди завершенных
    for q in &matched_quests {
        let qn_lower = q.quest_name.to_lowercase();
        if gn_lower.contains(&qn_lower) || qn_lower.contains(&gn_lower) {
            return Some(q.clone());
        }
    }

    // 4. Fallback
    matched_quests.into_iter().next()
}

/// Проверка, доступен ли квест для app_id, который еще не был принят в Discord
pub fn get_unenrolled_quest_name(token: &str, app_id: &str, game_name: &str) -> Option<String> {
    let resp = ureq::get("https://discord.com/api/v9/quests/@me")
        .set("Authorization", token)
        .set("User-Agent", DISCORD_USER_AGENT)
        .call()
        .ok()?;

    let json: Value = resp.into_json().ok()?;
    let quests = json.get("quests")?.as_array()?;

    let gn_lower = game_name.to_lowercase();
    let mut candidate = None;

    for q in quests {
        let cfg = match q.get("config") {
            Some(c) => c,
            None => continue,
        };
        let q_name = cfg.get("messages")
            .and_then(|m| m.get("quest_name"))
            .and_then(|n| n.as_str())
            .unwrap_or("Quest");

        let mut matches_app = false;
        if let Some(tasks) = cfg.get("task_config_v2").and_then(|t| t.get("tasks")) {
            if let Some(play_task) = tasks.get("PLAY_ON_DESKTOP") {
                if let Some(apps) = play_task.get("applications").and_then(|a| a.as_array()) {
                    for app in apps {
                        if app.get("id").and_then(|i| i.as_str()) == Some(app_id) {
                            matches_app = true;
                            break;
                        }
                    }
                }
            }
        }

        if matches_app {
            let user_status = q.get("user_status");
            let is_enrolled = user_status.map_or(false, |s| !s.is_null() && s.get("enrolled_at").map_or(false, |e| !e.is_null()));
            if !is_enrolled {
                let qn_lower = q_name.to_lowercase();
                if gn_lower.contains(&qn_lower) || qn_lower.contains(&gn_lower) {
                    return Some(q_name.to_string());
                }
                if candidate.is_none() {
                    candidate = Some(q_name.to_string());
                }
            }
        }
    }

    candidate
}

/// Отправка heartbeat пакета квеста с настоящим Discord Desktop User-Agent
pub fn send_heartbeat(token: &str, quest_id: &str, app_id: &str, terminal: bool) -> Result<u64, String> {
    let url = format!("https://discord.com/api/v9/quests/{}/heartbeat", quest_id);
    let payload = serde_json::json!({
        "stream_key": null,
        "application_id": app_id,
        "terminal": terminal
    });

    let resp = ureq::post(&url)
        .set("Authorization", token)
        .set("Content-Type", "application/json")
        .set("User-Agent", DISCORD_USER_AGENT)
        .send_json(payload)
        .map_err(|e| e.to_string())?;

    let json: Value = resp.into_json().map_err(|e| e.to_string())?;
    let value = json.get("progress")
        .and_then(|p| p.get("PLAY_ON_DESKTOP"))
        .and_then(|d| d.get("value"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    Ok(value)
}

/// Фоновый авто-сборщик квеста (запускается в отдельном потоке)
pub fn spawn_quest_heartbeat_worker(
    token: String,
    quest: EnrolledQuest,
    stop_signal: Arc<AtomicBool>,
    status_sender: Sender<String>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut cur_secs = quest.current_seconds;
        let pct = if quest.target_seconds > 0 { (cur_secs * 100) / quest.target_seconds } else { 0 };
        let _ = status_sender.send(format!(
            "{}/{} сек. ({}%)", cur_secs, quest.target_seconds, pct
        ));
        
        while !stop_signal.load(Ordering::Relaxed) && cur_secs < quest.target_seconds {
            // Ожидание 30 секунд с частой проверкой сигнала остановки (мгновенная реакция)
            for _ in 0..30 {
                thread::sleep(Duration::from_secs(1));
                if stop_signal.load(Ordering::Relaxed) {
                    break;
                }
            }
            if stop_signal.load(Ordering::Relaxed) {
                break;
            }

            match send_heartbeat(&token, &quest.quest_id, &quest.app_id, false) {
                Ok(val) => {
                    cur_secs = val;
                    let pct = if quest.target_seconds > 0 {
                        (cur_secs * 100) / quest.target_seconds
                    } else {
                        100
                    };
                    let _ = status_sender.send(format!(
                        "{}/{} сек. ({}%)", cur_secs, quest.target_seconds, pct
                    ));

                    if cur_secs >= quest.target_seconds {
                        let _ = send_heartbeat(&token, &quest.quest_id, &quest.app_id, true);
                        let _ = status_sender.send(format!(
                            "100% ВЫПОЛНЕНО! Заберите награду в Discord."
                        ));
                        break;
                    }
                }
                Err(e) => {
                    let _ = status_sender.send(format!(
                        "Ошибка связи с сервером Discord: {}", e
                    ));
                    thread::sleep(Duration::from_secs(5));
                }
            }
        }
    })
}
