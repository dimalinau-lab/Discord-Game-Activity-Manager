use std::fs;
use std::path::PathBuf;

const PATCH_MARKER: &str = "__discordAntiPauseInjected";

const PATCH_CODE: &str = r#"const electron = require('electron');

const ANTI_PAUSE_SCRIPT = `
(() => {
    try {
        if (window.__discordAntiPauseInjected) return;
        window.__discordAntiPauseInjected = true;

        // 1. Постоянная видимость вкладки и наличие фокуса
        try {
            Object.defineProperty(document, 'hidden', {
                get: () => false,
                configurable: true
            });
            Object.defineProperty(document, 'visibilityState', {
                get: () => 'visible',
                configurable: true
            });
            document.hasFocus = () => true;
        } catch (_) {}

        // 2. Блокировка событий потери фокуса в DOM
        const originalAddEventListener = EventTarget.prototype.addEventListener;
        EventTarget.prototype.addEventListener = function(type, listener, options) {
            if ((this === window || this === document) && 
                (type === 'blur' || type === 'visibilitychange' || type === 'webkitvisibilitychange')) {
                return;
            }
            return originalAddEventListener.call(this, type, listener, options);
        };

        ['visibilitychange', 'webkitvisibilitychange', 'blur'].forEach(evt => {
            window.addEventListener(evt, e => e.stopImmediatePropagation(), true);
            document.addEventListener(evt, e => e.stopImmediatePropagation(), true);
        });

        // 3. Блокировка программного вызова pause() у видеоэлементов (React effects, stores)
        const origPause = HTMLVideoElement.prototype.pause;
        HTMLVideoElement.prototype.pause = function(...args) {
            const evt = window.event;
            const isUserAction = evt && (evt.type === 'click' || evt.type === 'keydown' || evt.type === 'pointerup');
            if (!isUserAction) {
                console.log('[DiscordGameFarmer] Авто-пауза видео заблокирована');
                return;
            }
            return origPause.apply(this, args);
        };

        // 4. Авто-возобновление воспроизведения, если видео всё же приостановлено в фоне
        document.addEventListener('pause', (e) => {
            if (e.target instanceof HTMLVideoElement && !e.target.ended) {
                const evt = window.event;
                const isUserAction = evt && (evt.type === 'click' || evt.type === 'keydown' || evt.type === 'pointerup');
                if (!isUserAction) {
                    setTimeout(() => {
                        if (e.target.paused && !e.target.ended) {
                            e.target.play().catch(() => {});
                        }
                    }, 50);
                }
            }
        }, true);

        console.log('[DiscordGameFarmer] Анти-пауза видео активна');
    } catch (e) {
        console.error('[DiscordGameFarmer] Ошибка инициализации анти-паузы:', e);
    }
})();
`;

function setupWindow(win) {
    if (!win || !win.webContents || win.__antiPauseConfigured) return;
    win.__antiPauseConfigured = true;

    // Отключаем фоновый троттлинг таймеров и видео в Electron
    try {
        if (typeof win.webContents.setBackgroundThrottling === 'function') {
            win.webContents.setBackgroundThrottling(false);
        }
        win.webContents.backgroundThrottling = false;
    } catch (_) {}

    // Перехватываем IPC-сообщения главного процесса:
    // Подменяем MAIN_WINDOW_BLUR и MAIN_WINDOW_HIDDEN на MAIN_WINDOW_FOCUS,
    // чтобы WindowStore и видеоплеер Discord всегда считали окно активным
    const origSend = win.webContents.send;
    win.webContents.send = function(channel, ...args) {
        if (typeof channel === 'string' && (channel.endsWith('MAIN_WINDOW_BLUR') || channel.endsWith('MAIN_WINDOW_HIDDEN'))) {
            const focusChannel = channel.replace(/MAIN_WINDOW_(BLUR|HIDDEN)/, 'MAIN_WINDOW_FOCUS');
            return origSend.call(this, focusChannel, ...args);
        }
        return origSend.call(this, channel, ...args);
    };

    const inject = () => {
        win.webContents.executeJavaScript(ANTI_PAUSE_SCRIPT).catch(() => {});
    };
    win.webContents.on('dom-ready', inject);
    win.webContents.on('did-finish-load', inject);
    inject();
}

if (electron && electron.app) {
    electron.app.on('browser-window-created', (event, win) => {
        setupWindow(win);
    });
    if (electron.BrowserWindow && typeof electron.BrowserWindow.getAllWindows === 'function') {
        electron.BrowserWindow.getAllWindows().forEach(setupWindow);
    }
}

module.exports = require('./core.asar');
"#;

pub fn find_discord_core_index_js() -> Option<PathBuf> {
    let localappdata = std::env::var("LOCALAPPDATA").ok()?;
    let discord_dir = PathBuf::from(localappdata).join("Discord");
    if !discord_dir.exists() {
        return None;
    }

    let mut app_dirs = Vec::new();
    if let Ok(entries) = fs::read_dir(&discord_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("app-") {
                        app_dirs.push(path);
                    }
                }
            }
        }
    }
    app_dirs.sort();
    let latest_app = app_dirs.pop()?;

    let modules_dir = latest_app.join("modules");
    if let Ok(entries) = fs::read_dir(&modules_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("discord_desktop_core") {
                        let index_js = path.join("discord_desktop_core").join("index.js");
                        if index_js.exists() {
                            return Some(index_js);
                        }
                    }
                }
            }
        }
    }

    None
}

pub fn is_anti_pause_installed() -> bool {
    if let Some(index_js) = find_discord_core_index_js() {
        if let Ok(content) = fs::read_to_string(index_js) {
            return content.contains(PATCH_MARKER);
        }
    }
    false
}

pub fn install_anti_pause() -> Result<(), String> {
    let index_js = find_discord_core_index_js().ok_or("Файл Discord index.js не найден")?;
    let bak_path = index_js.with_extension("js.bak");

    if !bak_path.exists() {
        if let Ok(content) = fs::read_to_string(&index_js) {
            let _ = fs::write(&bak_path, content);
        }
    }

    fs::write(&index_js, PATCH_CODE).map_err(|e| format!("Ошибка записи index.js: {}", e))?;
    Ok(())
}

pub fn uninstall_anti_pause() -> Result<(), String> {
    let index_js = find_discord_core_index_js().ok_or("Файл Discord index.js не найден")?;
    let bak_path = index_js.with_extension("js.bak");

    if bak_path.exists() {
        if let Ok(content) = fs::read_to_string(&bak_path) {
            fs::write(&index_js, content).map_err(|e| format!("Ошибка восстановления: {}", e))?;
            let _ = fs::remove_file(bak_path);
            return Ok(());
        }
    }

    fs::write(&index_js, "module.exports = require('./core.asar');\n")
        .map_err(|e| format!("Ошибка записи: {}", e))?;
    Ok(())
}

