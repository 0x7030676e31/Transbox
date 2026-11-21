mod adb;
mod assets;
mod browser;
mod proxy;

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::{
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(
    version,
    about = "A portable browser using an Android phone's Internet over USB ADB"
)]
struct Args {
    /// Select a particular USB-connected Android device.
    #[arg(long)]
    device: Option<String>,
    /// List devices and exit.
    #[arg(long)]
    list_devices: bool,
    /// Development: use an unpacked runtime instead of the embedded bundle.
    #[arg(long)]
    runtime: Option<PathBuf>,
    /// Development: override the bundled ADB executable.
    #[arg(long)]
    adb: Option<PathBuf>,
    /// Development: override the bundled Chromium executable.
    #[arg(long)]
    browser: Option<PathBuf>,
    /// Initial page; restricted to HTTP(S).
    #[arg(long, default_value = "https://example.com")]
    url: String,
    /// Run the USB SOCKS5 proxy without launching a browser.
    #[arg(long)]
    proxy_only: bool,
    /// Testing: stop automatically after this many seconds.
    #[arg(long)]
    duration: Option<u64>,
    /// Testing: launch Chromium headlessly and print the page DOM.
    #[arg(long)]
    headless: bool,
}

fn run(args: Args) -> Result<()> {
    if !(args.url.starts_with("https://") || args.url.starts_with("http://")) {
        bail!("--url must start with https:// or http://");
    }
    let running = Arc::new(AtomicBool::new(true));
    let signal_running = running.clone();
    ctrlc::set_handler(move || {
        signal_running.store(false, Ordering::Relaxed);
    })?;
    // Listing can use a system ADB without extracting the browser.
    if args.list_devices
        && let Some(executable) = args.adb
    {
        adb::select(executable, args.device.as_deref(), true)?;
        return Ok(());
    }
    let runtime = assets::Runtime::open(args.runtime.as_deref())?;
    let executable = args.adb.unwrap_or_else(|| runtime.adb_path());
    let Some(adb) = adb::select(executable, args.device.as_deref(), args.list_devices)? else {
        return Ok(());
    };
    let mut token = [0; 32];
    getrandom::fill(&mut token).map_err(|e| anyhow::anyhow!("generating session token: {e}"))?;
    let token_hex: String = token.iter().map(|b| format!("{b:02x}")).collect();
    let session = adb::PhoneSession::start(adb, &runtime.root, &token_hex)?;
    let port = proxy::start(session.port, token, running.clone())?;
    eprintln!(
        "Connected. Browser traffic uses the phone's selected network (Wi-Fi or mobile data)."
    );
    eprintln!("Prototype: proxy routing only; Windows firewall isolation is not implemented.");
    println!("TRANSBOX_PROXY socks5h://127.0.0.1:{port}");
    std::io::stdout().flush()?;
    let profile = tempfile::Builder::new()
        .prefix("transbox-profile-")
        .tempdir()?;
    let mut child = if args.proxy_only {
        None
    } else {
        Some(browser::launch(
            &args.browser.unwrap_or_else(|| runtime.browser_path()),
            profile.path(),
            port,
            &args.url,
            args.headless,
        )?)
    };
    let start = Instant::now();
    let mut last_health = Instant::now();
    let mut lost_connection = false;
    eprintln!("Close the browser or press Ctrl+C to end the session.");
    while running.load(Ordering::Relaxed) {
        if args
            .duration
            .is_some_and(|seconds| start.elapsed() >= Duration::from_secs(seconds))
        {
            break;
        }
        if let Some(child) = &mut child
            && let Some(status) = child.try_wait().context("checking browser process")?
        {
            eprintln!("Browser exited ({status}).");
            break;
        }
        if last_health.elapsed() >= Duration::from_secs(2) {
            if let Err(error) = proxy::heartbeat(session.port, &token) {
                eprintln!("Phone tunnel lost: {error}. Stopping the browser.");
                lost_connection = true;
                break;
            }
            last_health = Instant::now();
        }
        thread::sleep(Duration::from_millis(100));
    }
    running.store(false, Ordering::Relaxed);
    drop(child); // Stop Chromium descendants before deleting their profile.
    drop(session);
    if lost_connection {
        bail!("USB tunnel disconnected; no direct fallback was attempted");
    }
    eprintln!("Session ended.");
    Ok(())
}

fn main() {
    let args = Args::parse();
    if let Err(error) = run(args) {
        eprintln!("Transbox: {error:#}");
        // Keep double-clicked Windows launch errors visible.
        #[cfg(windows)]
        if std::env::args_os().len() == 1 {
            eprintln!("Press Enter to close.");
            let _ = std::io::stdin().read_line(&mut String::new());
        }
        std::process::exit(1);
    }
}
