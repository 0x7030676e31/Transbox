use crate::{Args, adb, assets, browser, proxy};
use anyhow::{Context, Result, ensure};
use std::{
    io::Write,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub struct Outcome {
    pub result: Result<()>,
    pub safe_to_save: bool,
}

/// All windows/renderer descendants belong to one browser, proxy, and profile.
pub fn run(
    args: &Args,
    profile: &Path,
    running: Arc<AtomicBool>,
    focus: Arc<AtomicBool>,
    progress: impl Fn(&str),
    started: impl Fn(),
) -> Outcome {
    let mut browser = None;
    let result = (|| -> Result<()> {
        progress("Preparing bundled browser…");
        let runtime = assets::Runtime::open(args.runtime.as_deref())?;
        progress("Connecting to your phone…");
        let executable = args.adb.clone().unwrap_or_else(|| runtime.adb_path());
        let adb = adb::select(executable, args.device.as_deref(), false)?
            .context("No device selected")?;
        let mut token = [0; 32];
        getrandom::fill(&mut token)
            .map_err(|e| anyhow::anyhow!("Generating session token: {e}"))?;
        let token_hex: String = token.iter().map(|b| format!("{b:02x}")).collect();
        let phone = adb::PhoneSession::start(adb, &runtime.root, &token_hex)?;
        let proxy_running = Arc::new(AtomicBool::new(true));
        struct StopProxy(Arc<AtomicBool>);
        impl Drop for StopProxy {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Relaxed);
            }
        }
        let _stop_proxy = StopProxy(proxy_running.clone());
        let port = proxy::start(phone.port, token, proxy_running.clone())?;
        ensure!(running.load(Ordering::Relaxed), "Session cancelled");
        println!("TRANSBOX_PROXY socks5h://127.0.0.1:{port}");
        std::io::stdout().flush()?;
        if !args.proxy_only {
            progress("Opening your browser…");
            browser = Some(browser::launch(
                &args
                    .browser
                    .clone()
                    .unwrap_or_else(|| runtime.browser_path()),
                profile,
                port,
                &args.url,
                args.headless,
            )?);
        }
        started();
        let start = Instant::now();
        let mut last_health = Instant::now();
        let monitored = (|| -> Result<()> {
            while running.load(Ordering::Relaxed) && proxy_running.load(Ordering::Relaxed) {
                if args
                    .duration
                    .is_some_and(|seconds| start.elapsed() >= Duration::from_secs(seconds))
                {
                    break;
                }
                if let Some(browser) = &mut browser {
                    if focus.swap(false, Ordering::Relaxed) {
                        browser.focus();
                    }
                    if browser.is_finished()? {
                        break;
                    }
                }
                if last_health.elapsed() >= Duration::from_secs(2) {
                    proxy::heartbeat(phone.port, &token).context("Phone tunnel disconnected. Browser session is closing; no direct fallback is used.")?;
                    last_health = Instant::now();
                }
                thread::sleep(Duration::from_millis(100));
            }
            Ok(())
        })();
        // Retain runtime, phone, and proxy until all browser descendants close.
        progress("Closing browser windows…");
        if let Some(browser) = &mut browser {
            browser.shutdown()?;
        }
        monitored
    })();
    // Error paths before/after the monitor still settle all processes before save.
    let settled = browser
        .as_mut()
        .map(|browser| browser.shutdown())
        .transpose();
    match settled {
        Ok(_) => Outcome {
            result,
            safe_to_save: true,
        },
        Err(error) => Outcome {
            result: Err(error),
            safe_to_save: false,
        },
    }
}
