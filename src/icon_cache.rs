use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use eframe::egui;
use serde::Deserialize;

#[derive(Deserialize)]
struct DiscordAppRpcMeta {
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

struct FetchResult {
    app_id: String,
    png_bytes: Option<Vec<u8>>,
    description: Option<String>,
}

/// Кэш иконок и кратких описаний игр из Discord API/CDN с сохранением на диск.
pub struct IconCache {
    textures: HashMap<String, Option<egui::TextureHandle>>,
    descriptions: HashMap<String, String>,
    requested: HashSet<String>,
    queue: Arc<(Mutex<VecDeque<String>>, Condvar)>,
    rx: Receiver<FetchResult>,
    ctx_slot: Arc<Mutex<Option<egui::Context>>>,
    cache_dir: PathBuf,
}

impl IconCache {
    pub fn new() -> Self {
        let cache_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("icon_cache")))
            .unwrap_or_else(|| PathBuf::from("icon_cache"));
        let _ = std::fs::create_dir_all(&cache_dir);

        let queue: Arc<(Mutex<VecDeque<String>>, Condvar)> =
            Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let (tx, rx) = channel::<FetchResult>();
        let ctx_slot: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));

        // Два фоновых воркера для неблокирующей подгрузки видимых карточек
        for _ in 0..2 {
            let q_clone = Arc::clone(&queue);
            let tx_clone: Sender<FetchResult> = tx.clone();
            let dir_clone = cache_dir.clone();
            let ctx_clone = Arc::clone(&ctx_slot);

            thread::spawn(move || loop {
                let app_id = {
                    let (lock, cvar) = &*q_clone;
                    let mut guard = match lock.lock() {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                    while guard.is_empty() {
                        guard = match cvar.wait(guard) {
                            Ok(g) => g,
                            Err(_) => return,
                        };
                    }
                    // Берём с конца (LIFO), чтобы приоритет имели карточки на текущем экране
                    match guard.pop_back() {
                        Some(id) => id,
                        None => continue,
                    }
                };

                let res = fetch_or_load_cached(&dir_clone, &app_id);
                let _ = tx_clone.send(res);
                if let Ok(guard) = ctx_clone.lock() {
                    if let Some(ref ctx) = *guard {
                        ctx.request_repaint();
                    }
                }
            });
        }

        Self {
            textures: HashMap::new(),
            descriptions: HashMap::new(),
            requested: HashSet::new(),
            queue,
            rx,
            ctx_slot,
            cache_dir,
        }
    }

    /// Запрашивает текстуру и описание игры, если она ещё не загружена.
    pub fn ensure_requested(&mut self, app_id: &str) {
        if app_id.is_empty() || self.requested.contains(app_id) {
            return;
        }
        self.requested.insert(app_id.to_string());

        let (lock, cvar) = &*self.queue;
        if let Ok(mut guard) = lock.lock() {
            // Ограничиваем очередь при быстрой прокрутке
            if guard.len() >= 48 {
                if let Some(dropped) = guard.pop_front() {
                    self.requested.remove(&dropped);
                }
            }
            guard.push_back(app_id.to_string());
            cvar.notify_one();
        }
    }

    /// Принимает загруженные иконки из фоновых потоков и регистрирует их в egui.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Ok(mut slot) = self.ctx_slot.lock() {
            if slot.is_none() {
                *slot = Some(ctx.clone());
            }
        }

        while let Ok(res) = self.rx.try_recv() {
            if let Some(desc) = res.description {
                let clean = desc.trim().to_string();
                if !clean.is_empty() {
                    self.descriptions.insert(res.app_id.clone(), clean);
                }
            }

            let tex_opt = res.png_bytes.and_then(|bytes| decode_png_to_texture(ctx, &res.app_id, &bytes));
            self.textures.insert(res.app_id, tex_opt);
        }
    }

    pub fn get_texture(&self, app_id: &str) -> Option<&egui::TextureHandle> {
        self.textures.get(app_id).and_then(|opt| opt.as_ref())
    }

    pub fn png_path(&self, app_id: &str) -> PathBuf {
        self.cache_dir.join(format!("{}.png", app_id))
    }

    pub fn get_description(&self, app_id: &str) -> Option<&str> {
        self.descriptions.get(app_id).map(|s| s.as_str())
    }

    /// Очищает неудачные/отрицательные маркеры для повторной загрузки по кнопке обновления.
    pub fn clear_negative_cache(&mut self) {
        self.textures.retain(|_, v| v.is_some());
        self.requested.retain(|id| self.textures.contains_key(id));
        if let Ok(entries) = std::fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                if entry.path().extension().and_then(|e| e.to_str()) == Some("none") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

fn decode_png_to_texture(ctx: &egui::Context, app_id: &str, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color_img = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Some(ctx.load_texture(
        format!("game_icon_{}", app_id),
        color_img,
        egui::TextureOptions::LINEAR,
    ))
}

fn fetch_or_load_cached(cache_dir: &Path, app_id: &str) -> FetchResult {
    let png_path = cache_dir.join(format!("{}.png", app_id));
    let meta_path = cache_dir.join(format!("{}.txt", app_id));
    let none_path = cache_dir.join(format!("{}.none", app_id));

    let cached_desc = std::fs::read_to_string(&meta_path).ok();
    if png_path.exists() {
        if let Ok(bytes) = std::fs::read(&png_path) {
            return FetchResult {
                app_id: app_id.to_string(),
                png_bytes: Some(bytes),
                description: cached_desc,
            };
        }
    }
    if none_path.exists() {
        return FetchResult {
            app_id: app_id.to_string(),
            png_bytes: None,
            description: cached_desc,
        };
    }

    // Запрашиваем метаданные у Discord RPC API
    let rpc_url = format!("https://discord.com/api/v10/applications/{}/rpc", app_id);
    let resp = ureq::get(&rpc_url)
        .set("User-Agent", "DiscordGameActivityManager")
        .timeout(std::time::Duration::from_secs(6))
        .call();

    let Ok(r) = resp else {
        return FetchResult {
            app_id: app_id.to_string(),
            png_bytes: None,
            description: None,
        };
    };

    let Ok(meta) = r.into_json::<DiscordAppRpcMeta>() else {
        let _ = std::fs::write(&none_path, b"");
        return FetchResult {
            app_id: app_id.to_string(),
            png_bytes: None,
            description: None,
        };
    };

    let description = meta.description.filter(|d| !d.trim().is_empty());
    if let Some(ref desc) = description {
        let _ = std::fs::write(&meta_path, desc);
    }

    let Some(icon_hash) = meta.icon.filter(|h| !h.trim().is_empty()) else {
        let _ = std::fs::write(&none_path, b"");
        return FetchResult {
            app_id: app_id.to_string(),
            png_bytes: None,
            description,
        };
    };

    let cdn_url = format!(
        "https://cdn.discordapp.com/app-icons/{}/{}.png?size=128",
        app_id, icon_hash
    );
    let img_resp = ureq::get(&cdn_url)
        .set("User-Agent", "DiscordGameActivityManager")
        .timeout(std::time::Duration::from_secs(6))
        .call();

    if let Ok(ir) = img_resp {
        let mut bytes = Vec::new();
        if std::io::Read::read_to_end(&mut ir.into_reader(), &mut bytes).is_ok() && !bytes.is_empty() {
            let tmp_path = png_path.with_extension("png.tmp");
            if std::fs::write(&tmp_path, &bytes).is_ok() {
                let _ = std::fs::rename(&tmp_path, &png_path);
            }
            return FetchResult {
                app_id: app_id.to_string(),
                png_bytes: Some(bytes),
                description,
            };
        }
    }

    let _ = std::fs::write(&none_path, b"");
    FetchResult {
        app_id: app_id.to_string(),
        png_bytes: None,
        description,
    }
}
