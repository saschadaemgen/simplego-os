//! SimpleGo OS Settings (canvas "SimpleGo OS Settings", seven categories).
//!
//! Full screen between the shell bars. Left: seven categories. Right: the open category in two
//! columns over the full width. English by default, Deutsch as second language. Closing goes
//! through the top bar, so the app has no close button of its own.
//!
//! `sentinel-settings`              opens the settings window.
//! `sentinel-settings --power-menu` opens the power menu (mode "Ask").
//!
//! Backends:
//! - ~/.config/sentinel-os/settings.conf   power.*, idle.*, battery.poweroff, night.*, display.scale,
//!                                          kb.*, tp.*, language
//! - ~/.config/simplego/lock-screen        "on" or "off" (read by /usr/local/bin/sentinel-lock)
//! - /usr/local/bin/simplego-idle          swayidle: power off after the idle time (restarted on change)
//! - /usr/local/bin/simplego-input-apply   writes keyboard and touchpad into labwc and reloads it
//! - /usr/local/sbin/simplego-settings-helper (sudo -n) for brightness, camera, radios, time zone,
//!                                          battery action, disk and kill switch status, updates
//! - wpctl (volume, microphone), wlr-randr (scale, mode), wlsunset (night light)

use gtk::prelude::*;
use gtk::{cairo, gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;

const APP_ID: &str = "org.sentinelx.Settings";
const VERSION: &str = "0.1.0 alpha";
const POWEROFF: &str = "/usr/local/sbin/sentinel-poweroff";
const HELPER: &str = "/usr/local/sbin/simplego-settings-helper";
const LOCK_CMD: &str = "/usr/local/bin/sentinel-lock";
const INPUT_APPLY: &str = "/usr/local/bin/simplego-input-apply";
const SESSION_APPLY: &str = "/usr/local/bin/simplego-session-apply";

// ---------------------------------------------------------------- files and commands

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
}

fn conf_path() -> PathBuf {
    config_dir().join("sentinel-os").join("settings.conf")
}

fn lock_path() -> PathBuf {
    config_dir().join("simplego").join("lock-screen")
}

fn write_atomic(p: &PathBuf, body: &str) -> std::io::Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(tmp, p)
}

fn lock_allowed() -> bool {
    std::fs::read_to_string(lock_path()).map(|s| s.trim() == "on").unwrap_or(false)
}

fn set_lock_allowed(on: bool) -> bool {
    write_atomic(&lock_path(), if on { "on\n" } else { "off\n" }).is_ok()
}

fn run(prog: &str, args: &[&str]) -> bool {
    Command::new(prog).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}

fn out(prog: &str, args: &[&str]) -> String {
    Command::new(prog).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

fn helper(args: &[&str]) -> bool {
    let mut a = vec!["-n", HELPER];
    a.extend_from_slice(args);
    run("sudo", &a)
}

fn helper_out(args: &[&str]) -> String {
    let mut a = vec!["-n", HELPER];
    a.extend_from_slice(args);
    out("sudo", &a)
}

/// Starts a program detached from this app, so it keeps running after the window closes.
fn spawn_detached(cmd: &str) {
    let _ = Command::new("setsid").args(["-f", "sh", "-c", cmd]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

fn read_file(p: &str) -> String {
    std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// key=value lines; unknown keys and comments are kept when saving.
#[derive(Clone, Default)]
struct Conf {
    lines: Vec<String>,
}

impl Conf {
    fn load() -> Self {
        let text = std::fs::read_to_string(conf_path()).unwrap_or_default();
        Conf { lines: text.lines().map(str::to_string).collect() }
    }
    fn get(&self, key: &str) -> Option<String> {
        self.lines.iter().rev().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == key).then(|| v.trim().to_string())
        })
    }
    fn get_or(&self, key: &str, def: &str) -> String {
        self.get(key).filter(|v| !v.is_empty()).unwrap_or_else(|| def.to_string())
    }
    fn num(&self, key: &str, def: i64) -> i64 {
        self.get(key).and_then(|v| v.parse().ok()).unwrap_or(def)
    }
    fn flag(&self, key: &str, def: bool) -> bool {
        match self.get(key).as_deref() {
            Some("on") => true,
            Some("off") => false,
            _ => def,
        }
    }
    fn set(&mut self, key: &str, value: &str) {
        self.lines.retain(|l| l.split_once('=').map(|(k, _)| k.trim() != key).unwrap_or(true));
        self.lines.push(format!("{key}={value}"));
    }
    fn save(&self) -> bool {
        let mut body = String::from("# SimpleGo OS settings (written by SimpleGo OS Settings)\n");
        for l in &self.lines {
            if l.starts_with("# SENTINEL OS settings") || l.starts_with("# SimpleGo OS settings") {
                continue;
            }
            body.push_str(l);
            body.push('\n');
        }
        write_atomic(&conf_path(), &body).is_ok()
    }
    fn lang(&self) -> Lang {
        if self.get("language").as_deref() == Some("de") { Lang::De } else { Lang::En }
    }
    fn mode(&self, key: &str) -> String {
        let m = self.get(key).unwrap_or_default();
        if MODES.iter().any(|x| x.0 == m) { m } else { "instant".into() }
    }
}

// ---------------------------------------------------------------- system facts

fn file_has(path: &str, needle: &str) -> bool {
    std::fs::read_to_string(path).map(|s| s.lines().any(|l| l.trim() == needle)).unwrap_or(false)
}

fn lid_powers_off() -> bool {
    file_has("/etc/systemd/logind.conf.d/90-simplego-power.conf", "HandleLidSwitch=poweroff")
}
fn key_powers_off() -> bool {
    file_has("/etc/systemd/logind.conf.d/90-simplego-power.conf", "HandlePowerKey=poweroff")
}
fn sleep_disabled() -> bool {
    file_has("/etc/systemd/sleep.conf.d/90-simplego-nosleep.conf", "AllowSuspend=no")
}

fn backlight() -> Option<(f64, f64)> {
    let dir = std::fs::read_dir("/sys/class/backlight").ok()?.flatten().next()?.path();
    let cur: f64 = read_file(&format!("{}/brightness", dir.display())).parse().ok()?;
    let max: f64 = read_file(&format!("{}/max_brightness", dir.display())).parse().ok()?;
    Some((cur, max))
}

fn battery() -> Option<(i64, String)> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?.flatten()
        .map(|e| e.path())
        .find(|p| read_file(&format!("{}/type", p.display())) == "Battery")?;
    let cap: i64 = read_file(&format!("{}/capacity", dir.display())).parse().ok()?;
    Some((cap, read_file(&format!("{}/status", dir.display()))))
}

/// (volume 0..100, muted) of the default sink or source.
fn wp_volume(target: &str) -> Option<(i64, bool)> {
    let s = out("wpctl", &["get-volume", target]);
    let v: f64 = s.split_whitespace().nth(1)?.parse().ok()?;
    Some(((v * 100.0).round() as i64, s.contains("MUTED")))
}

fn module_loaded(m: &str) -> bool {
    std::fs::read_to_string("/proc/modules").map(|s| s.lines().any(|l| l.split(' ').next() == Some(m))).unwrap_or(false)
}

/// Some(blocked) for the first rfkill device of a type, None if the laptop has none.
fn rfkill_blocked(kind: &str) -> Option<bool> {
    for e in std::fs::read_dir("/sys/class/rfkill").ok()?.flatten() {
        let p = e.path();
        if read_file(&format!("{}/type", p.display())) == kind {
            return Some(read_file(&format!("{}/soft", p.display())) == "1" || read_file(&format!("{}/hard", p.display())) == "1");
        }
    }
    None
}

fn ssid() -> String {
    let s = out("iwgetid", &["-r"]);
    if !s.is_empty() {
        return s;
    }
    out("sh", &["-c", "iw dev 2>/dev/null | awk '/ssid/{ $1=\"\"; sub(/^ /,\"\"); print; exit }'"])
}

/// Outputs of wlr-randr: (name, current mode, scale).
fn outputs() -> Vec<(String, String, String)> {
    let text = out("wlr-randr", &[]);
    let mut res = Vec::new();
    for l in text.lines() {
        if !l.starts_with(' ') && !l.is_empty() {
            res.push((l.split_whitespace().next().unwrap_or("").to_string(), String::new(), String::new()));
        } else if let Some(last) = res.last_mut() {
            let t = l.trim();
            if t.contains("(current)") {
                last.1 = t.replace(", current", "").replace("(current)", "").replace(" px", "").trim().to_string();
            } else if let Some(s) = t.strip_prefix("Scale:") {
                last.2 = s.trim().to_string();
            }
        }
    }
    res
}

fn os_name() -> String {
    let text = read_file("/etc/os-release");
    text.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|v| v.trim_matches('"').to_string())
        .unwrap_or_else(|| "SimpleGo OS".into())
}

fn device_name() -> String {
    let n = format!("{} {}", read_file("/sys/class/dmi/id/sys_vendor"), read_file("/sys/class/dmi/id/product_name")).trim().to_string();
    if n.is_empty() { "-".into() } else { n }
}

fn cpu_name() -> String {
    read_file("/proc/cpuinfo").lines().find_map(|l| l.strip_prefix("model name").map(|v| v.trim_start_matches([' ', '\t', ':']).to_string()))
        .unwrap_or_else(|| "-".into())
}

fn memory() -> String {
    read_file("/proc/meminfo").lines().find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|v| v.split_whitespace().next()?.parse::<f64>().ok())
        .map(|kb| format!("{:.1} GB", kb / 1024.0 / 1024.0)).unwrap_or_else(|| "-".into())
}

