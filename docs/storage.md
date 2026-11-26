# Local storage

The default location is `%LOCALAPPDATA%\Transbox\storage` on Windows and
`$XDG_DATA_HOME/transbox` (or `~/.local/share/transbox`) on Linux.
`--data-dir PATH` selects a different store. The browser runtime cache is separate.

First use creates a password of at least 12 characters. Subsequent launches unlock
that store. Chromium history, cookies, saved logins, bookmarks, settings and site
storage persist between sessions. Browser caches and process lock files are not
saved. The profile limit is 512 MiB excluding caches, with at most 100,000 files.

The whole profile is encrypted, rather than just one Chromium database:

- Argon2id derives a wrapping key with a random salt, 64 MiB memory, three passes,
  and one lane. It protects a random 256-bit data key.
- XChaCha20-Poly1305 authenticates the versioned header and encrypted snapshots.
  Every snapshot uses a fresh random nonce. Wrong passwords and tampering fail
  before profile extraction.
- Saves use atomic file replacement and retain one authenticated encrypted backup.
  Keys and password buffers are cleared when no longer needed where practical.

`profile.vault` is the saved copy; `profile.backup` is the previous saved copy.
`working/` is decrypted while Chromium is running. An OS-held lock prevents
another launcher from opening or resetting the same store during that session.
The second launch asks the existing controller to focus its window. Windows
activation is best effort; Linux compositor focus restrictions may prevent it.

## Closing and recovery

Transbox keeps the profile and tunnel available until all tracked browser
processes finish. It then saves the encrypted snapshot and removes working files.
A session that cannot be saved is retained for recovery, not silently discarded.

After interruption, unlock and choose **Recover unsaved session** or explicitly
confirm **Use last saved copy**. A browser still holding that profile must be
closed first. An interrupted cleanup after a completed save restores from the
saved vault rather than recovering a partially deleted working directory.

**Use previous saved copy** opens the encrypted backup. It requires the password
and deliberately rolls back to an older session. There is no password recovery.
**Reset local storage** requires typing `RESET` and deletes the vault, backup,
and interrupted working files; it does not delete downloads or the runtime cache.

Saved logins also retain Chromium's Windows-account protection. This feature is
intended for one machine/account, not credential migration between computers.
Anyone able to access the filesystem can delete or roll back files. While open,
the working profile is readable by the logged-in OS account. Crashes can leave
plaintext files; deletion does not promise forensic erasure, including on SSDs.
