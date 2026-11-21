use anyhow::{Context, Result};
use std::{
    fs,
    path::Path,
    process::{Child, Command, ExitStatus},
};

pub fn launch(
    executable: &Path,
    profile: &Path,
    port: u16,
    url: &str,
    headless: bool,
) -> Result<BrowserProcess> {
    fs::create_dir_all(profile.join("Default"))?;
    let preferences = serde_json::json!({
        "credentials_enable_service": false,
        "profile": {
            "password_manager_enabled": false,
            "default_content_setting_values": {
                "geolocation": 2, "notifications": 2, "media_stream_mic": 2,
                "media_stream_camera": 2, "automatic_downloads": 2,
                "usb_guard": 2, "serial_guard": 2, "hid_guard": 2, "bluetooth_guard": 2
            }
        },
        "download": { "prompt_for_download": true },
        "dns_prefetching": { "enabled": false },
        "net": { "network_prediction_options": 2 },
        "webrtc": { "ip_handling_policy": "disable_non_proxied_udp" },
        "session": { "restore_on_startup": 5 }
    });
    fs::write(
        profile.join("Default/Preferences"),
        serde_json::to_vec(&preferences)?,
    )?;
    let mut cmd = Command::new(executable);
    cmd.arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--proxy-server=socks5://127.0.0.1:{port}"))
        .args([
            "--proxy-bypass-list=<-loopback>",
            "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1",
            "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
            "--disable-quic", "--disable-background-networking", "--disable-component-update",
            "--disable-sync", "--disable-extensions", "--disable-default-apps",
            "--disable-domain-reliability", "--disable-breakpad",
            "--disable-features=DnsOverHttps,MediaRouter,OptimizationHints,AutofillServerCommunication",
            "--no-first-run", "--no-default-browser-check", "--new-window",
        ]);
    if headless {
        cmd.args(["--headless=new", "--dump-dom", "--timeout=15000"]);
    }
    // Deliberately preserve Chromium's own sandbox. Never pass --no-sandbox.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd
        .arg(url)
        .spawn()
        .with_context(|| format!("launching browser at {}", executable.display()))?;
    let mut process = BrowserProcess {
        child,
        #[cfg(windows)]
        _job: None,
    };
    #[cfg(windows)]
    {
        process._job =
            Some(BrowserJob::new(&process.child).context("attaching browser to cleanup job")?);
    }
    // Windows needs mutation to attach its job; Unix does not.
    let _ = &mut process;
    Ok(process)
}

pub struct BrowserProcess {
    child: Child,
    #[cfg(windows)]
    _job: Option<BrowserJob>,
}

impl BrowserProcess {
    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }
}

impl Drop for BrowserProcess {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::killpg(self.child.id() as libc::pid_t, libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Windows job is dropped next and terminates the remaining descendants.
    }
}

#[cfg(windows)]
pub struct BrowserJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl BrowserJob {
    pub fn new(child: &Child) -> Result<Self> {
        use std::{
            mem::{size_of, zeroed},
            os::windows::io::AsRawHandle,
            ptr::null,
        };
        use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
        unsafe {
            let handle = CreateJobObjectW(null(), null());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
                || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0
            {
                let error = std::io::Error::last_os_error();
                CloseHandle(handle);
                return Err(error.into());
            }
            Ok(Self(handle))
        }
    }
}

#[cfg(windows)]
impl Drop for BrowserJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