/// (used GB, total GB, percent) of the root file system.
fn disk() -> Option<(f64, f64, i64)> {
    let s = out("df", &["-B1", "--output=used,size", "/"]);
    let l = s.lines().nth(1)?;
    let mut it = l.split_whitespace();
    let used: f64 = it.next()?.parse().ok()?;
    let size: f64 = it.next()?.parse().ok()?;
    Some((used / 1e9, size / 1e9, (used / size * 100.0).round() as i64))
}

fn timezone() -> String {
    let t = out("timedatectl", &["show", "-p", "Timezone", "--value"]);
    if t.is_empty() { read_file("/etc/timezone") } else { t }
}

fn tor_running() -> bool {
    run("systemctl", &["is-active", "--quiet", "tor@default"]) || run("systemctl", &["is-active", "--quiet", "tor"])
}

// ---------------------------------------------------------------- texts

#[derive(Clone, Copy, PartialEq)]
enum Lang {
    En,
    De,
}

fn t(l: Lang, en: &'static str, de: &'static str) -> &'static str {
    if l == Lang::De { de } else { en }
}

const MODES: [(&str, &str, &str); 4] = [
    ("instant", "Instant off", "Sofort aus"),
    ("fast", "Fast", "Schnell"),
    ("normal", "Normal", "Normal"),
    ("ask", "Ask", "Fragen"),
];

const TRIGGERS: [(&str, &str, &str); 3] = [
    ("power.topbar", "Power button, top bar", "Ausschalter, obere Leiste"),
    ("power.sidebar", "Power button, sidebar", "Ausschalter, Seitenleiste"),
    ("power.hotkey", "Shortcut Super + Shift + Esc", "Tastenkürzel Super + Shift + Esc"),
];

/// Idle power off in minutes; 0 means never.
const POWER_OFF: [i64; 4] = [5, 15, 60, 0];

fn mode_name(l: Lang, m: &str) -> &'static str {
    MODES.iter().find(|x| x.0 == m).map(|x| t(l, x.1, x.2)).unwrap_or("")
}

fn run_poweroff(mode: &str) {
    if !run("sudo", &["-n", POWEROFF, mode]) {
        let verb = if mode == "reboot" { "reboot" } else { "poweroff" };
        let _ = Command::new("systemctl").arg(verb).status();
    }
}

// ---------------------------------------------------------------- look

fn setup_display() {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(true);
        settings.set_gtk_enable_animations(true);
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("../settings.css"));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_USER);
    }
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    for c in class.split_whitespace() {
        l.add_css_class(c);
    }
    l.set_xalign(0.0);
    // Explanations wrap inside their column instead of widening the page.
    if class.contains("sg-sub") || class.contains("sg-hint") {
        l.set_wrap(true);
        l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    }
    l
}

fn centered(text: &str, class: &str) -> gtk::Label {
    let l = label(text, class);
    l.set_xalign(0.5);
    l.set_justify(gtk::Justification::Center);
    l
}

fn bx(o: gtk::Orientation, spacing: i32, class: &str) -> gtk::Box {
    let b = gtk::Box::new(o, spacing);
    for c in class.split_whitespace() {
        b.add_css_class(c);
    }
    b
}
fn vbox(spacing: i32, class: &str) -> gtk::Box {
    bx(gtk::Orientation::Vertical, spacing, class)
}
fn hbox(spacing: i32, class: &str) -> gtk::Box {
    bx(gtk::Orientation::Horizontal, spacing, class)
}

fn wordmark(suffix: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_markup(&format!(
        "<span foreground=\"#ECE6D6\">SIMPLE</span><span foreground=\"#D4AF37\">GO</span><span foreground=\"#9C9CA3\">{}</span>",
        glib::markup_escape_text(suffix)
    ));
    l.add_css_class(class);
    l
}

// ---------------------------------------------------------------- icons (24 unit grid)

#[derive(Clone, Copy)]
enum Glyph {
    Power,
    Lock,
    Display,
    Network,
    Privacy,
    Input,
    Shield,
    System,
    Restart,
    Warning,
    Check,
}

fn hex(cr: &cairo::Context, rgb: u32, a: f64) {
    cr.set_source_rgba(((rgb >> 16) & 255) as f64 / 255.0, ((rgb >> 8) & 255) as f64 / 255.0, (rgb & 255) as f64 / 255.0, a);
}

fn rounded_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

fn circle(cr: &cairo::Context, x: f64, y: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x, y, r, 0.0, 2.0 * PI);
}

fn arc_open(cr: &cairo::Context, x: f64, y: f64, r: f64, a: f64, b: f64) {
    cr.new_sub_path();
    cr.arc(x, y, r, a * PI / 180.0, b * PI / 180.0);
}

fn line(cr: &cairo::Context, pts: &[(f64, f64)]) {
    cr.move_to(pts[0].0, pts[0].1);
    for p in &pts[1..] {
        cr.line_to(p.0, p.1);
    }
}

/// Adds the stroked parts of a glyph; returns dots to fill.
fn glyph_path(cr: &cairo::Context, g: Glyph) -> Vec<(f64, f64)> {
    match g {
        Glyph::Power => {
            line(cr, &[(12.0, 3.0), (12.0, 11.0)]);
            arc_open(cr, 12.0, 12.3, 7.5, -49.3, 229.3);
            vec![]
        }
        Glyph::Lock => {
            line(cr, &[(7.0, 11.0), (7.0, 8.0)]);
            cr.arc(12.0, 8.0, 5.0, PI, 2.0 * PI);
            cr.line_to(17.0, 11.0);
            rounded_rect(cr, 5.5, 11.0, 13.0, 9.5, 2.0);
            vec![]
        }
        Glyph::Display => {
            rounded_rect(cr, 3.5, 5.0, 17.0, 11.0, 1.5);
            line(cr, &[(9.0, 20.0), (15.0, 20.0)]);
            line(cr, &[(12.0, 16.0), (12.0, 20.0)]);
            vec![]
        }
        Glyph::Network => {
            arc_open(cr, 12.0, 19.5, 15.5, 215.0, 325.0);
            arc_open(cr, 12.0, 19.5, 10.5, 215.0, 325.0);
            arc_open(cr, 12.0, 19.5, 5.5, 215.0, 325.0);
            vec![(12.0, 19.0)]
        }
        Glyph::Privacy => {
            cr.move_to(2.5, 12.0);
            cr.curve_to(5.5, 7.0, 8.5, 5.5, 12.0, 5.5);
            cr.curve_to(15.5, 5.5, 18.5, 7.0, 21.5, 12.0);
            cr.curve_to(18.5, 17.0, 15.5, 18.5, 12.0, 18.5);
            cr.curve_to(8.5, 18.5, 5.5, 17.0, 2.5, 12.0);
            cr.close_path();
            circle(cr, 12.0, 12.0, 2.6);
            vec![]
        }
        Glyph::Input => {
            rounded_rect(cr, 3.0, 6.5, 18.0, 11.0, 1.5);
            line(cr, &[(8.0, 14.0), (16.0, 14.0)]);
            vec![(7.0, 10.0), (10.5, 10.0), (14.0, 10.0), (17.5, 10.0)]
        }
        Glyph::Shield => {
            cr.move_to(12.0, 3.0);
            cr.line_to(4.5, 6.0);
            cr.line_to(4.5, 11.5);
            cr.curve_to(4.5, 16.0, 7.7, 19.8, 12.0, 21.0);
            cr.curve_to(16.3, 19.8, 19.5, 16.0, 19.5, 11.5);
            cr.line_to(19.5, 6.0);
            cr.close_path();
            line(cr, &[(9.5, 12.5), (11.3, 14.3), (14.8, 10.5)]);
            vec![]
        }
        Glyph::System => {
            circle(cr, 12.0, 12.0, 3.0);
            for k in 0..8 {
                let a = k as f64 * PI / 4.0;
                line(cr, &[(12.0 + 6.0 * a.cos(), 12.0 + 6.0 * a.sin()), (12.0 + 9.0 * a.cos(), 12.0 + 9.0 * a.sin())]);
            }
            vec![]
        }
        Glyph::Restart => {
            arc_open(cr, 12.0, 12.0, 8.0, 0.0, 315.5);
            line(cr, &[(20.0, 4.0), (20.0, 9.0), (15.0, 9.0)]);
            vec![]
        }
        Glyph::Warning => {
            line(cr, &[(12.0, 3.0), (2.5, 20.0), (21.5, 20.0)]);
            cr.close_path();
            line(cr, &[(12.0, 10.0), (12.0, 14.5)]);
            vec![(12.0, 17.4)]
        }
        Glyph::Check => {
            line(cr, &[(5.0, 12.5), (10.0, 17.5), (19.0, 7.5)]);
            vec![]
        }
    }
}

fn draw_glyph(cr: &cairo::Context, g: Glyph, x: f64, y: f64, size: f64, color: u32, width: f64, glow: bool) {
    let s = size / 24.0;
    cr.save().ok();
    cr.translate(x, y);
    cr.scale(s, s);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    if glow {
        cr.new_path();
        let _ = glyph_path(cr, g);
        hex(cr, 0xD4AF37, 0.35);
        cr.set_line_width(width + 3.0);
        let _ = cr.stroke();
    }
    cr.new_path();
    let dots = glyph_path(cr, g);
    hex(cr, color, 1.0);
    cr.set_line_width(width);
    let _ = cr.stroke();
    for (dx, dy) in dots {
        circle(cr, dx, dy, width * 0.65);
        let _ = cr.fill();
    }
    cr.restore().ok();
}

