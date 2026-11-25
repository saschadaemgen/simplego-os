# Contributing to SimpleGo OS

Thank you for considering contributing to SimpleGo OS!

---

## Code of Conduct

This project is governed by our Code of Conduct (CODE_OF_CONDUCT.md). By participating, you are expected to uphold this code.

---

## Getting Started

### Before You Start

1. Check existing issues - Your idea might already be discussed
2. Read the documentation - Familiarize yourself with the README and docs/ARCHITECTURE.md
3. Discuss first - For significant changes, open an issue before coding

### Types of Contributions

| Contribution | Description |
|--------------|-------------|
| Bug Reports | Found a bug? Let us know! |
| Feature Requests | Have an idea? We would love to hear it! |
| Documentation | Typos, clarifications, examples |
| Code | Bug fixes, features, refactoring |
| Hardware Testing | Test on other laptops (power off, lid, suspend keys, sensors) |

---

## Development Setup

### Prerequisites

| Component | Version |
|-----------|---------|
| Debian | 13 (trixie), x86_64 |
| Rust | 1.80 or later |
| GTK | 4.12 or later, with gtk4-layer-shell |
| labwc | as shipped by Debian 13 |

Build steps are in docs/DEVELOPMENT.md.

---

## Rules for Changes

1. **Off means encrypted.** No change may add standby, hibernate, a screen saver or any other state where the laptop looks off while the disk is unlocked.
2. **No control without a backend.** Every switch, slider or choice in the settings app must change the system for real.
3. **English by default.** Every text in the user interface is English first; German is the second language.
4. **No secrets.** Never commit server or onion addresses of real servers, keys, passwords or personal data.
5. **Root helpers stay small.** Anything that runs through sudo takes fixed verbs and checks every argument.

### Commit Messages

Conventional Commits, one module per commit:

```
feat(settings): add idle power off choices
fix(power): wipe the disk key before the power off
docs(readme): describe the protection layers
```

### Pull Requests

- One topic per pull request
- Describe what you tested and on which hardware
- Screenshots for user interface changes
