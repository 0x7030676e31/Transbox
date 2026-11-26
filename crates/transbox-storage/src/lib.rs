//! A single-machine encrypted Chromium profile. Plaintext exists only in `working/`
//! during an unlocked session, or after an interrupted session awaiting recovery.
use anyhow::{Context, Result, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

const MAGIC: &[u8; 8] = b"TBXVLT01";
const HEADER: usize = 96;
const MAX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILES: usize = 100_000;
const AUTH_ERROR: &str = "Incorrect password or damaged storage.";

pub struct Store {
    root: PathBuf,
    _lock: File,
}

pub struct Unlocked {
    header: [u8; HEADER],
    key: Zeroizing<[u8; 32]>,
    snapshot: Zeroizing<Vec<u8>>,
    previous: Vec<u8>,
}

#[derive(Clone, Copy)]
pub enum Recovery {
    Saved,
    Unsaved,
}

pub enum OpenStore {
    Acquired(Store),
    Busy,
}

pub fn default_directory() -> Result<PathBuf> {
    if cfg!(windows) {
        Ok(
            PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is not set")?)
                .join("Transbox/storage"),
        )
    } else if let Some(base) = std::env::var_os("XDG_DATA_HOME") {
        Ok(PathBuf::from(base).join("transbox"))
    } else {
        Ok(
            PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?)
                .join(".local/share/transbox"),
        )
    }
}

