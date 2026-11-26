use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

/// Merge required connection/privacy settings without throwing away user preferences.
pub fn configure(profile: &Path) -> Result<()> {
    fs::create_dir_all(profile.join("Default"))?;
    let path = profile.join("Default/Preferences");
    let mut preferences: Value = if path.exists() {
        serde_json::from_slice(&fs::read(&path)?).context("Reading saved browser preferences")?
    } else {
        json!({})
    };
    merge(
        &mut preferences,
        json!({
            "credentials_enable_service": true,
            "profile": { "password_manager_enabled": true,
                "default_content_setting_values": { "geolocation": 2, "notifications": 2,
                    "media_stream_mic": 2, "media_stream_camera": 2, "automatic_downloads": 2,
                    "usb_guard": 2, "serial_guard": 2, "hid_guard": 2, "bluetooth_guard": 2 } },
            "download": { "prompt_for_download": true },
            "dns_prefetching": { "enabled": false },
            "net": { "network_prediction_options": 2 },
            "webrtc": { "ip_handling_policy": "disable_non_proxied_udp" },
            "background_mode": { "enabled": false }
        }),
    );
    transbox_storage::atomic_write(&path, &serde_json::to_vec(&preferences)?)?;
    // This setting lives in Local State, not the profile Preferences, on Chromium.
    let state_path = profile.join("Local State");
    let mut state: Value = if state_path.exists() {
        serde_json::from_slice(&fs::read(&state_path)?)?
    } else {
        json!({})
    };
    merge(&mut state, json!({"background_mode": {"enabled": false}}));
    transbox_storage::atomic_write(&state_path, &serde_json::to_vec(&state)?)?;
    Ok(())
}

fn merge(target: &mut Value, changes: Value) {
    match (target, changes) {
        (Value::Object(target), Value::Object(changes)) => {
            for (key, value) in changes {
                merge(target.entry(key).or_insert(Value::Null), value);
            }
        }
        (target, value) => *target = value,
    }
}

pub fn command(executable: &Path, profile: &Path, port: u16, url: &str, headless: bool) -> Command {
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
    cmd.arg(url);
    cmd
}

pub fn launch(
    executable: &Path,
    profile: &Path,
    port: u16,
    url: &str,
    headless: bool,
) -> Result<BrowserProcess> {
    configure(profile)?;
    let mut cmd = command(executable, profile, port, url, headless);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Assign the job before Chromium can create children, eliminating the
        // startup race where a subprocess could escape controller cleanup.
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd
        .spawn()
        .with_context(|| format!("Launching browser at {}", executable.display()))?;
    let mut process = BrowserProcess {
        child,
        settled: false,
        #[cfg(windows)]
        job: None,
    };
    #[cfg(windows)]
    {
        use std::os::windows::{io::AsRawHandle, process::ChildExt};
        process.job = Some(BrowserJob::new(&process.child)?);
        unsafe {
            if windows_sys::Win32::System::Threading::ResumeThread(
                process.child.main_thread_handle().as_raw_handle(),
            ) == u32::MAX
            {
                return Err(std::io::Error::last_os_error().into());
            }
        }
    }
    let _ = &mut process;
    fs::write(
        profile.join(".transbox-process"),
        process.child.id().to_string(),
    )?;
    Ok(process)
}

pub struct BrowserProcess {
    child: Child,
    settled: bool,
    #[cfg(windows)]
    job: Option<BrowserJob>,
}

impl BrowserProcess {
    pub fn is_finished(&mut self) -> Result<bool> {
        // Reap the root, but keep the tunnel and vault lock for any descendants.
        let root_exited = self.child.try_wait()?.is_some();
        #[cfg(windows)]
        {
            Ok(self
                .job
                .as_ref()
                .is_none_or(|job| job.active().is_ok_and(|count| count == 0))
                && root_exited)
        }
        #[cfg(target_os = "linux")]
        {
            Ok(root_exited && !transbox_storage::live_process_group(self.child.id()))
        }
        #[cfg(all(unix, not(target_os = "linux")))]
        {
            Ok(root_exited)
        }
    }

