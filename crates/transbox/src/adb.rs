use anyhow::{Context, Result, bail};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

#[derive(Clone)]
pub struct Adb {
    pub executable: PathBuf,
    transport_id: u64,
}

pub fn command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    // A remote ADB server would invalidate the promise of a local USB path.
    command.args(["-H", "127.0.0.1", "-P", "5037"]);
    #[cfg(windows)]
    let command = {
        let mut command = command;
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW: no console per ADB command.
        command
    };
    command
}

pub fn select(
    executable: PathBuf,
    requested: Option<&str>,
    list_only: bool,
) -> Result<Option<Adb>> {
    let output = command(&executable)
        .args(["devices", "-l"])
        .output()
        .context("starting ADB; ensure the bundled ADB files and USB driver are present")?;
    if !output.status.success() {
        bail!("ADB failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    if list_only {
        print!("{listing}");
        return Ok(None);
    }
    let mut devices = Vec::new();
    for line in listing.lines().skip(1) {
        let mut fields = line.split_whitespace();
        if let (Some(serial), Some(state)) = (fields.next(), fields.next())
            && requested.is_none_or(|wanted| wanted == serial)
        {
            devices.push((serial, state, line));
        }
    }
    if devices.is_empty() {
        bail!(
            "No matching Android device. Connect USB, enable USB debugging, and accept the prompt on the phone.\n{listing}"
        );
    }
    if devices.len() != 1 {
        bail!("Multiple devices found. Select one with --device SERIAL.\n{listing}");
    }
    let (_, state, line) = devices[0];
    if state != "device" {
        bail!("Device is {state}. Unlock the phone and accept USB debugging.\n{line}");
    }
    let adb = Adb {
        executable,
        transport_id: line
            .split_whitespace()
            .find_map(|field| field.strip_prefix("transport_id:"))
            .context("ADB did not report a transport ID; update Platform Tools")?
            .parse()?,
    };
    // Refuse wireless ADB and emulators: this application promises a USB path.
    if !line
        .split_whitespace()
        .any(|field| field.starts_with("usb:"))
    {
        // Windows' native USB backend does not supply a bus path. Query the
        // actual transport type rather than guessing from the device serial.
        if !usb_transport_snapshot(adb.transport_id)? {
            bail!(
                "The selected device is not connected through USB. Wireless ADB is not supported."
            );
        }
    }
    eprintln!("USB device: {line}");
    Ok(Some(adb))
}

impl Adb {
    pub fn cmd(&self) -> Command {
        let mut cmd = command(&self.executable);
        // Pin the transport so USB loss can never switch us to wireless ADB.
        cmd.args(["-t", &self.transport_id.to_string()]);
        cmd
    }

    pub fn run(&self, args: &[&str]) -> Result<String> {
        let output = self.cmd().args(args).output().context("running ADB")?;
        if !output.status.success() {
            bail!(
                "ADB {} failed: {}",
                args.first().unwrap_or(&"command"),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
}

fn usb_transport_snapshot(transport_id: u64) -> Result<bool> {
    let mut stream =
        TcpStream::connect_timeout(&"127.0.0.1:5037".parse()?, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let service = "host:track-devices-proto-text";
    stream.write_all(format!("{:04x}{service}", service.len()).as_bytes())?;
    let mut status = [0; 4];
    stream.read_exact(&mut status)?;
    if &status != b"OKAY" {
        bail!("ADB cannot verify USB transport type; use the bundled Platform Tools");
    }
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = usize::from_str_radix(std::str::from_utf8(&length)?, 16)?;
    let mut snapshot = vec![0; length];
    stream.read_exact(&mut snapshot)?;
    Ok(snapshot_has_usb_transport(
        std::str::from_utf8(&snapshot)?,
        transport_id,
    ))
}

fn snapshot_has_usb_transport(snapshot: &str, transport_id: u64) -> bool {
    snapshot.split("device {").skip(1).any(|record| {
        let record = record.split('}').next().unwrap_or("");
        let fields: Vec<_> = record
            .lines()
            .filter_map(|line| line.trim().split_once(':'))
            .collect();
        fields
            .iter()
            .any(|(key, value)| *key == "connection_type" && value.trim() == "USB")
            && fields.iter().any(|(key, value)| {
                *key == "transport_id" && value.trim().parse::<u64>() == Ok(transport_id)
            })
    })
}

pub struct PhoneSession {
    pub adb: Adb,
    pub port: u16,
    remote_path: String,
    forward: Option<String>,
    child: Option<Child>,
}

impl PhoneSession {
    pub fn start(adb: Adb, runtime: &Path, token: &str) -> Result<Self> {
        let abi = adb.run(&["shell", "getprop", "ro.product.cpu.abi"])?;
        if !["arm64-v8a", "armeabi-v7a", "x86_64", "x86"].contains(&abi.as_str()) {
            bail!("Unsupported Android CPU ABI: {abi}");
        }
        let relay = runtime.join("relays").join(&abi).join("transbox-relay");
        if !relay.is_file() {
            bail!("No relay for {abi} in {}", relay.display());
        }
        // Names and token are generated hex; never interpolate user input into an Android shell.
        let remote_path = format!("/data/local/tmp/transbox-{}", &token[..16]);
        let mut session = Self {
            adb,
            port: 0,
            remote_path,
            forward: None,
            child: None,
        };
        eprintln!("Starting Rust relay on Android ({abi})…");
        session.adb.run(&[
            "push",
            relay.to_str().context("relay path must be UTF-8")?,
            &session.remote_path,
        ])?;
        session
            .adb
            .run(&["shell", "chmod", "700", &session.remote_path])?;
        let mut child = session
            .adb
            .cmd()
            .args(["shell", "-T", &session.remote_path, token])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("launching Android relay")?;
        let stdout = child.stdout.take().context("relay stdout unavailable")?;
        session.child = Some(child);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let result = reader.read_line(&mut line).map(|_| line);
            let _ = sender.send(result);
            // Keep draining until ADB closes stdout; do not break the remote process's pipe.
            let _ = std::io::copy(&mut reader, &mut std::io::sink());
        });
        let ready = receiver
            .recv_timeout(Duration::from_secs(12))
            .context("Android relay did not become ready in 12 seconds")??;
        let remote_port: u16 = ready
            .trim()
            .strip_prefix("TRANSBOX_READY ")
            .context("unexpected relay startup response")?
            .parse()
            .context("invalid relay port")?;
        let forward = session
            .adb
            .run(&["forward", "tcp:0", &format!("tcp:{remote_port}")])?;
        session.port = forward
            .parse()
            .context("ADB returned an invalid forwarding port")?;
        session.forward = Some(format!("tcp:{}", session.port));
        Ok(session)
    }
}

impl Drop for PhoneSession {
    fn drop(&mut self) {
        // Do not kill the ADB server: scrcpy or other tools may be using it.
        if let Some(forward) = &self.forward {
            let _ = self.adb.run(&["forward", "--remove", forward]);
        }
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        // If disconnected, the relay's heartbeat lease terminates it within 15 seconds.
        let _ = self.adb.run(&["shell", "rm", "-f", &self.remote_path]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_usb_without_a_bus_path_but_never_socket_transport() {
        let snapshot = "device {\n serial: \"phone\"\n connection_type: USB\n transport_id: 7\n}\n\
                        device {\n serial: \"phone-over-wifi\"\n connection_type: SOCKET\n transport_id: 8\n}\n";
        assert!(snapshot_has_usb_transport(snapshot, 7));
        assert!(!snapshot_has_usb_transport(snapshot, 8));
        assert!(!snapshot_has_usb_transport(snapshot, 9));
    }
}
