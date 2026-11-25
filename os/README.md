# Laptop configuration

Every file in `rootfs/` goes to the same path on the laptop (Debian 13 with labwc). Packaging as Debian packages follows in `simplego-distribution`; until then the files are installed as below.

## Files

| File | Purpose |
|:-----|:--------|
| `etc/systemd/logind.conf.d/90-simplego-power.conf` | Lid, power key, suspend and hibernate keys power off; no idle action in logind |
| `etc/systemd/sleep.conf.d/90-simplego-nosleep.conf` | Suspend, hibernate, hybrid sleep and suspend-then-hibernate are not allowed |
| `usr/local/sbin/sentinel-poweroff` | Power off helper: `instant` (disk key wipe, power off, restart fallback), `fast`, `normal`, `reboot` |
| `usr/local/bin/sentinel-lock` | Every lock request: lock screen only if allowed, otherwise instant off |
| `usr/local/bin/simplego-idle` | swayidle with the idle power off time from the settings (5, 15, 60 minutes or never) |
| `usr/local/bin/simplego-session-apply` | At login: microphone muted, display scale, night light, keyboard and touchpad |
| `usr/local/bin/simplego-input-apply` | Writes keyboard repeat, touchpad options and layout into labwc and reloads it |
| `usr/local/sbin/simplego-settings-helper` | Root actions of the settings app (brightness, camera, radios, time zone, battery action, disk and kill switch status, update check) |
| `etc/systemd/system/simplego-radios-off.service` | Bluetooth blocked at every start |
| `etc/modprobe.d/simplego-camera.conf` | Camera driver not loaded at boot |
| `etc/sudoers.d/91-simplego.example` | The two sudo rules for the working user |

## Install (as root)

```bash
cp -r rootfs/etc/. /etc/
cp -r rootfs/usr/. /usr/
chmod 755 /usr/local/sbin/sentinel-poweroff /usr/local/sbin/simplego-settings-helper /usr/local/bin/sentinel-lock /usr/local/bin/simplego-*
sed "s/^USER /$(logname) /" rootfs/etc/sudoers.d/91-simplego.example > /tmp/91-simplego
visudo -cf /tmp/91-simplego && install -m 440 /tmp/91-simplego /etc/sudoers.d/91-simplego
rm /etc/sudoers.d/91-simplego.example
systemctl mask sleep.target suspend.target hibernate.target hybrid-sleep.target suspend-then-hibernate.target
systemctl enable simplego-radios-off.service
systemctl daemon-reload
```

In `~/.config/labwc/autostart` of the working user, replace any `swayidle` line with:

```
sh -c 'while :; do /usr/local/bin/simplego-idle; sleep 1; done' &
/usr/local/bin/simplego-session-apply &
```

Packages the backends need: `swayidle brightnessctl wlsunset wlr-randr upower rfkill iw wireless-tools wireplumber cryptsetup`.

Changes to the power path need a cold start test: lid closed means off, the next start asks for the disk passphrase, Super + L means off while the lock screen is not allowed.