pub fn private_directory(path: &Path) -> Result<()> {
    if path.exists() {
        ensure!(
            !is_link(&fs::symlink_metadata(path)?),
            "Storage directory must not be a link"
        );
    }
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("Storage path has no parent")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .context("Replacing storage file")?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

impl Store {
    pub fn open(root: &Path) -> Result<OpenStore> {
        private_directory(root)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(root.join("session.lock"))?;
        match lock.try_lock() {
            Ok(()) => Ok(OpenStore::Acquired(Self {
                root: root.canonicalize()?,
                _lock: lock,
            })),
            Err(TryLockError::WouldBlock) => Ok(OpenStore::Busy),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.root
    }
    pub fn exists(&self) -> bool {
        self.root.join("profile.vault").exists()
    }
    pub fn has_backup(&self) -> bool {
        self.root.join("profile.backup").exists()
    }
    pub fn needs_recovery(&self) -> bool {
        self.has_working_files() && !self.pending_cleanup()
    }
    pub fn has_working_files(&self) -> bool {
        self.workspace().exists()
    }
    pub fn pending_cleanup(&self) -> bool {
        self.root.join("cleanup.pending").exists()
    }
    pub fn workspace(&self) -> PathBuf {
        self.root.join("working")
    }

    pub fn create(&self, password: &str) -> Result<Unlocked> {
        ensure!(
            !self.exists() && !self.has_backup() && !self.has_working_files(),
            "Storage already exists; unlock or reset it first"
        );
        ensure!(
            password.chars().count() >= 12,
            "Use at least 12 characters for the password."
        );
        let mut header = [0; HEADER];
        header[..8].copy_from_slice(MAGIC);
        random(&mut header[8..48])?;
        let mut key = Zeroizing::new([0; 32]);
        random(key.as_mut())?;
        let wrapping_key = derive(password, &header[8..24])?;
        let wrapped = cipher(&wrapping_key)
            .encrypt(
                nonce(&header[24..48])?,
                Payload {
                    msg: key.as_ref(),
                    aad: &header[..24],
                },
            )
            .map_err(|_| anyhow::anyhow!("Encrypting storage key failed"))?;
        header[48..].copy_from_slice(&wrapped);
        let snapshot = Zeroizing::new(
            ZipWriter::new(Cursor::new(Vec::new()))
                .finish()?
                .into_inner(),
        );
        let mut unlocked = Unlocked {
            header,
            key,
            snapshot,
            previous: Vec::new(),
        };
        let bytes = unlocked.seal(&unlocked.snapshot)?;
        atomic_write(&self.root.join("profile.vault"), &bytes)?;
        unlocked.previous = bytes;
        Ok(unlocked)
    }

    pub fn unlock(&self, password: &str, backup: bool) -> Result<Unlocked> {
        let path = self.root.join(if backup {
            "profile.backup"
        } else {
            "profile.vault"
        });
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            !is_link(&metadata) && metadata.is_file(),
            "Storage must be a regular file"
        );
        ensure!(
            metadata.len() <= MAX_BYTES + 4096,
            "Storage exceeds the 512 MiB limit"
        );
        let bytes = fs::read(path)?;
        ensure!(
            bytes.len() >= HEADER + 24 + 16 && &bytes[..8] == MAGIC,
            "Unsupported or damaged storage format"
        );
        let header: [u8; HEADER] = bytes[..HEADER].try_into()?;
        let wrapping_key = derive(password, &header[8..24])?;
        let decoded = Zeroizing::new(
            cipher(&wrapping_key)
                .decrypt(
                    nonce(&header[24..48])?,
                    Payload {
                        msg: &header[48..],
                        aad: &header[..24],
                    },
                )
                .map_err(|_| anyhow::anyhow!(AUTH_ERROR))?,
        );
        ensure!(decoded.len() == 32, "{AUTH_ERROR}");
        let mut key = Zeroizing::new([0; 32]);
        key.copy_from_slice(&decoded);
        let snapshot = Zeroizing::new(
            cipher(&key)
                .decrypt(
                    nonce(&bytes[HEADER..HEADER + 24])?,
                    Payload {
                        msg: &bytes[HEADER + 24..],
                        aad: &header,
                    },
                )
                .map_err(|_| anyhow::anyhow!(AUTH_ERROR))?,
        );
        Ok(Unlocked {
            header,
            key,
            snapshot,
            previous: bytes,
        })
    }

    pub fn prepare(&self, unlocked: &Unlocked, recovery: Recovery) -> Result<PathBuf> {
        let work = self.workspace();
        ensure!(
            !matches!(recovery, Recovery::Unsaved) || !self.pending_cleanup(),
            "The previous session was already saved; use the saved copy to finish cleanup"
        );
        if work.exists() {
            ensure!(
                !is_link(&fs::symlink_metadata(&work)?),
                "Working profile must not be a link"
            );
            ensure_idle(&work)?;
            if matches!(recovery, Recovery::Unsaved) {
                return Ok(work);
            }
            fs::remove_dir_all(&work).context("Removing interrupted working profile")?;
        } else {
            ensure!(
                !matches!(recovery, Recovery::Unsaved),
                "No interrupted session to recover"
            );
        }
        remove_if_present(&self.root.join("cleanup.pending"))?;
        private_directory(&work)?;
        // Failed extraction deliberately leaves working/ as a recovery marker.
        extract(&unlocked.snapshot, &work)?;
        Ok(work)
    }

    pub fn save(&self, unlocked: &Unlocked) -> Result<()> {
        let work = self.workspace();
        ensure_idle(&work)?;
        let snapshot = archive(&work)?;
        let bytes = unlocked.seal(&snapshot)?;
        // Only a previously authenticated snapshot may become the backup.
        atomic_write(&self.root.join("profile.backup"), &unlocked.previous)?;
        atomic_write(&self.root.join("profile.vault"), &bytes)?;
        // Outside working/: a crash during recursive deletion must never offer
        // the partially deleted directory as an unsaved session to recover.
        atomic_write(
            &self.root.join("cleanup.pending"),
            &bytes[HEADER..HEADER + 24],
        )?;
        fs::remove_dir_all(work)
            .context("Saved encrypted profile, but could not remove the working files")?;
        remove_if_present(&self.root.join("cleanup.pending"))?;
        Ok(())
    }

    pub fn reset(&self) -> Result<()> {
        if self.workspace().exists() {
            ensure_idle(&self.workspace())?;
            if is_link(&fs::symlink_metadata(self.workspace())?) {
                fs::remove_file(self.workspace())?;
            } else {
                fs::remove_dir_all(self.workspace())?;
            }
        }
        for name in ["profile.vault", "profile.backup", "cleanup.pending"] {
            match fs::remove_file(self.root.join(name)) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

impl Unlocked {
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        ensure!(
            plaintext.len() as u64 <= MAX_BYTES,
            "Compressed profile exceeds the 512 MiB limit"
        );
        let mut nonce = [0; 24];
        random(&mut nonce)?;
        let encrypted = cipher(&self.key)
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: plaintext,
                    aad: &self.header,
                },
            )
            .map_err(|_| anyhow::anyhow!("Encrypting profile failed"))?;
        let mut result = Vec::with_capacity(HEADER + 24 + encrypted.len());
        result.extend_from_slice(&self.header);
        result.extend_from_slice(&nonce);
        result.extend_from_slice(&encrypted);
        Ok(result)
    }
}

fn random(bytes: &mut [u8]) -> Result<()> {
    getrandom::fill(bytes)
        .map_err(|error| anyhow::anyhow!("Secure random generation failed: {error}"))
}

fn nonce(bytes: &[u8]) -> Result<&XNonce> {
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid storage nonce"))
}

fn cipher(key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(key.into())
}

fn derive(password: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(64 * 1024, 3, 1, Some(32))
        .map_err(|e| anyhow::anyhow!("Argon2 parameters: {e}"))?;
    let mut key = Zeroizing::new([0; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|e| anyhow::anyhow!("Deriving storage key: {e}"))?;
    Ok(key)
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn excluded(name: &str) -> bool {
    matches!(
        name,
        "SingletonLock"
            | "SingletonCookie"
            | "SingletonSocket"
            | "DevToolsActivePort"
            | "Cache"
            | "Code Cache"
            | "GPUCache"
            | "ShaderCache"
            | "GrShaderCache"
            | "GraphiteDawnCache"
            | "DawnGraphiteCache"
            | "Crashpad"
            | "BrowserMetrics"
            | "component_crx_cache"
            | "extensions_crx_cache"
            | ".transbox-process"
    )
}

fn archive(root: &Path) -> Result<Zeroizing<Vec<u8>>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let mut bytes = 0;
    let mut files = 0;
    fn walk(
        root: &Path,
        path: &Path,
        writer: &mut ZipWriter<Cursor<Vec<u8>>>,
        bytes: &mut u64,
        files: &mut usize,
    ) -> Result<()> {
        let mut entries: Vec<_> = fs::read_dir(path)?.collect::<std::io::Result<_>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if entry.file_name().to_str().is_some_and(excluded) {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if is_link(&metadata) {
                continue;
            }
            if metadata.is_dir() {
                walk(root, &entry.path(), writer, bytes, files)?;
            } else if metadata.is_file() {
                *bytes = bytes
                    .checked_add(metadata.len())
                    .context("Profile size overflow")?;
                *files += 1;
                ensure!(
                    *bytes <= MAX_BYTES && *files <= MAX_FILES,
                    "Profile exceeds the 512 MiB / 100,000 file limit"
                );
                let path = entry.path();
                let name = path
                    .strip_prefix(root)?
                    .to_str()
                    .context("Profile path is not UTF-8")?
                    .replace('\\', "/");
                writer.start_file(
                    name,
                    SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated)
                        .unix_permissions(0o600),
                )?;
                std::io::copy(&mut File::open(path)?.take(metadata.len()), writer)?;
            }
        }
        Ok(())
    }
    walk(root, root, &mut writer, &mut bytes, &mut files)?;
    Ok(Zeroizing::new(writer.finish()?.into_inner()))
}

fn extract(bytes: &[u8], root: &Path) -> Result<()> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    ensure!(archive.len() <= MAX_FILES, "Too many profile files");
    let mut total = 0u64;
    let mut names = std::collections::HashSet::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        let relative = file
            .enclosed_name()
            .context("Invalid profile archive path")?;
        ensure!(names.insert(relative.clone()), "Duplicate profile path");
        ensure!(!file.is_symlink(), "Profile archive contains a link");
        total = total
            .checked_add(file.size())
            .context("Profile size overflow")?;
        ensure!(total <= MAX_BYTES, "Profile exceeds the 512 MiB limit");
        let output = root.join(relative);
        if file.is_dir() {
            private_directory(&output)?;
        } else {
            private_directory(output.parent().context("Invalid profile path")?)?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut target = options.open(output)?;
            std::io::copy(&mut file, &mut target)?;
        }
    }
    Ok(())
}

