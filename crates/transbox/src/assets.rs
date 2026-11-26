use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    io::Cursor,
    path::{Path, PathBuf},
};

include!(concat!(env!("OUT_DIR"), "/bundle.rs"));

pub struct Runtime {
    pub root: PathBuf,
}

impl Runtime {
    pub fn open(external: Option<&Path>) -> Result<Self> {
        if let Some(root) = external {
            return Ok(Self {
                root: root
                    .canonicalize()
                    .context("runtime directory does not exist")?,
            });
        }
        if BUNDLE.is_empty() {
            bail!(
                "This development build has no embedded runtime. Pass --runtime PATH or build with scripts/build.py."
            );
        }
        // ADB's shared server may outlive this session and keep adb.exe open on Windows.
        // A content-addressed cache avoids orphaned temp directories on every launch.
        let base = std::env::var_os("TRANSBOX_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os(if cfg!(windows) {
                    "LOCALAPPDATA"
                } else {
                    "XDG_CACHE_HOME"
                })
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("Transbox/runtime")
            });
        std::fs::create_dir_all(&base)?;
        let digest: String = Sha256::digest(BUNDLE)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let root = base.join(&digest[..24]);
        if root.join(".complete").is_file() {
            return Ok(Self { root });
        }
        eprintln!("Unpacking bundled browser and ADB (first launch only)…");
        let directory = tempfile::Builder::new()
            .prefix("unpacking-")
            .tempdir_in(&base)?;
        let mut archive =
            zip::ZipArchive::new(Cursor::new(BUNDLE)).context("invalid embedded runtime")?;
        archive
            .extract(directory.path())
            .context("extracting bundled runtime")?;
        std::fs::write(directory.path().join(".complete"), digest)?;
        if let Err(error) = std::fs::rename(directory.path(), &root) {
            // Another launcher may have finished the same immutable payload concurrently.
            if !root.join(".complete").is_file() {
                return Err(error).context("publishing extracted runtime");
            }
        }
        Ok(Self { root })
    }

    pub fn adb_path(&self) -> PathBuf {
        self.root
            .join("adb")
            .join(if cfg!(windows) { "adb.exe" } else { "adb" })
    }

    pub fn browser_path(&self) -> PathBuf {
        self.root.join("browser").join(if cfg!(windows) {
            "chrome.exe"
        } else {
            "chrome"
        })
    }
}
