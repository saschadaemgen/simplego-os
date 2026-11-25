# Architecture

SimpleGo OS 0.1.0-alpha. Part of the SimpleGo system: laptop (this repository), SimpleGo Messenger for Android and the desktop, our onion-only SimpleX relays, and the provisioning tools.

## Layers

| Layer | Parts |
|:------|:------|
| Base | Debian 13, full disk encryption (LUKS2 with LVM inside), systemd, labwc (Wayland) |
| Network | Tor as the only way out; nftables kill switch; clock set over Tor |
| Power policy | logind and sleep configuration, `sentinel-poweroff`, `sentinel-lock`, `simplego-idle` |
| Shell | `sentinel-shell`: top bar, sidebar, Home menu (GTK4, gtk4-layer-shell) |
| Apps | Settings, Clean Drop, Server, the SimpleGo Messenger (desktop, separate repository) |

## Power and lock flow

```
lid / power key / suspend key ──> logind ──> poweroff
Super + L / power menu "Lock" ──> sentinel-lock ──┬─ lock screen allowed ──> sentinel-lock-screen
idle (swayidle, simplego-idle) ───────────────────┘
                                                  └─ otherwise ──> sudo sentinel-poweroff instant
power buttons in the shell ──> mode from settings.conf: instant | fast | normal | ask (power menu)
```

`sentinel-poweroff instant`:

1. `sync`, kernel messages to a free text console
2. sysrq `s` (sync) and `u` (remount read only)
3. `cryptsetup luksSuspend` for every dm-crypt mapping: the volume key is wiped from kernel memory
4. sysrq `o` (power off)
5. if the power is still on 15 seconds later: sysrq `b` (immediate restart, stops at the passphrase prompt)

## Settings app

Seven categories: Power and lock, Display and sound, Network and Tor, Devices and privacy, Keyboard and touchpad, Security, System. Layout rules: no titles above the content (the page name is the window title, shown in the top bar), no scroll bars, no close button inside the app (windows close from the top bar), content in three columns on wide screens and two on smaller ones.

| Setting | Stored in | Applied by |
|:--------|:----------|:-----------|
| Power buttons, idle power off, language, scale, night light, keyboard, touchpad | `~/.config/sentinel-os/settings.conf` | the shell, `simplego-idle`, `simplego-session-apply`, `simplego-input-apply` |
| Lock screen allowed | `~/.config/simplego/lock-screen` (`on` / `off`) | `sentinel-lock` |
| Brightness, camera, Wi-Fi and Bluetooth radios, time zone, battery action | system state | `simplego-settings-helper` (root, through sudo, fixed verbs) |
| Volume, microphone | PipeWire | `wpctl` |

## Files

See `os/README.md` for every file in `os/rootfs/` and where it goes.