/// "Leuchtring": gold ring, warm inner glow, light glyph.
fn ring_icon(g: Glyph, size: i32) -> gtk::DrawingArea {
    let da = gtk::DrawingArea::new();
    da.set_content_width(size);
    da.set_content_height(size);
    da.set_valign(gtk::Align::Center);
    da.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        let r = w.min(h) / 2.0 - 2.0;
        let (cx, cy) = (w / 2.0, h / 2.0);
        let grad = cairo::RadialGradient::new(cx, cy - r, 0.0, cx, cy - r, r * 2.6);
        grad.add_color_stop_rgb(0.0, 0x2A as f64 / 255.0, 0x24 as f64 / 255.0, 0x16 as f64 / 255.0);
        grad.add_color_stop_rgb(0.7, 0x0F as f64 / 255.0, 0x0F as f64 / 255.0, 0x11 as f64 / 255.0);
        cr.arc(cx, cy, r, 0.0, 2.0 * PI);
        let _ = cr.set_source(&grad);
        let _ = cr.fill_preserve();
        hex(cr, 0xD4AF37, 0.22);
        cr.set_line_width(4.0);
        let _ = cr.stroke_preserve();
        hex(cr, 0xD4AF37, 1.0);
        cr.set_line_width(2.0);
        let _ = cr.stroke();
        let gs = r;
        draw_glyph(cr, g, cx - gs / 2.0, cy - gs / 2.0, gs, 0xF7E7A6, 1.75, true);
    });
    da
}

fn glyph_icon(g: Glyph, size: i32, color: u32, width: f64) -> gtk::DrawingArea {
    let da = gtk::DrawingArea::new();
    da.set_content_width(size);
    da.set_content_height(size);
    da.set_valign(gtk::Align::Center);
    da.set_draw_func(move |_, cr, w, h| {
        let s = w.min(h) as f64;
        draw_glyph(cr, g, (w as f64 - s) / 2.0, (h as f64 - s) / 2.0, s, color, width, false);
    });
    da
}

fn chevron(active: Rc<Cell<bool>>) -> gtk::DrawingArea {
    let da = gtk::DrawingArea::new();
    da.set_content_width(8);
    da.set_content_height(14);
    da.set_valign(gtk::Align::Center);
    da.set_draw_func(move |_, cr, _, _| {
        cr.set_line_cap(cairo::LineCap::Round);
        cr.set_line_join(cairo::LineJoin::Round);
        line(cr, &[(1.5, 1.5), (6.5, 7.0), (1.5, 12.5)]);
        hex(cr, if active.get() { 0xD4AF37 } else { 0x9C9CA3 }, 1.0);
        cr.set_line_width(2.0);
        let _ = cr.stroke();
    });
    da
}

// ---------------------------------------------------------------- building blocks

/// A captioned card. Rows are added with `Card::row`; separators come by themselves.
struct Card {
    section: gtk::Box,
    card: gtk::Box,
}

impl Card {
    fn new(cap: &str) -> Card {
        let section = vbox(8, "");
        let c = label(cap, "sg-cap");
        c.set_margin_start(14);
        section.append(&c);
        let card = vbox(0, "sg-card");
        section.append(&card);
        Card { section, card }
    }
    fn row(&self, w: &impl IsA<gtk::Widget>) {
        if self.card.first_child().is_some() {
            let s = hbox(0, "sg-sep");
            s.set_margin_start(16);
            self.card.append(&s);
        }
        self.card.append(w);
    }
    fn note(&self, text: &str) {
        let n = label(text, "sg-hint");
        n.set_margin_start(14);
        self.section.append(&n);
    }
}

fn value_label(text: &str, warn: bool) -> gtk::Label {
    let v = label(text, if warn { "sg-value warn" } else { "sg-value" });
    v.set_xalign(1.0);
    v.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    v.set_max_width_chars(34);
    v
}

/// A row: label (and optional sub line) left, widget right.
fn kv(name: &str, sub: Option<&str>, right: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = hbox(12, "sg-row");
    let left = vbox(2, "");
    left.set_hexpand(true);
    left.set_valign(gtk::Align::Center);
    left.append(&label(name, "sg-body"));
    if let Some(s) = sub {
        left.append(&label(s, "sg-hint"));
    }
    row.append(&left);
    row.append(right);
    row
}

fn kv_text(name: &str, value: &str) -> gtk::Box {
    kv(name, None, &value_label(value, false))
}

/// A fixed value with the gold lock, or "Not secured" in orange.
fn kv_fixed(l: Lang, name: &str, value: &str, ok: bool) -> gtk::Box {
    let right = hbox(8, "");
    if ok {
        right.append(&value_label(value, false));
        right.append(&glyph_icon(Glyph::Lock, 16, 0xD4AF37, 1.9));
    } else {
        right.append(&value_label(t(l, "Not secured", "Nicht gesichert"), true));
        right.append(&glyph_icon(Glyph::Warning, 16, 0xFB923C, 1.9));
    }
    kv(name, None, &right)
}

/// Segmented control. Returns (row, buttons).
fn seg(name: &str, options: &[String], current: usize, on_pick: impl Fn(usize) + 'static) -> (gtk::Box, Rc<Vec<gtk::ToggleButton>>) {
    let row = vbox(12, "sg-trigger");
    row.append(&label(name, "sg-body"));
    let bar = hbox(4, "sg-segbar");
    bar.set_homogeneous(true);
    let mut buttons = Vec::new();
    for (i, o) in options.iter().enumerate() {
        let b = gtk::ToggleButton::with_label(o);
        b.add_css_class("sg-seg");
        if let Some(f) = buttons.first() {
            b.set_group(Some(f));
        }
        b.set_active(i == current);
        bar.append(&b);
        buttons.push(b);
    }
    let buttons = Rc::new(buttons);
    let on_pick = Rc::new(on_pick);
    for (i, b) in buttons.iter().enumerate() {
        let f = Rc::clone(&on_pick);
        b.connect_toggled(move |b| {
            if b.is_active() {
                f(i);
            }
        });
    }
    row.append(&bar);
    (row, buttons)
}

/// Slider 0..100 with value text; `apply` runs 150 ms after the last move.
fn slider(name: &str, value: f64, unit: &'static str, ends: Option<(&str, &str)>, apply: impl Fn(f64) + 'static) -> gtk::Box {
    let row = vbox(10, "sg-trigger");
    let top = hbox(12, "");
    let n = label(name, "sg-body");
    n.set_hexpand(true);
    top.append(&n);
    let v = label(&format!("{} {}", value.round(), unit), "sg-value gold");
    top.append(&v);
    row.append(&top);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.add_css_class("sg-scale");
    scale.set_value(value);
    scale.set_draw_value(false);
    row.append(&scale);
    if let Some((a, b)) = ends {
        let e = hbox(0, "");
        let la = label(a, "sg-hint");
        la.set_hexpand(true);
        e.append(&la);
        e.append(&label(b, "sg-hint"));
        row.append(&e);
    }
    let gen = Rc::new(Cell::new(0u64));
    let apply = Rc::new(apply);
    scale.connect_value_changed(move |s| {
        let val = s.value().round();
        v.set_text(&format!("{} {}", val, unit));
        let g = gen.get() + 1;
        gen.set(g);
        let gen2 = Rc::clone(&gen);
        let ap = Rc::clone(&apply);
        glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
            if gen2.get() == g {
                ap(val);
            }
        });
    });
    row
}

/// Switch row with a hint that follows the state. `set` returns false when it failed;
/// then the switch goes back. `warn` paints the row orange while on.
struct Toggle {
    row: gtk::Box,
    sw: gtk::Switch,
}

fn toggle(name: &str, hints: (&'static str, &'static str), on: bool, warn: bool, set: impl Fn(bool) -> bool + 'static) -> Toggle {
    let row = hbox(12, "sg-row tall");
    let left = vbox(2, "");
    left.set_hexpand(true);
    left.set_valign(gtk::Align::Center);
    left.append(&label(name, "sg-body"));
    let hint = label("", "sg-hint");
    left.append(&hint);
    row.append(&left);
    let sw = gtk::Switch::new();
    sw.add_css_class("sg-switch");
    if warn {
        sw.add_css_class("warn");
    }
    sw.set_valign(gtk::Align::Center);
    sw.set_active(on);
    row.append(&sw);
    let paint = {
        let row = row.clone();
        let hint = hint.clone();
        Rc::new(move |on: bool| {
            hint.set_text(if on { hints.1 } else { hints.0 });
            if warn && on {
                row.add_css_class("warnbg");
                hint.add_css_class("warn");
            } else {
                row.remove_css_class("warnbg");
                hint.remove_css_class("warn");
            }
        })
    };
    paint(on);
    let p2 = Rc::clone(&paint);
    sw.connect_state_set(move |sw, want| {
        if set(want) {
            p2(want);
            glib::Propagation::Proceed
        } else {
            let s2 = sw.clone();
            glib::idle_add_local_once(move || s2.set_active(!want));
            glib::Propagation::Stop
        }
    });
    Toggle { row, sw }
}

