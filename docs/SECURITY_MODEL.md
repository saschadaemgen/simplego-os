# Security Model

## What SimpleGo OS protects

| Asset | Protection |
|:------|:-----------|
| Data on the disk while the laptop is off | LUKS2 full disk encryption, AES-XTS with a 512-bit key, Argon2id key derivation |
| The disk key | Lives in RAM only while the laptop runs; wiped with `cryptsetup luksSuspend` before an instant power off |
| The user's location and network identity | Every connection through Tor; kill switch blocks the rest |
| What the laptop hears, sees and radiates | Camera, microphone and Bluetooth off by default and at every start |

## Threat model

- **Lost, stolen or seized laptop that is off:** the disk cannot be read without the passphrase.
- **Laptop taken while running:** the attacker holds a running system. Mitigations: idle power off (5 minutes, 15 minutes by default, 1 hour, or never), lid and power key power off, no standby. SimpleGo OS does not claim protection for a laptop that is taken while unlocked and in use.
- **Cold boot and memory attacks:** no standby or hibernate state exists; instant off wipes the disk key first; a power off that hangs ends in an immediate restart.
- **Network observers:** see only Tor traffic.
- **Malicious or compromised applications:** cannot reach the network outside Tor (kill switch). Stronger isolation (Whonix Gateway and Workstation) is part of the image line in `simplego-distribution`.

## Deliberate decisions

- **No screen saver, no screen off:** a dark screen on a running laptop looks off while the disk is unlocked.
- **Lock screen optional and off by default:** a locked laptop keeps the disk unlocked. Users who switch it on are warned.
- **"Never" for idle power off stays available:** some uses need a laptop that does not switch itself off; the user decides.

## Root helpers

`sentinel-poweroff` and `simplego-settings-helper` are the only programs the working user may run as root, without a password, through sudo. Both accept fixed verbs and check every argument (numbers in range, values from fixed lists, time zones only from `timedatectl list-timezones`).
