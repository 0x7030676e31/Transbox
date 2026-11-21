use anyhow::{Context, Result};
use std::{
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

pub fn heartbeat(port: u16, token: &[u8; 32]) -> std::io::Result<()> {
    let mut stream = TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_secs(2),
    )?;
    transbox_core::authenticate(&mut stream, token, transbox_core::HEARTBEAT)
}

pub fn start(port: u16, token: [u8; 32], running: Arc<AtomicBool>) -> Result<u16> {
    heartbeat(port, &token).context("checking Android relay over USB")?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let local_port = listener.local_addr()?.port();
    let active = Arc::new(AtomicUsize::new(0));
    thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((client, _)) => {
                    if active.load(Ordering::Relaxed) >= 128 {
                        continue;
                    }
                    active.fetch_add(1, Ordering::Relaxed);
                    let active = active.clone();
                    thread::spawn(move || {
                        let result = (|| -> std::io::Result<()> {
                            let mut remote = TcpStream::connect_timeout(
                                &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
                                Duration::from_secs(2),
                            )?;
                            remote.set_nodelay(true)?;
                            client.set_nodelay(true)?;
                            transbox_core::authenticate(
                                &mut remote,
                                &token,
                                transbox_core::CONNECT,
                            )?;
                            transbox_core::bridge(client, remote)?;
                            Ok(())
                        })();
                        // Per-site failures are normal. Never retry through a desktop connection.
                        if let Err(error) = result
                            && !matches!(
                                error.kind(),
                                std::io::ErrorKind::ConnectionReset
                                    | std::io::ErrorKind::ConnectionAborted
                                    | std::io::ErrorKind::BrokenPipe
                            )
                        {
                            eprintln!("Tunnel stream ended: {error}");
                        }
                        active.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20))
                }
                Err(e) => {
                    eprintln!("Local proxy stopped: {e}");
                    running.store(false, Ordering::Relaxed);
                    break;
                }
            }
        }
    });
    Ok(local_port)
}
