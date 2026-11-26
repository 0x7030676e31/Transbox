#![cfg_attr(windows, windows_subsystem = "windows")]
mod adb;
mod assets;
mod browser;
mod gui;
mod instance;
mod proxy;
mod session;

use anyhow::{Result, bail};
use clap::Parser;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Parser)]
#[command(
    version,
    about = "A portable browser using an Android phone's Internet over USB ADB"
)]
pub struct Args {
    /// Select a particular USB-connected Android device.
    #[arg(long)]
    device: Option<String>,
    /// List devices and exit without unlocking browser storage.
    #[arg(long)]
    list_devices: bool,
    /// Directory for this machine's encrypted profile (independent of runtime cache).
    #[arg(long)]
    data_dir: Option<PathBuf>,
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
    /// Testing: run without loading or saving the encrypted profile.
    #[arg(long)]
    ephemeral: bool,
    /// Run only the USB SOCKS5 proxy; does not access stored browser data.
    #[arg(long)]
    proxy_only: bool,
    /// Testing: stop automatically after this many seconds.
    #[arg(long)]
    duration: Option<u64>,
    /// Testing: ephemeral headless Chromium, printing the page DOM.
    #[arg(long)]
    headless: bool,
}

fn run(args: Args) -> Result<()> {
    if !(args.url.starts_with("https://") || args.url.starts_with("http://")) {
        bail!("--url must start with https:// or http://");
    }
    if args.list_devices {
        let executable = if let Some(path) = args.adb {
            path
        } else {
            assets::Runtime::open(args.runtime.as_deref())?.adb_path()
        };
        adb::select(executable, args.device.as_deref(), true)?;
        return Ok(());
    }
    let running = Arc::new(AtomicBool::new(true));
    let signal_running = running.clone();
    ctrlc::set_handler(move || signal_running.store(false, Ordering::Relaxed))?;
    if args.ephemeral || args.headless || args.proxy_only {
        let profile = tempfile::Builder::new()
            .prefix("transbox-ephemeral-")
            .tempdir()?;
        let outcome = session::run(
            &args,
            profile.path(),
            running,
            Arc::new(AtomicBool::new(false)),
            |text| eprintln!("{text}"),
            || {},
        );
        outcome.result
    } else {
        gui::run(args, running)
    }
}

fn main() {
    let args = Args::parse();
    let graphical = !(args.list_devices || args.ephemeral || args.headless || args.proxy_only);
    if let Err(error) = run(args) {
        eprintln!("Transbox: {error:#}");
        #[cfg(windows)]
        if graphical {
            use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
            let text: Vec<u16> = format!("{error:#}").encode_utf16().chain(Some(0)).collect();
            let title: Vec<u16> = "Transbox".encode_utf16().chain(Some(0)).collect();
            unsafe {
                MessageBoxW(
                    std::ptr::null_mut(),
                    text.as_ptr(),
                    title.as_ptr(),
                    MB_OK | MB_ICONERROR,
                );
            }
        }
        let _ = graphical;
        std::process::exit(1);
    }
}
