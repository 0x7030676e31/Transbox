# Troubleshooting

## Phone not found or unauthorized

Use `--list-devices`. Check the data cable, unlock the phone, enable USB debugging,
and accept the computer's authorization prompt. Windows may require an OEM ADB
USB driver. If scrcpy works with the same phone and PC, the basic USB setup is
likely ready. Select `--device SERIAL` if multiple devices are present.

Wireless ADB and emulators are deliberately rejected. Transbox uses the local
ADB server on port 5037; remote/custom-port ADB servers are not supported.

## Unsupported `--host-resolver-rules` warning

The alpha deliberately uses this Chromium flag to suppress desktop DNS lookups
while SOCKS destination names are resolved on the phone. Chromium displays an
unsupported-flag warning. This does not indicate that its sandbox is disabled,
and it does not prove that all possible traffic is isolated.

## TLS handshake failures / `net_error -107`

This indicates an SSL protocol failure for that connection. Repeated errors are
not a healthy result. Some pages may still load while individual connections
fail. The cause in the reported alpha logs has not been established.

Record a public affected URL, whether the main page loads, the Transbox version,
desktop OS, and whether the page works directly on the phone. Include relevant
logs, removing private URLs, credentials, tokens, and device serials. Do not bypass
certificate checks or install a custom CA as a workaround.

## Linux Wayland/Vulkan, GLib, or Fontconfig messages

These concern the host browser environment and may affect rendering or stability.
They do not establish that the phone tunnel works or that traffic has leaked.
Linux development uses your installed Chromium and its local configuration.
Check its graphics backend and font configuration independently, and report the
desktop session, browser version, and whether the page is blank or crashes.

No graphics workaround is currently applied by Transbox. Do not disable the
Chromium sandbox to work around a graphics error.

## Phone is using Wi-Fi instead of cellular

Expected: this version follows the phone's default selected network. Turn off
phone Wi-Fi to test cellular. There is no cellular-only or SIM-selection option.

## Tunnel lost

The launcher stops the browser when its relay check fails. Reconnect USB and
restart Transbox. The relay expires without heartbeats; a phone file can remain
if USB disconnected before cleanup. Automatic reconnect is not implemented.

## Download or runtime cache

Unsigned alpha executables may trigger Windows reputation prompts. Verify the
download against its published checksum and source. First launch extracts its
runtime; ensure the cache location has enough free space and is writable.
`TRANSBOX_CACHE_DIR` overrides the default location.
