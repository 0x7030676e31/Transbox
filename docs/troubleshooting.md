# Troubleshooting

## Phone connection

Use `--list-devices`. Check the USB data cable, enable USB debugging, unlock the
phone, and accept its authorization prompt. Windows may need an OEM ADB driver.
Use `--device SERIAL` when more than one device is connected.

Only USB devices and the local ADB server on port 5037 are supported. If USB or
phone networking disappears, Transbox closes the browser and saves the profile.
Reconnect and launch again; automatic reconnect is not implemented.

## Unlock or recovery

A wrong password does not reset storage. If the main file is damaged, try
**Use previous saved copy** with the password. After an interrupted shutdown,
follow the recovery choices; do not delete working files if you need unsaved data.
See [storage](storage.md). Reset is destructive and cannot recover a forgotten password.

## Browser proxy warning and protocol limits

The `--host-resolver-rules` warning is expected: Transbox uses that flag to avoid
desktop destination DNS lookups. Chromium's sandbox remains enabled.

The browser's SOCKS path carries TCP, not UDP. HTTP/3 is disabled and direct
WebRTC UDP is blocked. Sites requiring those transports may fail; they are not
allowed to fall back to the computer's connection. This is browser configuration,
not OS-enforced isolation. Phone Wi-Fi may be selected instead of cellular.

## Disk space

The executable extracts its runtime on first use. The cache location must be
writable and have enough space; set `TRANSBOX_CACHE_DIR` to override it. The
profile also needs working space during unlock and save. Do not remove storage
files while Transbox is running.
