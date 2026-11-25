# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| latest  | :white_check_mark: |

SimpleGo OS is in active pre-release development (alpha). Security updates are applied
to the latest version on the main branch only.

## Reporting a Vulnerability

If you discover a security vulnerability in SimpleGo OS, please report it
privately using GitHub's **Private Vulnerability Reporting** feature on
this repository. Do NOT open a public issue for security vulnerabilities.

**What to expect:**
- Acknowledgment within 72 hours
- Status update within 7 days
- Coordinated disclosure after a fix is available

**What to include:**
- Description of the vulnerability
- Steps to reproduce (if applicable)
- Affected component (shell, settings app, power and lock handling, laptop configuration, server tooling)
- Impact assessment

## Scope

SimpleGo OS is the hardened laptop system of SimpleGo. Security-relevant areas include:
- Power off and disk encryption (instant off, disk key wipe, no standby, no hibernate)
- The optional lock screen and every path that must power off instead of locking
- Root helpers called through sudo (`sentinel-poweroff`, `simplego-settings-helper`) and their argument checks
- Camera, microphone and Bluetooth defaults
- Tor-only networking and the kill switch on the laptop
- The tooling for onion-only SimpleX relays

## Contact

For non-vulnerability security questions, open a regular GitHub issue.