/// Radio list with gold check marks.
fn choice(card: &Card, options: &[(String, String)], current: usize, pick: impl Fn(usize) + 'static) {
    let pick = Rc::new(pick);
    let checks: Rc<RefCell<Vec<gtk::DrawingArea>>> = Rc::new(RefCell::new(Vec::new()));
    for (i, (name, sub)) in options.iter().enumerate() {
        let b = gtk::Button::new();
        b.add_css_class("sg-listbtn");
        let inner = hbox(12, "");
        inner.set_valign(gtk::Align::Center);
        let left = vbox(2, "");
        left.set_hexpand(true);
        left.set_valign(gtk::Align::Center);
        left.append(&label(name, "sg-body"));
        if !sub.is_empty() {
            left.append(&label(sub, "sg-hint"));
        }
        inner.append(&left);
        let check = glyph_icon(Glyph::Check, 18, 0xD4AF37, 2.25);
        check.set_opacity(if i == current { 1.0 } else { 0.0 });
        inner.append(&check);
        checks.borrow_mut().push(check);
        b.set_child(Some(&inner));
        let p = Rc::clone(&pick);
        let cs = Rc::clone(&checks);
        b.connect_clicked(move |_| {
            for (j, c) in cs.borrow().iter().enumerate() {
                c.set_opacity(if j == i { 1.0 } else { 0.0 });
            }
            p(i);
        });
        card.row(&b);
    }
}

fn action_button(text: &str, gold: bool) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.add_css_class(if gold { "sg-gold" } else { "sg-outline-gold" });
    b
}

fn bar_row(name: &str, value: &str, frac: f64, sub: &str) -> gtk::Box {
    let row = vbox(10, "sg-trigger");
    let top = hbox(12, "");
    let n = label(name, "sg-body");
    n.set_hexpand(true);
    top.append(&n);
    top.append(&value_label(value, false));
    row.append(&top);
    let lb = gtk::LevelBar::for_interval(0.0, 1.0);
    lb.add_css_class("sg-level");
    lb.set_value(frac.clamp(0.0, 1.0));
    row.append(&lb);
    if !sub.is_empty() {
        row.append(&label(sub, "sg-hint"));
    }
    row
}

// ---------------------------------------------------------------- the window

struct NavRow {
    id: &'static str,
    button: gtk::Button,
    value: gtk::Label,
    active: Rc<Cell<bool>>,
    chevron: gtk::DrawingArea,
}

struct Ui {
    app: gtk::Application,
    win: gtk::ApplicationWindow,
    conf: RefCell<Conf>,
    page: RefCell<String>,
    nav: RefCell<Vec<NavRow>>,
    status: RefCell<Option<gtk::Label>>,
}

impl Ui {
    fn lang(&self) -> Lang {
        self.conf.borrow().lang()
    }
    fn set(&self, key: &str, value: &str) -> bool {
        let ok = {
            let mut c = self.conf.borrow_mut();
            c.set(key, value);
            c.save()
        };
        self.report(ok);
        ok
    }
    fn nav_value(&self, id: &str, text: &str) {
        if let Some(r) = self.nav.borrow().iter().find(|r| r.id == id) {
            r.value.set_text(text);
        }
    }
    fn report(&self, ok: bool) {
        if let Some(s) = self.status.borrow().as_ref() {
            s.set_visible(!ok);
            s.set_text(if ok { "" } else { t(self.lang(), "Could not save. Please try again.", "Speichern fehlgeschlagen. Bitte erneut versuchen.") });
        }
    }
}

struct Dialog {
    layer: gtk::Box,
    on_allow: RefCell<Option<Box<dyn Fn()>>>,
}

impl Dialog {
    fn show(&self, f: Box<dyn Fn()>) {
        *self.on_allow.borrow_mut() = Some(f);
        self.layer.set_visible(true);
    }
    fn hide(&self) {
        self.layer.set_visible(false);
        *self.on_allow.borrow_mut() = None;
    }
}

fn build_dialog(l: Lang) -> Rc<Dialog> {
    let layer = vbox(0, "sg-scrim");
    layer.set_hexpand(true);
    layer.set_vexpand(true);
    layer.set_visible(false);
    let card = vbox(12, "sg-dialog");
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Center);
    card.set_vexpand(true);
    card.set_size_request(460, -1);
    let disc = vbox(0, "sg-warndisc");
    disc.set_halign(gtk::Align::Center);
    disc.set_size_request(48, 48);
    let tri = glyph_icon(Glyph::Warning, 24, 0xFB923C, 1.75);
    tri.set_halign(gtk::Align::Center);
    tri.set_vexpand(true);
    disc.append(&tri);
    card.append(&disc);
    card.append(&centered(t(l, "Allow the lock screen?", "Sperre erlauben?"), "sg-h2"));
    card.append(&centered(
        t(l, "A lock screen encrypts nothing.\nThe laptop keeps running and the disk stays unlocked.\nOnly a laptop that is off is encrypted.",
             "Eine Sperre verschlüsselt nichts.\nDer Laptop läuft weiter, die Festplatte bleibt entsperrt.\nNur ein ausgeschalteter Laptop ist verschlüsselt."),
        "sg-sub",
    ));
    let buttons = hbox(12, "sg-dlgbtns");
    buttons.set_homogeneous(true);
    let allow = gtk::Button::with_label(t(l, "Allow", "Erlauben"));
    allow.add_css_class("sg-outline");
    let keep = gtk::Button::with_label(t(l, "Keep powering off", "Weiter ausschalten"));
    keep.add_css_class("sg-gold");
    buttons.append(&allow);
    buttons.append(&keep);
    card.append(&buttons);
    layer.append(&card);
    let d = Rc::new(Dialog { layer, on_allow: RefCell::new(None) });
    let d2 = Rc::clone(&d);
    allow.connect_clicked(move |_| {
        let f = d2.on_allow.borrow_mut().take();
        d2.hide();
        if let Some(f) = f {
            f();
        }
    });
    let d3 = Rc::clone(&d);
    keep.connect_clicked(move |_| d3.hide());
    let k = keep.clone();
    d.layer.connect_visible_notify(move |w| {
        if w.is_visible() {
            k.grab_focus();
        }
    });
    d
}

/// A page: optional intro, then two columns over the full width.
/// True when the screen is wide enough (logical pixels) for three columns.
fn wide_screen() -> bool {
    let Some(d) = gdk::Display::default() else { return false };
    let mons = d.monitors();
    let w = (0..mons.n_items())
        .filter_map(|i| mons.item(i).and_downcast::<gdk::Monitor>())
        .map(|m| m.geometry().width())
        .max()
        .unwrap_or(0);
    w >= 1800
}

fn page(intro: Option<&str>) -> (gtk::ScrolledWindow, gtk::Box, gtk::Box, gtk::Box, gtk::Box) {
    let body = vbox(24, "sg-pagebody");
    if let Some(i) = intro {
        body.append(&centered(i, "sg-sub"));
    }
    let cols = hbox(24, "");
    cols.set_homogeneous(true);
    let mk = || {
        let c = vbox(24, "");
        c.set_valign(gtk::Align::Start);
        c
    };
    // Three columns on wide screens, two on smaller ones (then the middle column's cards go
    // to the left column), so every page fits without scrolling.
    let (left, right) = (mk(), mk());
    let mid = if wide_screen() { mk() } else { left.clone() };
    cols.append(&left);
    if mid != left {
        cols.append(&mid);
    }
    cols.append(&right);
    body.append(&cols);
    // Pages are laid out to fit the screen; no scroll bar is shown.
    let sw = gtk::ScrolledWindow::new();
    sw.set_hscrollbar_policy(gtk::PolicyType::Never);
    sw.set_vscrollbar_policy(gtk::PolicyType::External);
    sw.set_vexpand(true);
    sw.set_child(Some(&body));
    (sw, body, left, mid, right)
}

// ---------------------------------------------------------------- pages

