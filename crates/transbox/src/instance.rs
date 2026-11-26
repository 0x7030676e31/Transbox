//! Focus-only IPC; no password, decrypted data, or arbitrary commands cross it.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub struct Control {
    pub focus: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    descriptor: PathBuf,
}

impl Control {
    pub fn start(directory: &Path) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let mut token = [0; 32];
        getrandom::fill(&mut token)
            .map_err(|e| anyhow::anyhow!("Generating controller token: {e}"))?;
        let token: String = token.iter().map(|b| format!("{b:02x}")).collect();
        let descriptor = directory.join("controller.json");
        transbox_storage::atomic_write(
            &descriptor,
            &serde_json::to_vec(&serde_json::json!({
                "port": listener.local_addr()?.port(), "token": token, "pid": std::process::id()
            }))?,
        )?;
        let focus = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let (pending, shutdown) = (focus.clone(), stopped.clone());
        thread::spawn(move || {
            let expected = format!("FOCUS {token}\n");
            while !shutdown.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                        let mut request = vec![0; expected.len()];
                        if stream.read_exact(&mut request).is_ok() && request == expected.as_bytes()
                        {
                            pending.store(true, Ordering::Relaxed);
                            let _ = stream.write_all(b"OK\n");
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(30))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            focus,
            stopped,
            descriptor,
        })
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        let _ = fs::remove_file(&self.descriptor);
    }
}

pub fn focus_existing(directory: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let attempt = (|| -> Result<()> {
            let bytes = fs::read(directory.join("controller.json"))?;
            ensure!(bytes.len() < 4096, "Invalid controller descriptor");
            let info: serde_json::Value = serde_json::from_slice(&bytes)?;
            let port: u16 = info["port"]
                .as_u64()
                .context("Missing controller port")?
                .try_into()?;
            let token = info["token"].as_str().context("Missing controller token")?;
            ensure!(
                token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid controller token"
            );
            #[cfg(windows)]
            if let Some(pid) = info["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()) {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid);
                }
            }
            let mut stream = TcpStream::connect_timeout(
                &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
                Duration::from_millis(300),
            )?;
            stream.set_read_timeout(Some(Duration::from_secs(1)))?;
            stream.write_all(format!("FOCUS {token}\n").as_bytes())?;
            let mut ack = [0; 3];
            stream.read_exact(&mut ack)?;
            ensure!(
                &ack == b"OK\n",
                "Controller did not acknowledge focus request"
            );
            Ok(())
        })();
        if attempt.is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return attempt.context("Transbox is already running, but could not focus its window");
        }
        thread::sleep(Duration::from_millis(75));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn second_launcher_can_only_request_focus_and_descriptor_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let control = Control::start(dir.path()).unwrap();
        focus_existing(dir.path()).unwrap();
        assert!(control.focus.load(Ordering::Relaxed));
        drop(control);
        assert!(!dir.path().join("controller.json").exists());
    }
}
