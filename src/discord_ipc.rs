use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};
use serde_json::json;

pub struct DiscordIpc {
    pipe: Option<File>,
    current_app_id: Option<String>,
}

impl DiscordIpc {
    pub fn new() -> Self {
        Self {
            pipe: None,
            current_app_id: None,
        }
    }

    fn connect(&mut self, app_id: &str) -> bool {
        self.disconnect();

        for i in 0..10 {
            let pipe_name = format!(r"\\.\pipe\discord-ipc-{}", i);
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&pipe_name);

            if let Ok(mut pipe) = file {
                let handshake = json!({
                    "v": 1,
                    "client_id": app_id
                }).to_string();

                if Self::write_frame(&mut pipe, 0, &handshake).is_ok() {
                    let mut header = [0u8; 8];
                    if pipe.read_exact(&mut header).is_ok() {
                        let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
                        let mut buf = vec![0u8; len];
                        let _ = pipe.read_exact(&mut buf);
                        self.pipe = Some(pipe);
                        self.current_app_id = Some(app_id.to_string());
                        return true;
                    }
                }
            }
        }
        false
    }

    fn write_frame(pipe: &mut File, op: u32, payload: &str) -> std::io::Result<()> {
        let bytes = payload.as_bytes();
        let len = bytes.len() as u32;
        pipe.write_all(&op.to_le_bytes())?;
        pipe.write_all(&len.to_le_bytes())?;
        pipe.write_all(bytes)?;
        pipe.flush()?;
        Ok(())
    }

    pub fn set_activity(&mut self, app_id: &str, details: &str, state: &str) {
        if app_id.is_empty() {
            return;
        }

        if self.current_app_id.as_deref() != Some(app_id) || self.pipe.is_none() {
            if !self.connect(app_id) {
                return;
            }
        }

        if let Some(ref mut pipe) = self.pipe {
            let start_ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);

            let payload = json!({
                "cmd": "SET_ACTIVITY",
                "args": {
                    "pid": std::process::id(),
                    "activity": {
                        "details": details,
                        "state": state,
                        "timestamps": {
                            "start": start_ts
                        }
                    }
                },
                "nonce": format!("{}", start_ts)
            }).to_string();

            if Self::write_frame(pipe, 1, &payload).is_ok() {
                let mut header = [0u8; 8];
                if pipe.read_exact(&mut header).is_ok() {
                    let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
                    let mut buf = vec![0u8; len];
                    let _ = pipe.read_exact(&mut buf);
                }
            }
        }
    }

    pub fn clear_activity(&mut self) {
        if let Some(ref mut pipe) = self.pipe {
            let payload = json!({
                "cmd": "SET_ACTIVITY",
                "args": {
                    "pid": std::process::id(),
                    "activity": null
                },
                "nonce": "clear"
            }).to_string();

            let _ = Self::write_frame(pipe, 1, &payload);
        }
        self.disconnect();
    }

    pub fn disconnect(&mut self) {
        self.pipe = None;
        self.current_app_id = None;
    }
}