pub fn ensure_idle(work: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    if let Ok(marker) = fs::read_to_string(work.join(".transbox-process")) {
        let pgid: u32 = marker
            .trim()
            .parse()
            .context("Invalid interrupted browser process marker")?;
        ensure!(
            !live_process_group(pgid),
            "A browser process is still using the interrupted profile. Close it before recovery or reset."
        );
    }
    #[cfg(target_os = "linux")]
    if let Ok(target) = fs::read_link(work.join("SingletonLock")) {
        let pid = target
            .to_string_lossy()
            .rsplit('-')
            .next()
            .and_then(|p| p.parse::<u32>().ok());
        if let Some(pid) = pid {
            ensure!(
                !Path::new("/proc").join(pid.to_string()).exists(),
                "A browser is still using the interrupted profile. Close it before recovering or resetting storage."
            );
        } else {
            anyhow::bail!("Cannot identify the browser holding the interrupted profile lock");
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = work;
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn live_process_group(pgid: u32) -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return true;
    };
    entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().parse::<u32>().is_ok())
        .any(|entry| {
            let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
                return false;
            };
            let Some((_, rest)) = stat.rsplit_once(')') else {
                return false;
            };
            let parts: Vec<_> = rest.split_whitespace().take(3).collect();
            parts.len() == 3 && parts[0] != "Z" && parts[2].parse::<u32>() == Ok(pgid)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    const PASSWORD: &str = "a correct horse battery password";

    fn open(root: &Path) -> Store {
        match Store::open(root).unwrap() {
            OpenStore::Acquired(store) => store,
            OpenStore::Busy => panic!("Store unexpectedly busy"),
        }
    }

    #[test]
    fn lock_is_os_held_and_survives_reset() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        assert!(matches!(Store::open(dir.path()).unwrap(), OpenStore::Busy));
        store.reset().unwrap();
        assert!(matches!(Store::open(dir.path()).unwrap(), OpenStore::Busy));
        drop(store);
        assert!(matches!(
            Store::open(dir.path()).unwrap(),
            OpenStore::Acquired(_)
        ));
    }

    #[test]
    fn roundtrip_profile_and_validated_backup_with_no_plaintext_at_rest() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        let unlocked = store.create(PASSWORD).unwrap();
        let work = store.prepare(&unlocked, Recovery::Saved).unwrap();
        private_directory(&work.join("Default")).unwrap();
        let secret = "private history, passwords and cookies";
        fs::write(work.join("Default/History"), secret).unwrap();
        fs::write(work.join("Local State"), "encryption metadata").unwrap();
        store.save(&unlocked).unwrap();
        assert!(!work.exists());
        let sealed = fs::read(dir.path().join("profile.vault")).unwrap();
        assert!(
            !sealed
                .windows(secret.len())
                .any(|part| part == secret.as_bytes())
        );
        assert!(store.unlock("incorrect password", false).is_err());
        let unlocked = store.unlock(PASSWORD, false).unwrap();
        let work = store.prepare(&unlocked, Recovery::Saved).unwrap();
        assert_eq!(
            fs::read_to_string(work.join("Default/History")).unwrap(),
            secret
        );
        assert_eq!(
            fs::read_to_string(work.join("Local State")).unwrap(),
            "encryption metadata"
        );
        fs::write(work.join("Default/History"), "new history").unwrap();
        store.save(&unlocked).unwrap();
        let backup = store.unlock(PASSWORD, true).unwrap();
        let work = store.prepare(&backup, Recovery::Saved).unwrap();
        assert_eq!(
            fs::read_to_string(work.join("Default/History")).unwrap(),
            secret
        );
        store.reset().unwrap();
        assert!(!store.exists() && !store.has_backup() && !store.needs_recovery());
    }

    #[test]
    fn tampering_never_unlocks_or_modifies_interrupted_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        let unlocked = store.create(PASSWORD).unwrap();
        let work = store.prepare(&unlocked, Recovery::Saved).unwrap();
        fs::write(work.join("unsaved"), "keep this session").unwrap();
        let original = fs::read(dir.path().join("profile.vault")).unwrap();
        for offset in [8, 24, 50, HEADER, original.len() - 1] {
            let mut broken = original.clone();
            broken[offset] ^= 1;
            fs::write(dir.path().join("profile.vault"), broken).unwrap();
            assert!(store.unlock(PASSWORD, false).is_err());
            assert_eq!(
                fs::read_to_string(work.join("unsaved")).unwrap(),
                "keep this session"
            );
        }
    }

    #[test]
    fn interrupted_profile_can_be_recovered_or_explicitly_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        let unlocked = store.create(PASSWORD).unwrap();
        let work = store.prepare(&unlocked, Recovery::Saved).unwrap();
        fs::write(work.join("unsaved"), "new session").unwrap();
        drop(unlocked);
        drop(store); // Simulate controller interruption, leaving working/ behind.
        let store = open(dir.path());
        assert!(store.needs_recovery());
        let unlocked = store.unlock(PASSWORD, false).unwrap();
        store.prepare(&unlocked, Recovery::Unsaved).unwrap();
        store.save(&unlocked).unwrap();
        let unlocked = store.unlock(PASSWORD, false).unwrap();
        let work = store.prepare(&unlocked, Recovery::Saved).unwrap();
        assert_eq!(
            fs::read_to_string(work.join("unsaved")).unwrap(),
            "new session"
        );
        fs::write(work.join("discard"), "discard me").unwrap();
        store.prepare(&unlocked, Recovery::Saved).unwrap();
        assert!(!work.join("discard").exists());
        assert!(work.join("unsaved").exists());
    }

    #[test]
    fn profile_archive_rejects_traversal_and_skips_cache_and_links() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("../escaped", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"escape").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let output = dir.path().join("out");
        private_directory(&output).unwrap();
        assert!(extract(&bytes, &output).is_err());
        assert!(!dir.path().join("escaped").exists());
        fs::create_dir(output.join("Cache")).unwrap();
        fs::write(output.join("Cache/cached"), "cache").unwrap();
        fs::write(output.join("History"), "history").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("outside"), output.join("link")).unwrap();
        let bytes = archive(&output).unwrap();
        let zip = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        assert_eq!(
            zip.file_names()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap(),
            vec!["History"]
        );
    }

    #[test]
    fn interrupted_cleanup_never_recovers_a_partially_deleted_profile() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        let unlocked = store.create(PASSWORD).unwrap();
        let work = store.prepare(&unlocked, Recovery::Saved).unwrap();
        fs::write(work.join("History"), "saved history").unwrap();
        store.save(&unlocked).unwrap();
        // Simulate a crash partway through cleanup after a committed save.
        private_directory(&work).unwrap();
        fs::write(work.join("leftover"), "partial working copy").unwrap();
        fs::write(dir.path().join("cleanup.pending"), [0u8; 24]).unwrap();
        assert!(!store.needs_recovery());
        let unlocked = store.unlock(PASSWORD, false).unwrap();
        assert!(store.prepare(&unlocked, Recovery::Unsaved).is_err());
        store.prepare(&unlocked, Recovery::Saved).unwrap();
        assert!(!work.join("leftover").exists());
        assert_eq!(
            fs::read_to_string(work.join("History")).unwrap(),
            "saved history"
        );
        assert!(!store.pending_cleanup());
    }
}
