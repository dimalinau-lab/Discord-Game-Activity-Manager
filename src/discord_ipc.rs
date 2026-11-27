use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::sync::mpsc::{self, Sender, SyncSender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use serde_json::json;

enum IpcCommand {
    SetActivity {
        app_id: String,
        details: String,
        state: String,
        pid: Option<u32>,
    },
    Clear,
    ClearAndAck(SyncSender<()>),
}

/// Асинхронный клиент Discord Rich Presence: все операции named pipe выполняются в фоновом потоке,
/// чтобы клики по интерфейсу не вызывали подвисаний UI.
#[derive(Clone)]
pub struct DiscordIpc {
    tx: Sender<IpcCommand>,
}

struct IpcWorker {
    pipe: Option<File>,
    current_app_id: Option<String>,
}

impl DiscordIpc {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<IpcCommand>();
        thread::spawn(move || {
            let mut worker = IpcWorker {
                pipe: None,
                current_app_id: None,
            };
            while let Ok(mut cmd) = rx.recv() {
                // Схлопываем очередь до последней актуальной команды, но сохраняем ClearAndAck
                while let Ok(next) = rx.try_recv() {
                    if let IpcCommand::ClearAndAck(ack) = cmd {
                        worker.clear_activity();
                        let _ = ack.send(());
                    }
                    cmd = next;
                }
                match cmd {
                    IpcCommand::SetActivity {
                        app_id,
                        details,
                        state,
                        pid,
                    } => worker.set_activity(&app_id, &details, &state, pid),
                    IpcCommand::Clear => worker.clear_activity(),
                    IpcCommand::ClearAndAck(ack) => {
                        worker.clear_activity();
                        let _ = ack.send(());
                    }
                }
            }
        });
        Self { tx }
    }

    pub fn set_activity(&mut self, app_id: &str, details: &str, state: &str, pid: Option<u32>) {
        if app_id.is_empty() {
            return;
        }
        let _ = self.tx.send(IpcCommand::SetActivity {
            app_id: app_id.to_string(),
            details: details.to_string(),
            state: state.to_string(),
            pid,
        });
    }

    pub fn clear_activity(&mut self) {
        let _ = self.tx.send(IpcCommand::Clear);
    }

    pub fn clear_activity_and_wait(&self, timeout: Duration) {
        let (ack_tx, ack_rx) = mpsc::sync_channel(1);
        if self.tx.send(IpcCommand::ClearAndAck(ack_tx)).is_ok() {
            let _ = ack_rx.recv_timeout(timeout);
        }
    }
}

impl IpcWorker {
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
                })
                .to_string();

                if Self::write_frame(&mut pipe, 0, &handshake).is_ok() {
                    let mut header = [0u8; 8];
                    if pipe.read_exact(&mut header).is_ok() {
                        let len =
                            u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
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

    fn set_activity(&mut self, app_id: &str, details: &str, state: &str, pid: Option<u32>) {
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

            let process_pid = pid.unwrap_or_else(std::process::id);

            let payload = json!({
                "cmd": "SET_ACTIVITY",
                "args": {
                    "pid": process_pid,
                    "activity": {
                        "details": details,
                        "state": state,
                        "timestamps": {
                            "start": start_ts
                        },
                        "instance": true
                    }
                },
                "nonce": format!("{}", start_ts)
            })
            .to_string();

            if Self::write_frame(pipe, 1, &payload).is_ok() {
                let mut header = [0u8; 8];
                if pipe.read_exact(&mut header).is_ok() {
                    let len =
                        u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
                    let mut buf = vec![0u8; len];
                    let _ = pipe.read_exact(&mut buf);
                }
            }
        }
    }

    fn clear_activity(&mut self) {
        if let Some(ref mut pipe) = self.pipe {
            let payload = json!({
                "cmd": "SET_ACTIVITY",
                "args": {
                    "pid": std::process::id(),
                    "activity": null
                },
                "nonce": "clear"
            })
            .to_string();

            let _ = Self::write_frame(pipe, 1, &payload);
        }
        self.disconnect();
    }

    fn disconnect(&mut self) {
        self.pipe = None;
        self.current_app_id = None;
    }
}