fn power_page(ui: &Rc<Ui>, dialog: &Rc<Dialog>) -> gtk::ScrolledWindow {
    let l = ui.lang();
    let (sw, _, left, mid, right) = page(None);
    let names: Vec<String> = MODES.iter().map(|m| t(l, m.1, m.2).to_string()).collect();

    let c = Card::new(t(l, "POWER BUTTONS", "AUSSCHALTER"));
    for (key, en, de) in TRIGGERS {
        let cur = ui.conf.borrow().mode(key);
        let idx = MODES.iter().position(|m| m.0 == cur).unwrap_or(0);
        let ui2 = Rc::clone(ui);
        let (row, _) = seg(t(l, en, de), &names, idx, move |i| {
            ui2.set(key, MODES[i].0);
            if key == "power.topbar" {
                ui2.nav_value("power", mode_name(ui2.lang(), MODES[i].0));
            }
        });
        c.row(&row);
    }
    left.append(&c.section);

    let c = Card::new(t(l, "THE MODES", "DIE MODI"));
    let legend: [(&str, &str, &str); 4] = [
        ("instant", "About 1 second. Unsaved work is lost.", "Etwa 1 Sekunde. Ungespeichertes ist weg."),
        ("fast", "About 3 seconds. Programs are closed.", "Etwa 3 Sekunden. Programme werden beendet."),
        ("normal", "About 10 seconds. Clean shutdown.", "Etwa 10 Sekunden. Sauberes Herunterfahren."),
        ("ask", "Opens the power menu.", "Öffnet das Ausschaltmenü."),
    ];
    for (m, en, de) in legend {
        let r = hbox(12, "sg-row");
        let n = label(mode_name(l, m), "sg-mode");
        n.set_size_request(96, -1);
        r.append(&n);
        let d = label(t(l, en, de), "sg-sub");
        d.set_hexpand(true);
        r.append(&d);
        c.row(&r);
    }
    mid.append(&c.section);

    let c = Card::new(t(l, "ALWAYS", "IMMER"));
    c.row(&kv_fixed(l, t(l, "Closing the lid", "Deckel zuklappen"), t(l, "Power off", "Ausschalten"), lid_powers_off()));
    c.row(&kv_fixed(l, t(l, "Power button on the laptop", "Ausschaltknopf am Gerät"), t(l, "Power off", "Ausschalten"), key_powers_off()));
    c.row(&kv_fixed(l, t(l, "Standby and hibernate", "Standby und Ruhezustand"), t(l, "Never", "Nie"), sleep_disabled()));
    let always = c.section.clone();

    let superl = value_label("", false);
    let idle_v = value_label("", false);
    let paint = {
        let superl = superl.clone();
        let idle_v = idle_v.clone();
        let ui = Rc::clone(ui);
        Rc::new(move |on: bool| {
            let l = ui.lang();
            let never = ui.conf.borrow().num("idle.poweroff", 15) == 0;
            for (v, idle) in [(&superl, false), (&idle_v, true)] {
                if idle && never {
                    v.set_text(t(l, "Nothing", "Nichts"));
                    v.remove_css_class("warn");
                    continue;
                }
                v.set_text(if on { t(l, "Lock", "Sperren") } else { t(l, "Power off", "Ausschalten") });
                if on { v.add_css_class("warn") } else { v.remove_css_class("warn") }
            }
        })
    };
    // When idle: the laptop powers off. There is no screen saver and no standby: a dark
    // screen on a running laptop looks off while the disk is still unlocked.
    let c = Card::new(t(l, "WHEN IDLE", "BEI UNTÄTIGKEIT"));
    let po = ui.conf.borrow().num("idle.poweroff", 15);
    let po_i = POWER_OFF.iter().position(|v| *v == po).unwrap_or(1);
    let ui3 = Rc::clone(ui);
    let paint_i = Rc::clone(&paint);
    let opts = vec![t(l, "5 min", "5 Min.").to_string(), t(l, "15 min", "15 Min.").to_string(), t(l, "1 hour", "1 Stunde").to_string(), t(l, "Never", "Nie").to_string()];
    let (row, _) = seg(t(l, "Power off after", "Ausschalten nach"), &opts, po_i, move |i| {
        ui3.set("idle.poweroff", &POWER_OFF[i].to_string());
        let _ = run("pkill", &["-x", "swayidle"]);
        paint_i(lock_allowed());
    });
    c.row(&row);
    c.note(t(l, "No screen saver, no standby. Powered off, the disk is encrypted again.",
                 "Kein Bildschirmschoner, kein Standby. Ausgeschaltet ist die Festplatte wieder verschlüsselt."));
    right.append(&c.section);

    // Lock screen
    let c = Card::new(t(l, "LOCK SCREEN", "BILDSCHIRMSPERRE"));
    let confirmed = Rc::new(Cell::new(false));
    let tg: Rc<RefCell<Option<gtk::Switch>>> = Rc::new(RefCell::new(None));
    let (dlg, conf2, tg2, paint2) = (Rc::clone(dialog), Rc::clone(&confirmed), Rc::clone(&tg), Rc::clone(&paint));
    let lock = toggle(t(l, "Allow lock screen", "Bildschirmsperre erlauben"),
        (t(l, "Off: Super + L and idle power the laptop off.", "Aus: Super + L und Untätigkeit schalten den Laptop aus."),
         t(l, "Allowed: Super + L and idle only lock.\nThe laptop keeps running, the disk stays unlocked.",
              "Erlaubt: Super + L und Untätigkeit sperren nur.\nDer Laptop läuft weiter, die Platte bleibt entsperrt.")),
        lock_allowed(), true, move |want| {
            if want && !conf2.get() {
                let c3 = Rc::clone(&conf2);
                let t3 = Rc::clone(&tg2);
                dlg.show(Box::new(move || {
                    c3.set(true);
                    if let Some(s) = t3.borrow().as_ref() {
                        s.set_active(true);
                    }
                }));
                return false;
            }
            conf2.set(false);
            let ok = set_lock_allowed(want);
            if ok {
                paint2(want);
            }
            ok
        });
    *tg.borrow_mut() = Some(lock.sw.clone());
    paint(lock_allowed());
    let ui4 = Rc::clone(ui);
    lock.sw.connect_active_notify(move |s| {
        let l = ui4.lang();
        ui4.nav_value("power", if s.is_active() { t(l, "Lock allowed", "Sperre erlaubt") } else { mode_name(l, &ui4.conf.borrow().mode("power.topbar")) });
    });
    c.row(&lock.row);
    c.row(&kv("Super + L", None, &superl));
    c.row(&kv(t(l, "When idle", "Bei Untätigkeit"), None, &idle_v));
    right.append(&c.section);

    right.append(&always);

    // Battery
    if let Some((cap, status)) = battery() {
        let c = Card::new(t(l, "BATTERY", "AKKU"));
        let st = match status.as_str() {
            "Charging" => t(l, "Charging", "Lädt"),
            "Full" => t(l, "Full", "Voll"),
            "Discharging" => t(l, "On battery", "Im Akkubetrieb"),
            _ => "",
        };
        c.row(&bar_row(t(l, "Charge", "Ladestand"), &format!("{cap} %"), cap as f64 / 100.0, st));
        let opts = [3i64, 5, 10];
        let cur = ui.conf.borrow().num("battery.poweroff", 5);
        let ui5 = Rc::clone(ui);
        let (row, _) = seg(t(l, "Power off below", "Ausschalten unter"), &opts.iter().map(|v| format!("{v} %")).collect::<Vec<_>>(),
            opts.iter().position(|v| *v == cur).unwrap_or(1), move |i| {
                let ok = helper(&["battery", &opts[i].to_string()]);
                if ok {
                    ui5.set("battery.poweroff", &opts[i].to_string());
                } else {
                    ui5.report(false);
                }
            });
        c.row(&row);
        mid.append(&c.section);
    }
    sw
}

fn display_page(ui: &Rc<Ui>) -> gtk::ScrolledWindow {
    let l = ui.lang();
    let (sw, _, left, mid, right) = page(None);

    let c = Card::new(t(l, "SCREEN", "BILDSCHIRM"));
    if let Some((cur, max)) = backlight() {
        c.row(&slider(t(l, "Brightness", "Helligkeit"), (cur / max * 100.0).round(), "%", None, |v| {
            let v = v.max(5.0);
            helper(&["brightness", &format!("{}", v as i64)]);
        }));
    }
    let outs = outputs();
    let scales = ["1", "1.25", "1.5", "2"];
    let cur_scale = ui.conf.borrow().get_or("display.scale", "1");
    let ui2 = Rc::clone(ui);
    let (row, _) = seg(t(l, "Scale", "Skalierung"), &["100 %", "125 %", "150 %", "200 %"].map(String::from),
        scales.iter().position(|s| *s == cur_scale).unwrap_or(0), move |i| {
            ui2.set("display.scale", scales[i]);
            for (name, _, _) in outputs() {
                run("wlr-randr", &["--output", &name, "--scale", scales[i]]);
            }
        });
    c.row(&row);
    if let Some((name, mode, _)) = outs.first() {
        c.row(&kv(t(l, "Resolution", "Auflösung"), Some(name), &value_label(if mode.is_empty() { "-" } else { mode }, false)));
    }
    left.append(&c.section);

    let c = Card::new(t(l, "NIGHT LIGHT", "NACHTLICHT"));
    let apply_night = |on: bool, warm: i64| {
        let _ = run("pkill", &["-x", "wlsunset"]);
        if on {
            // Warm all day: night lasts from 00:00 to 23:59.
            let temp = 6400 - warm * 40; // 0 -> 6400 K, 100 -> 2400 K
            spawn_detached(&format!("exec wlsunset -t {temp} -T 6500 -s 00:00 -S 23:59 -d 60"));
        }
    };
    let warm0 = ui.conf.borrow().num("night.warm", 50);
    let ui3 = Rc::clone(ui);
    let night = toggle(t(l, "Night light", "Nachtlicht"),
        (t(l, "Off: colours stay neutral.", "Aus: Farben bleiben neutral."), t(l, "On: warmer colours, easier on the eyes.", "An: wärmere Farben, schonender für die Augen.")),
        ui.conf.borrow().flag("night.on", false), false, move |on| {
            let ok = ui3.set("night.on", if on { "on" } else { "off" });
            apply_night(on, ui3.conf.borrow().num("night.warm", 50));
            ok
        });
    c.row(&night.row);
    let ui4 = Rc::clone(ui);
    c.row(&slider(t(l, "Warmth", "Wärme"), warm0 as f64, "%", Some((t(l, "Neutral", "Neutral"), t(l, "Warm", "Warm"))), move |v| {
        ui4.set("night.warm", &(v as i64).to_string());
        if ui4.conf.borrow().flag("night.on", false) {
            apply_night(true, v as i64);
        }
    }));
    mid.append(&c.section);

    let c = Card::new(t(l, "SOUND", "TON"));
    if let Some((vol, _)) = wp_volume("@DEFAULT_AUDIO_SINK@") {
        c.row(&slider(t(l, "Volume", "Lautstärke"), vol.min(100) as f64, "%", None, |v| {
            run("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{}%", v as i64)]);
            run("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", if v as i64 == 0 { "1" } else { "0" }]);
        }));
    } else {
        c.row(&kv_text(t(l, "Sound output", "Tonausgabe"), t(l, "Not found", "Nicht gefunden")));
    }
    right.append(&c.section);

    let c = Card::new(t(l, "MICROPHONE", "MIKROFON"));
    let muted = wp_volume("@DEFAULT_AUDIO_SOURCE@").map(|v| v.1).unwrap_or(true);
    c.row(&kv(t(l, "Microphone", "Mikrofon"), Some(t(l, "Switched under Devices and privacy.", "Geschaltet unter Geräte und Privatsphäre.")),
        &value_label(if muted { t(l, "Off", "Aus") } else { t(l, "On", "An") }, !muted)));
    right.append(&c.section);

    let c = Card::new(t(l, "DISPLAYS", "BILDSCHIRME"));
    for (name, mode, scale) in &outs {
        c.row(&kv(name, Some(&format!("{} {}", t(l, "Scale", "Skalierung"), scale)), &value_label(mode, false)));
    }
    if outs.len() < 2 {
        c.note(t(l, "Connect a second display to use it next to this one.", "Schließe einen zweiten Bildschirm an, um ihn neben diesem zu nutzen."));
    }
    right.append(&c.section);
    sw
}

