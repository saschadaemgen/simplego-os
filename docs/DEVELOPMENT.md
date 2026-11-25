# Development

## Build the shell and apps

```bash
sudo apt install rustc cargo pkg-config libgtk-4-dev libgtk4-layer-shell-dev
cd shell
cargo build --release
```

| Binary | Installed as |
|:-------|:-------------|
| `sentinel-shell` | `/usr/local/bin/sentinel-shell` |
| `settings` | `/usr/local/bin/sentinel-settings` |
| `cleandrop` | `/usr/local/bin/sentinel-cleandrop` |
| `serverapp` | `/usr/local/bin/sentinel-server` |

On the laptop the build runs offline against Debian's Rust crates (`/usr/share/cargo/registry`, `cargo build --release --offline`).

Previews on another desktop without gtk4-layer-shell: `cargo build --release --no-default-features`.

## Try the settings app on another desktop

```bash
SG_SETTINGS_PAGE=power ./target/release/settings
SG_SETTINGS_PAGE=system ./target/release/settings
./target/release/settings --power-menu
```

Settings that need the laptop's tools (brightness, camera, radios, disk status) show their real state or a plain "not found" where the tool or hardware is missing; they never show a control that does nothing.

## Conventions

- Rust edition 2021, `cargo fmt`, no warnings.
- User interface texts English first, German second (`t(lang, en, de)`).
- Colours, radii and type from the SimpleGo design system (background #0A0A0B, gold #D4AF37, Space Grotesk and Inter).
- No em dashes in texts or docs.