    pub fn focus(&self) {
        #[cfg(windows)]
        windows_action(self.window_pids(), false);
        // Wayland compositors control focus; do not launch a second browser as a workaround.
    }

    pub fn shutdown(&mut self) -> Result<()> {
        if self.settled {
            return Ok(());
        }
        if !self.is_finished()? {
            #[cfg(windows)]
            windows_action(self.window_pids(), true);
            #[cfg(unix)]
            unsafe {
                libc::killpg(self.child.id() as libc::pid_t, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(4);
            while Instant::now() < deadline && !self.is_finished()? {
                thread::sleep(Duration::from_millis(50));
            }
        }
        if !self.is_finished()? {
            self.force_stop();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !self.is_finished()? {
            thread::sleep(Duration::from_millis(50));
        }
        if !self.is_finished()? {
            bail!(
                "Chromium is still using its profile. Close it before recovering the unsaved session."
            );
        }
        self.settled = true;
        Ok(())
    }

    fn force_stop(&mut self) {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            unsafe {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(job.0, 1);
            }
        }
        #[cfg(unix)]
        unsafe {
            libc::killpg(self.child.id() as libc::pid_t, libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    #[cfg(windows)]
    fn window_pids(&self) -> Vec<u32> {
        self.job
            .as_ref()
            .and_then(|job| job.pids().ok())
            .unwrap_or_else(|| vec![self.child.id()])
    }
}

impl Drop for BrowserProcess {
    fn drop(&mut self) {
        if !self.settled {
            self.force_stop();
        }
    }
}

#[cfg(windows)]
fn windows_action(pids: Vec<u32>, close: bool) {
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::*,
    };
    use windows_sys::core::BOOL;
    struct Request {
        pids: Vec<u32>,
        close: bool,
    }
    unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> BOOL {
        unsafe {
            let request = &*(data as *const Request);
            let mut pid = 0;
            GetWindowThreadProcessId(window, &mut pid);
            if request.pids.contains(&pid) && IsWindowVisible(window) != 0 {
                if request.close {
                    PostMessageW(window, WM_CLOSE, 0, 0);
                } else {
                    ShowWindow(window, SW_RESTORE);
                    SetForegroundWindow(window);
                }
            }
            1
        }
    }
    let request = Request { pids, close };
    unsafe {
        EnumWindows(Some(visit), &request as *const _ as LPARAM);
    }
}

#[cfg(windows)]
pub struct BrowserJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl BrowserJob {
    fn new(child: &Child) -> Result<Self> {
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
    fn active(&self) -> Result<u32> {
        use std::{
            mem::{size_of, zeroed},
            ptr::null_mut,
        };
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = zeroed();
            if QueryInformationJobObject(
                self.0,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut _,
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                null_mut(),
            ) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(info.ActiveProcesses)
        }
    }

    fn pids(&self) -> Result<Vec<u32>> {
        use std::{mem::size_of, ptr::null_mut};
        use windows_sys::Win32::{Foundation::ERROR_MORE_DATA, System::JobObjects::*};
        let mut capacity = 64;
        loop {
            // Vec<usize> provides the alignment required by the variable-size list.
            let bytes =
                size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() + (capacity - 1) * size_of::<usize>();
            let mut buffer = vec![0usize; bytes.div_ceil(size_of::<usize>())];
            unsafe {
                let list = buffer.as_mut_ptr() as *mut JOBOBJECT_BASIC_PROCESS_ID_LIST;
                if QueryInformationJobObject(
                    self.0,
                    JobObjectBasicProcessIdList,
                    list.cast(),
                    bytes as u32,
                    null_mut(),
                ) != 0
                {
                    let count = (*list).NumberOfProcessIdsInList as usize;
                    anyhow::ensure!(count <= capacity, "Invalid browser job process count");
                    let ids = std::slice::from_raw_parts((*list).ProcessIdList.as_ptr(), count);
                    return Ok(ids.iter().map(|pid| *pid as u32).collect());
                }
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_MORE_DATA as i32) || capacity >= 8192 {
                    return Err(error.into());
                }
            }
            capacity *= 2;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn tunnel_lifetime_includes_descendants_after_root_exits() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("browser-helper");
        fs::write(&executable, "#!/bin/sh\nsleep 5 &\nexit 0\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let profile = dir.path().join("profile");
        let mut browser =
            launch(&executable, &profile, 1234, "https://example.org", false).unwrap();
        browser.child.wait().unwrap();
        assert!(
            !browser.is_finished().unwrap(),
            "A descendant still owns the session"
        );
        assert!(transbox_storage::ensure_idle(&profile).is_err());
        browser.shutdown().unwrap();
        assert!(browser.is_finished().unwrap());
        assert!(transbox_storage::ensure_idle(&profile).is_ok());
    }

    #[cfg(windows)]
    #[test]
    #[allow(clippy::zombie_processes)] // Parent must exit first; the test's job owns cleanup.
    fn job_process_helper() {
        let Ok(mode) = std::env::var("TRANSBOX_JOB_TEST_MODE") else {
            return;
        };
        if mode == "child" {
            thread::sleep(Duration::from_secs(30));
        } else {
            // Use a real descendant without cmd.exe's special quoting and
            // console/window behavior. The job owns and cleans up this child.
            let _child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "browser::tests::job_process_helper",
                    "--nocapture",
                ])
                .env("TRANSBOX_JOB_TEST_MODE", "child")
                .spawn()
                .unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn suspended_job_tracks_descendants_after_root_exits() {
        use std::os::windows::{
            io::AsRawHandle,
            process::{ChildExt, CommandExt},
        };
        use windows_sys::Win32::System::{
            JobObjects::TerminateJobObject,
            Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED, ResumeThread},
        };
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "browser::tests::job_process_helper",
                "--nocapture",
            ])
            .env("TRANSBOX_JOB_TEST_MODE", "parent")
            .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        let mut browser = BrowserProcess {
            child,
            job: None,
            settled: false,
        };
        browser.job = Some(BrowserJob::new(&browser.child).unwrap());
        unsafe {
            assert_ne!(
                ResumeThread(browser.child.main_thread_handle().as_raw_handle()),
                u32::MAX
            );
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = browser.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "Suspended helper did not exit after resume"
            );
            thread::sleep(Duration::from_millis(20));
        };
        assert!(status.success());
        assert!(
            !browser.is_finished().unwrap(),
            "Child must retain the browser job"
        );
        assert!(!browser.window_pids().is_empty());
        unsafe {
            assert_ne!(TerminateJobObject(browser.job.as_ref().unwrap().0, 0), 0);
        }
        browser.shutdown().unwrap();
        assert!(browser.is_finished().unwrap());
    }

    #[test]
    fn preserves_saved_preferences_and_windows_share_one_proxy() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("Default")).unwrap();
        fs::write(
            dir.path().join("Default/Preferences"),
            br#"{"custom":{"setting":42},"profile":{"name":"My browser"}}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("Local State"),
            br#"{"os_crypt":{"encrypted_key":"keep"}}"#,
        )
        .unwrap();
        configure(dir.path()).unwrap();
        let p: Value =
            serde_json::from_slice(&fs::read(dir.path().join("Default/Preferences")).unwrap())
                .unwrap();
        assert_eq!(p["custom"]["setting"], 42);
        assert_eq!(p["profile"]["name"], "My browser");
        assert_eq!(p["profile"]["password_manager_enabled"], true);
        let state: Value =
            serde_json::from_slice(&fs::read(dir.path().join("Local State")).unwrap()).unwrap();
        assert_eq!(state["os_crypt"]["encrypted_key"], "keep");
        let cmd = command(
            Path::new("chromium"),
            dir.path(),
            1234,
            "https://example.org",
            false,
        );
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"--proxy-server=socks5://127.0.0.1:1234".to_owned()));
        assert!(!args.iter().any(|a| a == "--no-sandbox"));
    }
}