fn network_page(ui: &Rc<Ui>) -> gtk::ScrolledWindow {
    let l = ui.lang();
    let (sw, _, left, mid, right) = page(None);

    let c = Card::new("TOR");
    let tor = tor_running();
    c.row(&kv(t(l, "Status", "Status"), None, &value_label(if tor { t(l, "Running", "Läuft") } else { t(l, "Stopped", "Gestoppt") }, !tor)));
    c.row(&kv_text(t(l, "Ports", "Ports"), "9050, 9150, 9153"));
    left.append(&c.section);

    let c = Card::new("KILL SWITCH");
    let ks = helper_out(&["killswitch-status"]) == "active";
    c.row(&kv_fixed(l, t(l, "Status", "Status"), t(l, "Active", "Aktiv"), ks));
    c.note(t(l, "Only Tor, local SSH and DHCP may pass.", "Nur Tor, lokales SSH und DHCP dürfen durch."));
    left.append(&c.section);

    let c = Card::new(t(l, "WI-FI", "WLAN"));
    match rfkill_blocked("wlan") {
        Some(blocked) => {
            let ui2 = Rc::clone(ui);
            let w = toggle(t(l, "Wi-Fi", "WLAN"),
                (t(l, "Off: the radio is switched off.", "Aus: der Funk ist abgeschaltet."), t(l, "On: the radio is switched on.", "An: der Funk ist eingeschaltet.")),
                !blocked, false, move |on| {
                    let ok = helper(&["wifi", if on { "on" } else { "off" }]);
                    ui2.report(ok);
                    ok
                });
            c.row(&w.row);
            let s = ssid();
            c.row(&kv_text(t(l, "Network", "Netzwerk"), if s.is_empty() { t(l, "Not connected", "Nicht verbunden") } else { &s }));
        }
        None => c.row(&kv_text(t(l, "Wi-Fi", "WLAN"), t(l, "No Wi-Fi hardware", "Kein WLAN vorhanden"))),
    }
    mid.append(&c.section);

    let c = Card::new(t(l, "CABLE", "KABEL"));
    let eth = out("sh", &["-c", "for d in /sys/class/net/e*; do [ -e \"$d\" ] && cat \"$d/operstate\"; done | head -1"]);
    c.row(&kv_text(t(l, "Ethernet", "Ethernet"), if eth == "up" { t(l, "Connected", "Verbunden") } else { t(l, "Not connected", "Nicht verbunden") }));
    right.append(&c.section);
    sw
}

fn privacy_page(ui: &Rc<Ui>) -> gtk::ScrolledWindow {
    let l = ui.lang();
    let (sw, _, left, mid, right) = page(None);

    let c = Card::new(t(l, "SENSORS", "SENSOREN"));
    let ui2 = Rc::clone(ui);
    let cam = toggle(t(l, "Camera", "Kamera"),
        (t(l, "Off: the camera driver is not loaded.", "Aus: der Kameratreiber ist nicht geladen."),
         t(l, "On until the next start.\nPrograms can see you.", "An bis zum nächsten Start.\nProgramme können dich sehen.")),
        module_loaded("uvcvideo"), true, move |on| {
            let ok = helper(&["camera", if on { "on" } else { "off" }]);
            ui2.report(ok);
            ok
        });
    c.row(&cam.row);
    let ui3 = Rc::clone(ui);
    let mic_on = wp_volume("@DEFAULT_AUDIO_SOURCE@").map(|v| !v.1).unwrap_or(false);
    let mic = toggle(t(l, "Microphone", "Mikrofon"),
        (t(l, "Off: no program can record.", "Aus: kein Programm kann aufnehmen."),
         t(l, "On until the next start.\nPrograms can hear you.", "An bis zum nächsten Start.\nProgramme können dich hören.")),
        mic_on, true, move |on| {
            let ok = run("wpctl", &["set-mute", "@DEFAULT_AUDIO_SOURCE@", if on { "0" } else { "1" }]);
            ui3.report(ok);
            ok
        });
    c.row(&mic.row);
    left.append(&c.section);

    let c = Card::new(t(l, "RADIO", "FUNK"));
    match rfkill_blocked("bluetooth") {
        Some(blocked) => {
            let ui4 = Rc::clone(ui);
            let bt = toggle("Bluetooth",
                (t(l, "Off: the radio is blocked.", "Aus: der Funk ist gesperrt."),
                 t(l, "On until the next start.\nNearby devices can see this laptop.", "An bis zum nächsten Start.\nGeräte in der Nähe sehen diesen Laptop.")),
                !blocked, true, move |on| {
                    let ok = helper(&["bluetooth", if on { "on" } else { "off" }]);
                    ui4.report(ok);
                    ok
                });
            c.row(&bt.row);
        }
        None => c.row(&kv_text("Bluetooth", t(l, "No Bluetooth hardware", "Kein Bluetooth vorhanden"))),
    }
    mid.append(&c.section);

    let c = Card::new(t(l, "PHONES", "TELEFONE"));
    let adb = run("sh", &["-c", "command -v adb"]);
    c.row(&kv("adb, fastboot", Some(t(l, "For setting up SimpleGo phones over USB.", "Zum Einrichten von SimpleGo-Telefonen über USB.")),
        &value_label(if adb { t(l, "Installed", "Installiert") } else { t(l, "Missing", "Fehlt") }, !adb)));
    right.append(&c.section);
    sw
}

fn input_page(ui: &Rc<Ui>) -> gtk::ScrolledWindow {
    let l = ui.lang();
    let (sw, _, left, mid, right) = page(None);
    let apply = || {
        spawn_detached(&format!("exec {INPUT_APPLY}"));
    };

    let c = Card::new(t(l, "KEYBOARD LAYOUT", "TASTATURLAYOUT"));
    let layouts = ["us", "de"];
    let cur = ui.conf.borrow().get_or("kb.layout", "us");
    let ui2 = Rc::clone(ui);
    choice(&c, &[("English (US)".into(), t(l, "Default", "Standard").into()), (t(l, "German", "Deutsch").into(), String::new())],
        layouts.iter().position(|x| *x == cur).unwrap_or(0), move |i| {
            ui2.set("kb.layout", layouts[i]);
            apply();
        });
    left.append(&c.section);

    let c = Card::new(t(l, "KEYBOARD", "TASTATUR"));
    let ui3 = Rc::clone(ui);
    c.row(&slider(t(l, "Repeat delay", "Verzögerung"), ui.conf.borrow().num("kb.delay", 40) as f64, "", Some((t(l, "Short", "Kurz"), t(l, "Long", "Lang"))), move |v| {
        ui3.set("kb.delay", &(v as i64).to_string());
        apply();
    }));
    let ui4 = Rc::clone(ui);
    c.row(&slider(t(l, "Repeat rate", "Wiederholrate"), ui.conf.borrow().num("kb.rate", 60) as f64, "", Some((t(l, "Slow", "Langsam"), t(l, "Fast", "Schnell"))), move |v| {
        ui4.set("kb.rate", &(v as i64).to_string());
        apply();
    }));
    mid.append(&c.section);

    let c = Card::new(t(l, "SHORTCUTS", "TASTENKÜRZEL"));
    c.row(&kv_text(t(l, "Emergency off", "Not-Aus"), "Super + Shift + Esc"));
    c.row(&kv_text(t(l, "Lock or power off", "Sperren oder ausschalten"), "Super + L"));
    left.append(&c.section);

    let c = Card::new("TOUCHPAD");
    let flag_toggle = |key: &'static str, name: &str, hints: (&'static str, &'static str), def: bool| {
        let ui = Rc::clone(ui);
        let on = ui.conf.borrow().flag(key, def);
        toggle(name, hints, on, false, move |on| {
            let ok = ui.set(key, if on { "on" } else { "off" });
            apply();
            ok
        })
    };
    c.row(&flag_toggle("tp.tap", t(l, "Tap to click", "Tippen zum Klicken"),
        (t(l, "Off: press the touchpad to click.", "Aus: zum Klicken das Touchpad drücken."), t(l, "On: a light tap is a click.", "An: leichtes Tippen ist ein Klick.")), true).row);
    c.row(&flag_toggle("tp.natural", t(l, "Natural scrolling", "Natürliches Scrollen"),
        (t(l, "Off: content moves against the fingers.", "Aus: Inhalt bewegt sich gegen die Finger."), t(l, "On: content follows the fingers.", "An: Inhalt folgt den Fingern.")), false).row);
    c.row(&flag_toggle("tp.dwt", t(l, "Off while typing", "Aus beim Tippen"),
        (t(l, "Off: the touchpad always reacts.", "Aus: das Touchpad reagiert immer."), t(l, "On: no stray clicks while you type.", "An: keine Fehlklicks beim Tippen.")), true).row);
    let ui5 = Rc::clone(ui);
    c.row(&slider(t(l, "Pointer speed", "Zeigertempo"), ui.conf.borrow().num("tp.speed", 50) as f64, "", Some((t(l, "Slow", "Langsam"), t(l, "Fast", "Schnell"))), move |v| {
        ui5.set("tp.speed", &(v as i64).to_string());
        apply();
    }));
    right.append(&c.section);
    sw
}

