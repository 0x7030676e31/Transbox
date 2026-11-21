//! Runs as an unprivileged ADB shell executable. No APK, root, or VPN required.
use std::io::{self, Write};
use std::net::TcpListener;
use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};
use transbox_core::{CONNECT, HEARTBEAT};

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: transbox-relay <64-character session token>");
        std::process::exit(2);
    }
    let token = transbox_core::decode_token(&args[1])?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    println!("TRANSBOX_READY {}", listener.local_addr()?.port());
    io::stdout().flush()?;
    let start = Instant::now();
    let last_heartbeat = Arc::new(AtomicU64::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    loop {
        // Even if the USB connection dies without delivering a signal, exit in 15 seconds.
        if start
            .elapsed()
            .as_secs()
            .saturating_sub(last_heartbeat.load(Ordering::Relaxed))
            > 15
        {
            eprintln!("Session lease expired; relay exiting.");
            return Ok(());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                if active.load(Ordering::Relaxed) >= 256 {
                    continue;
                }
                active.fetch_add(1, Ordering::Relaxed);
                let active = active.clone();
                let last_heartbeat = last_heartbeat.clone();
                thread::spawn(move || {
                    if let Ok(operation) = transbox_core::accept_auth(&mut stream, &token) {
                        match operation {
                            HEARTBEAT => {
                                last_heartbeat.store(start.elapsed().as_secs(), Ordering::Relaxed);
                            }
                            CONNECT => {
                                let _ = transbox_core::serve_socks(stream);
                            }
                            _ => unreachable!(),
                        }
                    }
                    active.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20))
            }
            Err(e) => return Err(e),
        }
    }
}