fn security_page(ui: &Rc<Ui>) -> gtk::ScrolledWindow {
    let l = ui.lang();
    let (sw, _, left, mid, right) = page(None);
    let hero = vbox(12, "sg-hero");
    let icon = ring_icon(Glyph::Shield, 56);
    icon.set_halign(gtk::Align::Center);
    hero.append(&icon);
    let luks = helper_out(&["luks-status"]);
    let get = |k: &str| luks.lines().find_map(|x| x.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
    let encrypted = !get("device").is_empty();
    hero.append(&centered(if encrypted { t(l, "The disk is encrypted.", "Die Festplatte ist verschlüsselt.") } else { t(l, "Disk status unknown.", "Status der Festplatte unbekannt.") }, "sg-body"));
    hero.append(&centered(t(l, "Every time the laptop is off, nothing on the disk\ncan be read without the passphrase.",
        "Sobald der Laptop aus ist, kann niemand etwas\nauf der Platte lesen ohne die Passphrase."), "sg-sub"));
    right.append(&hero);

    let c = Card::new(t(l, "DISK ENCRYPTION", "FESTPLATTENVERSCHLÜSSELUNG"));
    c.row(&kv_text(t(l, "Method", "Verfahren"), &format!("LUKS{}, {}", get("version"), get("cipher"))));
    c.row(&kv_text(t(l, "Key size", "Schlüssellänge"), &format!("{} bit", get("bits"))));
    c.row(&kv_text(t(l, "Key derivation", "Schlüsselableitung"), &format!("{} {}", get("pbkdf"), get("memory"))));
    c.row(&kv_text(t(l, "Device", "Gerät"), &get("device")));
    left.append(&c.section);

    let c = Card::new(t(l, "OVERVIEW", "ÜBERSICHT"));
    c.row(&kv_fixed(l, "Standby", t(l, "Never", "Nie"), sleep_disabled()));
    c.row(&kv_fixed(l, t(l, "Closing the lid", "Deckel zuklappen"), t(l, "Power off", "Ausschalten"), lid_powers_off()));
    c.row(&kv(t(l, "Lock screen", "Bildschirmsperre"), None, &value_label(if lock_allowed() { t(l, "Allowed", "Erlaubt") } else { t(l, "Off", "Aus") }, lock_allowed())));
    c.row(&kv_fixed(l, "Kill switch", t(l, "Active", "Aktiv"), helper_out(&["killswitch-status"]) == "active"));
    let cam = module_loaded("uvcvideo");
    c.row(&kv(t(l, "Camera", "Kamera"), None, &value_label(if cam { t(l, "On", "An") } else { t(l, "Off", "Aus") }, cam)));
    let bt = rfkill_blocked("bluetooth").map(|b| !b).unwrap_or(false);
    c.row(&kv("Bluetooth", None, &value_label(if bt { t(l, "On", "An") } else { t(l, "Off", "Aus") }, bt)));
    mid.append(&c.section);
    sw
}

fn system_page(ui: &Rc<Ui>) -> gtk::ScrolledWindow {
    let l = ui.lang();
    let (sw, _, left, mid, right) = page(None);

    let c = Card::new(t(l, "LANGUAGE", "SPRACHE"));
    let ui2 = Rc::clone(ui);
    choice(&c, &[("English".into(), t(l, "Default", "Standard").into()), ("Deutsch".into(), String::new())],
        if l == Lang::De { 1 } else { 0 }, move |i| {
            let code = if i == 1 { "de" } else { "en" };
            if ui2.conf.borrow().get_or("language", "en") == code {
                return;
            }
            let ok = ui2.set("language", code);
            let ui3 = Rc::clone(&ui2);
            glib::idle_add_local_once(move || {
                rebuild(&ui3);
                ui3.report(ok);
            });
        });
    c.note(t(l, "Applies to SimpleGo OS Settings and the power menu.", "Gilt für die Einstellungen und das Ausschaltmenü."));
    left.append(&c.section);

    let c = Card::new(t(l, "DATE AND TIME", "DATUM UND UHRZEIT"));
    c.row(&kv_fixed(l, t(l, "Time source", "Zeitquelle"), "Tor", true));
    let zones: Vec<String> = out("timedatectl", &["list-timezones"]).lines().map(str::to_string).collect();
    let cur = timezone();
    if !zones.is_empty() {
        let refs: Vec<&str> = zones.iter().map(String::as_str).collect();
        let dd = gtk::DropDown::from_strings(&refs);
        dd.add_css_class("sg-drop");
        dd.set_enable_search(true);
        dd.set_expression(Some(gtk::PropertyExpression::new(gtk::StringObject::static_type(), None::<gtk::Expression>, "string")));
        if let Some(i) = zones.iter().position(|z| *z == cur) {
            dd.set_selected(i as u32);
        }
        let ui4 = Rc::clone(ui);
        let zones2 = zones.clone();
        dd.connect_selected_notify(move |d| {
            if let Some(z) = zones2.get(d.selected() as usize) {
                let ok = helper(&["timezone", z]);
                ui4.report(ok);
            }
        });
        c.row(&kv(t(l, "Time zone", "Zeitzone"), None, &dd));
    } else {
        c.row(&kv_text(t(l, "Time zone", "Zeitzone"), &cur));
    }
    let now = glib::DateTime::now_local().ok().and_then(|d| d.format("%Y-%m-%d %H:%M").ok()).map(|s| s.to_string()).unwrap_or_default();
    c.row(&kv_text(t(l, "Now", "Jetzt"), &now));
    left.append(&c.section);

    let c = Card::new("UPDATES");
    let status = value_label(t(l, "Not checked yet", "Noch nicht geprüft"), false);
    c.row(&kv(t(l, "Status", "Status"), Some(t(l, "Checked only over Tor.", "Geprüft nur über Tor.")), &status));
    let check = action_button(t(l, "Check now", "Jetzt prüfen"), true);
    let wrap = vbox(0, "sg-btnrow");
    wrap.append(&check);
    c.row(&wrap);
    {
        let status = status.clone();
        check.connect_clicked(move |b| {
            b.set_sensitive(false);
            status.set_text(t(l, "Checking over Tor ...", "Prüfe über Tor ..."));
            let status = status.clone();
            let b = b.clone();
            glib::spawn_future_local(async move {
                let r = gio::spawn_blocking(|| helper_out(&["updates-check"])).await.unwrap_or_default();
                b.set_sensitive(true);
                match r.parse::<i64>() {
                    Ok(0) => status.set_text(t(l, "Up to date", "Aktuell")),
                    Ok(n) => status.set_text(&format!("{n} {}", t(l, "updates available", "Updates verfügbar"))),
                    Err(_) => status.set_text(t(l, "Check failed", "Prüfung fehlgeschlagen")),
                }
            });
        });
    }
    mid.append(&c.section);

    let c = Card::new(t(l, "STORAGE", "SPEICHER"));
    if let Some((used, size, pct)) = disk() {
        c.row(&bar_row(&format!("{:.0} GB {} {:.0} GB {}", used, t(l, "of", "von"), size, t(l, "used", "belegt")), &format!("{pct} %"), pct as f64 / 100.0,
            t(l, "Encrypted with LUKS2", "Verschlüsselt mit LUKS2")));
    }
    let cd = gtk::Button::new();
    cd.add_css_class("sg-listbtn");
    let inner = hbox(12, "");
    let lft = vbox(2, "");
    lft.set_hexpand(true);
    lft.append(&label("Clean Drop", "sg-body"));
    lft.append(&label(t(l, "Removes metadata and deletes files for good.", "Entfernt Metadaten und löscht Dateien endgültig."), "sg-hint"));
    inner.append(&lft);
    inner.append(&chevron(Rc::new(Cell::new(false))));
    cd.set_child(Some(&inner));
    cd.connect_clicked(|_| spawn_detached("exec /usr/local/bin/sentinel-cleandrop"));
    c.row(&cd);
    mid.append(&c.section);

    let c = Card::new(t(l, "ABOUT SIMPLEGO OS", "ÜBER SIMPLEGO OS"));
    c.row(&kv_text(t(l, "System", "System"), &format!("SimpleGo OS {VERSION}")));
    c.row(&kv_text(t(l, "Base", "Basis"), &os_name()));
    c.row(&kv_text(t(l, "Device", "Gerät"), &device_name()));
    c.row(&kv_text(t(l, "Processor", "Prozessor"), &cpu_name()));
    c.row(&kv_text(t(l, "Memory", "Arbeitsspeicher"), &memory()));
    c.row(&kv_text(t(l, "Maker", "Hersteller"), "IT & MORE Systems"));
    right.append(&c.section);
    sw
}


// ---------------------------------------------------------------- window

const CATS: [(&str, &str, &str); 7] = [
    ("power", "Power and lock", "Ausschalten und Sperre"),
    ("display", "Display and sound", "Bildschirm und Ton"),
    ("network", "Network and Tor", "Netzwerk und Tor"),
    ("privacy", "Devices and privacy", "Geräte und Privatsphäre"),
    ("input", "Keyboard and touchpad", "Tastatur und Touchpad"),
    ("security", "Security", "Sicherheit"),
    ("system", "System", "System"),
];

fn cat_glyph(id: &str) -> Glyph {
    match id {
        "power" => Glyph::Power,
        "display" => Glyph::Display,
        "network" => Glyph::Network,
        "privacy" => Glyph::Privacy,
        "input" => Glyph::Input,
        "security" => Glyph::Shield,
        _ => Glyph::System,
    }
}

fn rebuild(ui: &Rc<Ui>) {
    let l = ui.lang();
    ui.nav.borrow_mut().clear();

    let overlay = gtk::Overlay::new();
    let root = hbox(0, "sg-root");
    overlay.set_child(Some(&root));
    let dialog = build_dialog(l);
    overlay.add_overlay(&dialog.layer);

    // Left: seven categories, never scrolls.
    let aside = vbox(0, "sg-aside");
    aside.set_size_request(360, -1);
    aside.set_hexpand(false);
    let inner = vbox(0, "sg-groups");
    inner.set_vexpand(true);
    let card = vbox(0, "sg-card");
    let lock = lock_allowed();
    for (i, (id, en, de)) in CATS.iter().enumerate() {
        if i > 0 {
            let s = hbox(0, "sg-sep nav");
            s.set_margin_start(60);
            card.append(&s);
        }
        let value = match *id {
            "power" => if lock { t(l, "Lock allowed", "Sperre erlaubt").to_string() } else { mode_name(l, &ui.conf.borrow().mode("power.topbar")).to_string() },
            "network" => "Tor".to_string(),
            "security" => t(l, "Encrypted", "Verschlüsselt").to_string(),
            _ => String::new(),
        };
        let b = gtk::Button::new();
        b.add_css_class("sg-navrow");
        let row = hbox(12, "");
        row.append(&ring_icon(cat_glyph(id), 34));
        let n = label(t(l, en, de), "sg-body");
        n.set_hexpand(true);
        row.append(&n);
        let v = label(&value, "sg-navval");
        row.append(&v);
        let active = Rc::new(Cell::new(false));
        let ch = chevron(Rc::clone(&active));
        row.append(&ch);
        b.set_child(Some(&row));
        card.append(&b);
        ui.nav.borrow_mut().push(NavRow { id, button: b, value: v, active, chevron: ch });
    }
    inner.append(&card);
    let spacer = vbox(0, "");
    spacer.set_vexpand(true);
    inner.append(&spacer);
    let status = label("", "sg-status");
    status.set_xalign(0.5);
    status.set_wrap(true);
    status.set_visible(false);
    inner.append(&status);
    *ui.status.borrow_mut() = Some(status);
    inner.append(&centered(t(l, "Settings", "Einstellungen"), "sg-h2 sg-asidefoot"));
    let mark = wordmark(" OS", "sg-mark");
    mark.set_halign(gtk::Align::Center);
    inner.append(&mark);
    let ver = centered(&format!("{} {VERSION}", t(l, "Version", "Version")), "sg-hint sg-version");
    inner.append(&ver);
    aside.append(&inner);

    // Right: header and the open category.
    let main = vbox(0, "sg-main");
    main.set_hexpand(true);
    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(220);
    stack.add_named(&power_page(ui, &dialog), Some("power"));
    stack.add_named(&display_page(ui), Some("display"));
    stack.add_named(&network_page(ui), Some("network"));
    stack.add_named(&privacy_page(ui), Some("privacy"));
    stack.add_named(&input_page(ui), Some("input"));
    stack.add_named(&security_page(ui), Some("security"));
    stack.add_named(&system_page(ui), Some("system"));
    main.append(&stack);
    root.append(&aside);
    root.append(&main);

    let show = {
        let ui = Rc::clone(ui);
        let stack = stack.clone();
        Rc::new(move |id: &str| {
            stack.set_visible_child_name(id);
            *ui.page.borrow_mut() = id.to_string();
            if let Some((_, en, de)) = CATS.iter().find(|c| c.0 == id) {
                let l = ui.lang();
                ui.win.set_title(Some(&format!("{}: {}", t(l, "Settings", "Einstellungen"), t(l, en, de))));
            }
            for r in ui.nav.borrow().iter() {
                let on = r.id == id;
                r.active.set(on);
                if on { r.button.add_css_class("active") } else { r.button.remove_css_class("active") }
                if on { r.value.add_css_class("active") } else { r.value.remove_css_class("active") }
                r.chevron.queue_draw();
            }
        })
    };
    for r in ui.nav.borrow().iter() {
        let s = Rc::clone(&show);
        let id = r.id;
        r.button.connect_clicked(move |_| s(id));
    }
    let current = ui.page.borrow().clone();
    show(&current);

    let keys = gtk::EventControllerKey::new();
    let d2 = Rc::clone(&dialog);
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape && d2.layer.is_visible() {
            d2.hide();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    overlay.add_controller(keys);
    ui.win.set_child(Some(&overlay));
    let _ = &ui.app;
}

fn build_settings(app: &gtk::Application) {
    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .default_width(1280)
        .default_height(800)
        .decorated(false)
        .build();
    win.add_css_class("sg-window");
    let page = std::env::var("SG_SETTINGS_PAGE").ok()
        .filter(|p| CATS.iter().any(|c| c.0 == p))
        .unwrap_or_else(|| "power".into());
    let ui = Rc::new(Ui {
        app: app.clone(),
        win: win.clone(),
        conf: RefCell::new(Conf::load()),
        page: RefCell::new(page),
        nav: RefCell::new(Vec::new()),
        status: RefCell::new(None),
    });
    rebuild(&ui);
    win.maximize();
    win.present();
}

// ---------------------------------------------------------------- power menu ("Ask")

fn build_power_menu(app: &gtk::Application) {
    let l = Conf::load().lang();
    let win = gtk::ApplicationWindow::builder().application(app).title(t(l, "Power", "Ausschalten")).decorated(false).build();
    win.add_css_class("sg-menuwin");
    let backdrop = vbox(0, "sg-menubackdrop");
    let sheet = vbox(20, "sg-sheet");
    sheet.set_halign(gtk::Align::Center);
    sheet.set_valign(gtk::Align::Center);
    sheet.set_vexpand(true);
    sheet.set_size_request(440, -1);
    let head = vbox(12, "");
    let icon = ring_icon(Glyph::Power, 56);
    icon.set_halign(gtk::Align::Center);
    head.append(&icon);
    head.append(&centered(t(l, "Power off?", "Ausschalten?"), "sg-h1"));
    head.append(&centered(t(l, "Once off, the disk is encrypted.", "Ausgeschaltet ist die Festplatte verschlüsselt."), "sg-sub"));
    sheet.append(&head);
    let off = gtk::Button::with_label(t(l, "Power off now", "Jetzt ausschalten"));
    off.add_css_class("sg-gold");
    off.add_css_class("big");
    sheet.append(&off);
    let card = vbox(0, "sg-card");
    let restart = gtk::Button::new();
    restart.add_css_class("sg-listbtn");
    let ri = hbox(12, "");
    ri.append(&glyph_icon(Glyph::Restart, 18, 0xD4AF37, 1.75));
    ri.append(&label(t(l, "Restart", "Neu starten"), "sg-body"));
    restart.set_child(Some(&ri));
    card.append(&restart);
    let lock_btn = if lock_allowed() {
        let s = hbox(0, "sg-sep");
        s.set_margin_start(46);
        card.append(&s);
        let b = gtk::Button::new();
        b.add_css_class("sg-listbtn");
        let bi = hbox(12, "");
        bi.append(&glyph_icon(Glyph::Lock, 18, 0xFB923C, 1.75));
        let txt = vbox(2, "");
        txt.append(&label(t(l, "Lock", "Sperren"), "sg-body"));
        txt.append(&label(t(l, "Stays running, disk stays unlocked", "Läuft weiter, Platte bleibt entsperrt"), "sg-hint warn"));
        bi.append(&txt);
        b.set_child(Some(&bi));
        card.append(&b);
        Some(b)
    } else {
        None
    };
    sheet.append(&card);
    let cancel = gtk::Button::with_label(t(l, "Cancel", "Abbrechen"));
    cancel.add_css_class("sg-plain");
    sheet.append(&cancel);
    backdrop.append(&sheet);
    let act = |w: &gtk::ApplicationWindow, what: &'static str| {
        let w = w.clone();
        move |_: &gtk::Button| {
            w.set_visible(false);
            match what {
                "instant" | "reboot" => run_poweroff(what),
                "lock" => {
                    let _ = Command::new(LOCK_CMD).spawn();
                }
                _ => {}
            }
            w.close();
        }
    };
    off.connect_clicked(act(&win, "instant"));
    restart.connect_clicked(act(&win, "reboot"));
    if let Some(b) = &lock_btn {
        b.connect_clicked(act(&win, "lock"));
    }
    cancel.connect_clicked(act(&win, "cancel"));
    let click = gtk::GestureClick::new();
    let w3 = win.clone();
    let bd = backdrop.clone();
    click.connect_released(move |_, _, x, y| {
        if let Some(hit) = bd.pick(x, y, gtk::PickFlags::DEFAULT) {
            if hit == bd.clone().upcast::<gtk::Widget>() {
                w3.close();
            }
        }
    });
    backdrop.add_controller(click);
    let keys = gtk::EventControllerKey::new();
    let w2 = win.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            w2.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    win.add_controller(keys);
    win.set_child(Some(&backdrop));
    win.fullscreen();
    win.present();
    cancel.grab_focus();
}

fn main() -> glib::ExitCode {
    let menu = std::env::args().any(|a| a == "--power-menu");
    glib::set_prgname(Some(if menu { "sentinel-power-menu" } else { "sentinel-settings" }));
    let _ = SESSION_APPLY;
    let app = gtk::Application::builder()
        .application_id(if menu { "org.sentinelx.PowerMenu" } else { APP_ID })
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(|_| setup_display());
    app.connect_activate(move |app| if menu { build_power_menu(app) } else { build_settings(app) });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}
