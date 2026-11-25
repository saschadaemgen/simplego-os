//! SimpleGo OS Shell, stage 2: top bar, sidebar and fullscreen Home menu.
//!
//! Top bar:  menu button (3x3 grid, aligned over the sidebar) | clock (time over date) |
//!           TIME, VOL and SERVER chips
//! Sidebar:  app launchers on top; at the bottom plain TOR / CPU / RAM / NET rows,
//!           separated by full-width horizontal lines.
//! Home:     opens in the content area (between the bars): search, all apps,
//!           status, messaging shortcut, notes, power.
//!
//! `sentinel-shell` runs the shell (layer-shell surfaces).
//! `sentinel-shell --preview [wallpaper.png] [--home]` renders the whole layout in one
//! window for screenshots on development machines.

mod srvinfo;
mod sys;
mod torpage;
mod windows;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use sys::State;

/// Per-launcher state the window tracker reads: the window ids that mean "this app is open",
/// the left-edge marker bar, and a "launching since" time set on click (pulses until the
/// window appears, cleared on appearance or after LOADING_TIMEOUT).
type RunMark = (Vec<String>, gtk::Box, gtk::Button, Rc<std::cell::Cell<Option<std::time::Instant>>>);
const LOADING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(25);

const APP_ID: &str = "org.sentinelx.Shell";
/// Wallpaper cut to the free area between the bars (written by the installer).
const DESK_WALLPAPER: &str = "/usr/share/backgrounds/sentinel-x-desk.png";
/// Gold ribbons (transparent) drawn above the star sky, so the stars twinkle behind them.
const DESK_RIBBONS: &str = "/usr/share/backgrounds/sentinel-x-ribbons.png";
const TOPBAR_H: i32 = 42;
const SIDEBAR_W: i32 = 64;

const DEFAULT_LAUNCHERS: &[&str] = &[
    "/usr/share/applications/simplex.desktop",
    "~/.local/share/applications/dock-tor.desktop",
    "~/.local/share/applications/dock-terminal.desktop",
    "~/.local/share/applications/dock-dateien.desktop",
];
const SIMPLEX_DESKTOP: &str = "/usr/share/applications/simplex.desktop";

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home_dir().join(rest),
        None => PathBuf::from(p),
    }
}

/// Launcher list: ~/.config/sentinel-shell/launchers.conf (one .desktop path per line).
fn launcher_paths() -> Vec<PathBuf> {
    let conf = home_dir().join(".config/sentinel-shell/launchers.conf");
    match std::fs::read_to_string(conf) {
        Ok(s) => s
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(expand)
            .collect(),
        Err(_) => DEFAULT_LAUNCHERS.iter().map(|p| expand(p)).collect(),
    }
}

fn notes_path() -> PathBuf {
    home_dir().join(".local/share/sentinel-shell/notes.txt")
}

fn set_state_class(label: &gtk::Label, state: Option<State>) {
    for c in ["ok", "wait", "fail"] {
        label.remove_css_class(c);
    }
    if let Some(s) = state {
        label.add_css_class(match s {
            State::Ok => "ok",
            State::Wait => "wait",
            State::Fail => "fail",
        });
    }
}

/// Crisp vector 3x3 dot grid for the menu button (no icon theme involved).
fn grid_icon() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(20);
    area.set_content_height(20);
    area.set_draw_func(|_, cr, w, h| {
        cr.set_source_rgb(0xd4 as f64 / 255.0, 0xaf as f64 / 255.0, 0x37 as f64 / 255.0);
        let step = 7.0;
        let ox = (w as f64 - 2.0 * step) / 2.0;
        let oy = (h as f64 - 2.0 * step) / 2.0;
        for i in 0..3 {
            for j in 0..3 {
                cr.arc(ox + i as f64 * step, oy + j as f64 * step, 2.2, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
            }
        }
    });
    area
}

/// Crisp vector power symbol for the shutdown button (no icon theme involved).
fn power_icon() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(18);
    area.set_content_height(18);
    area.set_draw_func(|da, cr, w, h| {
        let armed = da.ancestor(gtk::Button::static_type())
            .map(|b| b.has_css_class("armed"))
            .unwrap_or(false);
        if armed {
            cr.set_source_rgb(0x08 as f64 / 255.0, 0x0c as f64 / 255.0, 0x12 as f64 / 255.0);
        } else {
            cr.set_source_rgb(0xf8 as f64 / 255.0, 0x71 as f64 / 255.0, 0x71 as f64 / 255.0);
        }
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0 + 0.5);
        let r = 6.5;
        cr.set_line_width(2.0);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        let gap = 0.9;
        cr.arc(cx, cy, r, -std::f64::consts::FRAC_PI_2 + gap, -std::f64::consts::FRAC_PI_2 - gap + std::f64::consts::TAU);
        let _ = cr.stroke();
        cr.move_to(cx, cy - r - 1.5);
        cr.line_to(cx, cy - 1.0);
        let _ = cr.stroke();
    });
    area
}

/// privleap action that powers the Workstation off immediately (installed by the update script).
const POWEROFF_ACTION_CONF: &str = "/etc/privleap/conf.d/sentinel.conf";

/// Power dispatcher of SimpleGo OS: reads the user's power setting for a button and
/// powers off accordingly (instant, fast, normal, or the "Ask" menu).
const POWER_DISPATCHER: &str = "/usr/local/bin/sentinel-power";

fn can_power_off() -> bool {
    std::path::Path::new(POWER_DISPATCHER).exists()
        || std::path::Path::new(POWEROFF_ACTION_CONF).exists()
        || logind_allows("CanPowerOff")
}

/// Power off as fast as possible. The Workstation goes down at once; the host sees it
/// and powers the whole laptop off, so the disk is fully encrypted again.
/// Fallback without the privleap action: normal shutdown via logind.
fn power_off_now(trigger: &'static str) {
    std::thread::spawn(move || {
        // SimpleGo OS: the dispatcher decides (settings app, "Power" page).
        if std::path::Path::new(POWER_DISPATCHER).exists() {
            if std::process::Command::new(POWER_DISPATCHER).arg(trigger).status().map(|s| s.success()).unwrap_or(false) {
                return;
            }
        }
        let fast = std::path::Path::new(POWEROFF_ACTION_CONF).exists()
            && std::process::Command::new("leaprun")
                .arg("sentinel-poweroff-now")
                .status()
                .map(|st| st.success())
                .unwrap_or(false);
        if !fast {
            let _ = std::process::Command::new("systemctl").arg("poweroff").status();
        }
    });
}

/// Shutdown cell, top right: one click powers everything off. No confirmation.
fn power_button(trigger: &'static str) -> gtk::Button {
    let btn = gtk::Button::new();
    btn.add_css_class("sx-power");
    btn.set_size_request(SIDEBAR_W - 1, -1);
    btn.set_child(Some(&power_icon()));
    btn.set_tooltip_text(Some("Power off now"));
    btn.connect_clicked(move |b| {
        b.add_css_class("armed");
        if let Some(c) = b.child() {
            c.queue_draw();
        }
        power_off_now(trigger);
        // Still running after a few seconds (mode "Ask", or cancelled): back to normal.
        let b2 = b.clone();
        glib::timeout_add_seconds_local_once(4, move || b2.remove_css_class("armed"));
    });
    btn
}

/// Labels that the periodic update writes into.
#[derive(Default, Clone)]
struct Labels {
    clock: Option<gtk::Label>,
    date: Option<gtk::Label>,
    time_chip: Option<gtk::Label>,
    vol_chip: Option<gtk::Label>,
    server_chip: Option<gtk::Label>,
    /// Sidebar markers: (window app_ids of the launcher, bar). Short grey bar while the
    /// program has a window, tall gold bar while its window is the active one.
    run_marks: Vec<RunMark>,
    /// Context buttons for Tor Browser, left in the top bar, shown while it is active.
    tor_ctx: Option<gtk::Box>,
    /// Window controls (minimize, close) for the active window, shown while one is focused.
    winctl: Option<gtk::Box>,
    tor: Vec<gtk::Label>,
    time: Vec<gtk::Label>,
    cpu: Vec<gtk::Label>,
    ram: Vec<gtk::Label>,
    net: Vec<gtk::Label>,
}

// ---------------------------------------------------------------- top bar

/// Status field in the top bar, built like the sidebar rows: value on top, key below.
fn seg(key: &str) -> (gtk::Box, gtk::Label) {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.add_css_class("sx-seg");
    b.set_valign(gtk::Align::Fill);
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 0);
    inner.set_valign(gtk::Align::Center);
    inner.set_vexpand(true);
    let v = gtk::Label::new(Some("..."));
    v.add_css_class("sx-seg-val");
    let k = gtk::Label::new(Some(key));
    k.add_css_class("sx-seg-key");
    inner.append(&v);
    inner.append(&k);
    b.append(&inner);
    (b, v)
}

/// Clock in the middle of the top bar: time on top, weekday and date below, centred.
/// Tabular digits keep the width steady while the seconds change.
fn bar_clock() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.add_css_class("sx-clock");
    b.set_valign(gtk::Align::Center);
    let time = gtk::Label::new(None);
    time.add_css_class("sx-clock-time");
    let date = gtk::Label::new(None);
    date.add_css_class("sx-clock-date");
    b.append(&time);
    b.append(&date);
    let update = move || {
        if let Ok(n) = glib::DateTime::now_local() {
            let t = n.format("%H:%M:%S").map(|s| s.to_string()).unwrap_or_default();
            let d = n.format("%a  %d.%m.%Y").map(|s| s.to_uppercase()).unwrap_or_default();
            if time.text() != t {
                time.set_text(&t);
            }
            if date.text() != d {
                date.set_text(&d);
            }
        }
    };
    update();
    // Checked five times a second, so the seconds flip close to the real second boundary.
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        update();
        glib::ControlFlow::Continue
    });
    b
}

/// Full-height vertical divider, the top bar counterpart of the sidebar lines.
fn vline() -> gtk::Box {
    let l = gtk::Box::new(gtk::Orientation::Vertical, 0);
    l.add_css_class("sx-vline");
    l
}

fn build_topbar(labels: &mut Labels, on_menu: Rc<dyn Fn()>) -> (gtk::Widget, gtk::Button, gtk::Box) {
    let bar = gtk::CenterBox::new();
    bar.add_css_class("sx-topbar-box");
    bar.set_height_request(TOPBAR_H);

    // Left: menu cell, exactly as wide as the sidebar, closed by a divider.
    let start = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let menu = gtk::Button::new();
    menu.add_css_class("sx-menu");
    menu.set_child(Some(&grid_icon()));
    menu.set_size_request(SIDEBAR_W - 1, -1);
    menu.set_tooltip_text(Some("Home"));
    menu.connect_clicked(move |_| on_menu());
    start.append(&menu);
    // Continues the sidebar border upwards, so both edges form one clean corner.
    let edge = vline();
    edge.add_css_class("edge");
    start.append(&edge);
    // Context buttons for Tor Browser, visible while its window is the active one.
    let ctx = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    ctx.set_visible(false);
    let actions: [(&str, &str, &str, fn()); 3] = [
        ("view-refresh-symbolic", "NEW CIRCUIT", "New Tor circuit for this site (Ctrl+Shift+L)", torpage::new_circuit),
        ("avatar-default-symbolic", "NEW IDENTITY", "New identity: closes all tabs and starts fresh (Ctrl+Shift+U)", torpage::new_identity),
        ("window-new-symbolic", "NEW WINDOW", "New Tor Browser window", torpage::new_window),
    ];
    for (icon, label, tip, action) in actions {
        let b = gtk::Button::new();
        b.add_css_class("sx-ctx-btn");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let img = gtk::Image::from_icon_name(icon);
        img.set_pixel_size(16);
        img.add_css_class("sx-ctx-icon");
        let l = gtk::Label::new(Some(label));
        l.add_css_class("sx-ctx-label");
        row.append(&img);
        row.append(&l);
        b.set_child(Some(&row));
        b.set_tooltip_text(Some(tip));
        b.connect_clicked(move |_| action());
        ctx.append(&b);
        ctx.append(&vline());
    }
    start.append(&ctx);
    labels.tor_ctx = Some(ctx);

    // Window controls for the active window (minimize, close). Hidden until the tracker sees
    // a focused toplevel; they act on whatever window is active (sx-wctl, wlrctl/wtype).
    let winctl = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    winctl.set_visible(false);
    let win_actions: [(&str, &str, &str); 2] = [
        ("window-minimize-symbolic", "Minimize", "min"),
        ("window-close-symbolic", "Close", "close"),
    ];
    for (icon, tip, action) in win_actions {
        let b = gtk::Button::new();
        b.add_css_class("sx-ctx-btn");
        b.add_css_class("sx-winctl-btn");
        let img = gtk::Image::from_icon_name(icon);
        img.set_pixel_size(16);
        img.add_css_class("sx-ctx-icon");
        b.set_child(Some(&img));
        b.set_tooltip_text(Some(tip));
        let act = action.to_string();
        b.connect_clicked(move |_| win_ctl(&act));
        winctl.append(&b);
        winctl.append(&vline());
    }
    start.append(&winctl);
    labels.winctl = Some(winctl);
    bar.set_start_widget(Some(&start));

    let mid = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mid.set_valign(gtk::Align::Center);
    // Time and date in two lines, like the Windows taskbar.
    mid.append(&bar_clock());
    bar.set_center_widget(Some(&mid));

    // Right: status fields and the shutdown cell, mirroring the menu cell.
    let end = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let (time_seg, time_val) = seg("TIME");
    time_seg.set_tooltip_text(Some("Time sync over Tor"));
    let (vol_seg, vol_val) = seg("VOL");
    vol_seg.set_tooltip_text(Some("Volume: scroll = louder/quieter, click = mute"));
    let click = gtk::GestureClick::new();
    click.connect_released(|_, _, _, _| sys::volume_change(None));
    vol_seg.add_controller(click);
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    scroll.connect_scroll(|_, _, dy| {
        sys::volume_change(Some(if dy < 0.0 { "5%+" } else { "5%-" }));
        glib::Propagation::Stop
    });
    vol_seg.add_controller(scroll);
    // Server monitoring: verdict of the last report. Hover shows the info panel,
    // a click opens SimpleGo Server (both wired up in shell()).
    let (srv_seg, srv_val) = seg("SERVER");
    end.append(&vline());
    end.append(&time_seg);
    end.append(&vline());
    end.append(&vol_seg);
    end.append(&vline());
    end.append(&srv_seg);
    // The most important control: always visible, top right.
    if can_power_off() {
        end.append(&vline());
        end.append(&power_button("topbar"));
    }
    bar.set_end_widget(Some(&end));

    labels.time_chip = Some(time_val);
    labels.vol_chip = Some(vol_val);
    labels.server_chip = Some(srv_val);
    (bar.upcast(), menu, srv_seg)
}

// ---------------------------------------------------------------- sidebar

fn hline() -> gtk::Box {
    let l = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    l.add_css_class("sx-side-line");
    l
}

/// PID of a running SimpleGo app (Clean Drop, SimpleGo Server). Each app records its PID
/// in $XDG_RUNTIME_DIR/<binary>.pid; a finished or reused PID does not count.
fn own_app_pid(bin: &str) -> Option<u32> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let pid: u32 = std::fs::read_to_string(dir.join(format!("{bin}.pid"))).ok()?.trim().parse().ok()?;
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    // The kernel keeps 15 characters of the name; development builds run as cleandrop/serverapp.
    let short: String = bin.chars().take(15).collect();
    let dev = match bin { "sentinel-server" => "serverapp", "sentinel-cleandrop" => "cleandrop", _ => "" };
    if comm.trim() != short && comm.trim() != dev {
        return None;
    }
    // Field 3 of /proc/PID/stat is the state; Z = finished but not reaped.
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let running = stat.rsplit(')').next().map(|r| !r.trim_start().starts_with('Z')).unwrap_or(false);
    running.then_some(pid)
}

/// Closes a SimpleGo app gracefully (SIGTERM; the apps quit cleanly on it).
fn close_own_app(pid: u32) {
    let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
}

/// The sidebar's Tor icon (Whonix starter or torbrowser).
fn is_tor_launcher(path: &std::path::Path, bin: &str) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    name == "dock-tor.desktop" || bin == "torbrowser" || bin == "desktop-starter-wrapper"
}

/// Window app_ids a launcher's program uses (lower case), for the running markers.
fn window_ids(path: &std::path::Path, info: &gio::DesktopAppInfo, bin: &str) -> Vec<String> {
    let name = path.file_stem().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut ids: Vec<String> = match (name.as_str(), bin) {
        (_, "sentinel-cleandrop") => vec!["org.sentinelx.cleandrop".into()],
        (_, "sentinel-server") => vec!["org.sentinelx.server".into()],
        ("dock-tor", _) | (_, "torbrowser") => vec![torpage::TB_APP_ID.to_lowercase()],
        ("dock-terminal", _) => vec!["qterminal".into()],
        ("dock-dateien", _) => vec!["pcmanfm-qt".into()],
        ("simplex", _) => vec!["simplex".into()],
        _ => Vec::new(),
    };
    if let Some(c) = info.startup_wm_class() {
        ids.push(c.to_lowercase());
    }
    if !bin.is_empty() {
        ids.push(bin.to_lowercase());
    }
    ids
}

/// Binary name of a launcher, e.g. "sentinel-cleandrop" for Clean Drop.
fn launcher_bin(info: &gio::DesktopAppInfo) -> String {
    info.commandline()
        .and_then(|c| c.to_str().and_then(|s| s.split_whitespace().next()).map(str::to_string))
        .map(|b| b.rsplit('/').next().unwrap_or("").to_string())
        .unwrap_or_default()
}

/// SERVER field click: opens SimpleGo Server, or closes it if it is already open.
fn toggle_server_app() {
    match own_app_pid("sentinel-server") {
        Some(pid) => close_own_app(pid),
        None => open_server_app(),
    }
}

/// Starts SimpleGo Server. The app keeps itself to one window; the child is reaped
/// in the background so no finished process is left behind.
fn open_server_app() {
    if let Ok(mut child) = std::process::Command::new("sentinel-server").spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

fn launch(info: &gio::DesktopAppInfo) {
    let _ = info.launch(&[], gio::AppLaunchContext::NONE);
}

/// Runs the window-control helper (sx-wctl) on the active window, fire and forget.
fn win_ctl(action: &str) {
    if let Ok(mut c) = std::process::Command::new("sx-wctl").arg(action).spawn() {
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
}

/// Brings an app's existing window forward, restoring it if it was minimized.
fn win_focus(app_id: &str) {
    if let Ok(mut c) = std::process::Command::new("sx-wctl").arg("restore").arg(app_id).spawn() {
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
}

/// The exact app_id of an open window belonging to this launcher, if one exists.
fn open_window_id(ids: &[String]) -> Option<String> {
    windows::query()?.open.into_iter().find(|a| windows::matches(a, ids))
}

fn build_sidebar(labels: &mut Labels, on_tor: Rc<dyn Fn()>) -> gtk::Widget {
    let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
    side.add_css_class("sx-sidebar-box");
    side.set_width_request(SIDEBAR_W);

    let launchers = gtk::Box::new(gtk::Orientation::Vertical, 0);
    launchers.set_margin_top(10);
    for path in launcher_paths() {
        let Some(info) = gio::DesktopAppInfo::from_filename(&path) else { continue };
        let btn = gtk::Button::new();
        btn.add_css_class("sx-launcher");
        let img = match info.icon() {
            Some(icon) => gtk::Image::from_gicon(&icon),
            None => gtk::Image::from_icon_name("application-x-executable"),
        };
        img.set_pixel_size(34);
        btn.set_child(Some(&img));
        btn.set_tooltip_text(Some(info.name().as_str()));
        // Our own apps toggle: a second click closes them. The Tor icon opens the Tor
        // page. Other programs just start.
        let bin = launcher_bin(&info);
        let own = bin.starts_with("sentinel-");
        let is_tor = is_tor_launcher(&path, &bin);
        let ids = window_ids(&path, &info, &bin);
        // Running marker: a bar on the left edge of the sidebar, next to the icon.
        let row = gtk::Overlay::new();
        row.set_child(Some(&btn));
        let mark = gtk::Box::new(gtk::Orientation::Vertical, 0);
        mark.add_css_class("sx-run-bar");
        mark.set_halign(gtk::Align::Start);
        mark.set_valign(gtk::Align::Center);
        mark.set_can_target(false);
        mark.set_visible(false);
        row.add_overlay(&mark);
        // Loading state: set on a click that starts a program, so the whole icon blinks until
        // the window shows up. The window tracker clears it.
        let loading = Rc::new(std::cell::Cell::new(None::<std::time::Instant>));
        let start_loading = {
            let btn = btn.clone();
            let loading = loading.clone();
            move || {
                loading.set(Some(std::time::Instant::now()));
                btn.add_css_class("sx-blink-off");
            }
        };
        if is_tor {
            let on_tor = on_tor.clone();
            btn.connect_clicked(move |_| on_tor());
        } else {
            let bin = bin.clone();
            let ids_cl = ids.clone();
            btn.connect_clicked(move |_| {
                // Our own apps toggle: a second click closes them.
                if own {
                    match own_app_pid(&bin) {
                        Some(pid) => close_own_app(pid),
                        None => {
                            start_loading();
                            launch(&info);
                        }
                    }
                    return;
                }
                // Other apps: if a window is already open, bring it forward (restoring it if it
                // was minimized); otherwise start the program.
                match open_window_id(&ids_cl) {
                    Some(app_id) => win_focus(&app_id),
                    None => {
                        start_loading();
                        launch(&info);
                    }
                }
            });
        }
        labels.run_marks.push((ids, mark, btn.clone(), loading));
        launchers.append(&row);
    }
    side.append(&launchers);

    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    side.append(&spacer);

    // System section: rows straight on the sidebar, lines across the full width.
    let rows: [(&str, fn(&mut Labels) -> &mut Vec<gtk::Label>); 4] = [
        ("TOR", |l| &mut l.tor),
        ("CPU", |l| &mut l.cpu),
        ("RAM", |l| &mut l.ram),
        ("NET", |l| &mut l.net),
    ];
    for (key, slot) in rows {
        side.append(&hline());
        let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
        row.add_css_class("sx-sys-row");
        let v = gtk::Label::new(Some("..."));
        v.add_css_class("sx-sys-val");
        let k = gtk::Label::new(Some(key));
        k.add_css_class("sx-sys-key");
        row.append(&v);
        row.append(&k);
        side.append(&row);
        slot(labels).push(v);
    }
    // Same power cell as top right, closing the sidebar at the bottom.
    if can_power_off() {
        side.append(&hline());
        let p = power_button("sidebar");
        p.set_size_request(-1, TOPBAR_H - 1);
        side.append(&p);
    }
    side.set_tooltip_text(Some("TOR: ON = connected, WAIT = connecting, OFF = no connection"));
    side.upcast()
}

// ---------------------------------------------------------------- home

fn card(title: &str) -> gtk::Box {
    let c = gtk::Box::new(gtk::Orientation::Vertical, 0);
    c.add_css_class("sx-card");
    let t = gtk::Label::new(Some(title));
    t.add_css_class("sx-card-title");
    t.set_halign(gtk::Align::Start);
    c.append(&t);
    c
}

fn stat_row(parent: &gtk::Box, key: &str) -> gtk::Label {
    let row = gtk::CenterBox::new();
    row.add_css_class("sx-stat-row");
    let k = gtk::Label::new(Some(key));
    k.add_css_class("sx-stat-key");
    let v = gtk::Label::new(Some("..."));
    v.add_css_class("sx-stat-val");
    row.set_start_widget(Some(&k));
    row.set_end_widget(Some(&v));
    parent.append(&row);
    v
}

/// Asks logind whether the session may run an action without a password prompt.
fn logind_allows(method: &str) -> bool {
    if std::env::var_os("SX_PREVIEW_POWER").is_some() {
        return true;
    }
    std::process::Command::new("busctl")
        .args(["call", "org.freedesktop.login1", "/org/freedesktop/login1",
               "org.freedesktop.login1.Manager", method])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("\"yes\""))
        .unwrap_or(false)
}

fn build_home(labels: &mut Labels, hide: Rc<dyn Fn()>) -> (gtk::Widget, gtk::SearchEntry) {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 32);
    root.add_css_class("sx-home-box");

    // Left: search and all apps.
    let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
    left.set_hexpand(true);
    let search = gtk::SearchEntry::new();
    search.add_css_class("sx-search");
    search.set_placeholder_text(Some("Search apps"));
    search.set_halign(gtk::Align::Start);
    search.set_width_request(520);
    left.append(&search);
    let sec = gtk::Label::new(Some("APPS"));
    sec.add_css_class("sx-section");
    sec.set_halign(gtk::Align::Start);
    left.append(&sec);

    let flow = gtk::FlowBox::new();
    flow.add_css_class("sx-apps");
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_homogeneous(true);
    flow.set_max_children_per_line(12);
    flow.set_min_children_per_line(3);
    flow.set_row_spacing(6);
    flow.set_column_spacing(6);
    flow.set_valign(gtk::Align::Start);
    let mut apps: Vec<gio::AppInfo> = gio::AppInfo::all().into_iter().filter(|a| a.should_show()).collect();
    apps.sort_by_key(|a| a.display_name().to_lowercase());
    for app in apps {
        let btn = gtk::Button::new();
        btn.add_css_class("sx-app");
        let v = gtk::Box::new(gtk::Orientation::Vertical, 0);
        v.set_valign(gtk::Align::Center);
        let img = match app.icon() {
            Some(icon) => gtk::Image::from_gicon(&icon),
            None => gtk::Image::from_icon_name("application-x-executable"),
        };
        img.set_pixel_size(48);
        let name = gtk::Label::new(Some(app.display_name().as_str()));
        name.add_css_class("sx-app-name");
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.set_max_width_chars(14);
        v.append(&img);
        v.append(&name);
        btn.set_child(Some(&v));
        btn.set_widget_name(&app.display_name().to_lowercase());
        let h = hide.clone();
        btn.connect_clicked(move |_| {
            let _ = app.launch(&[], gio::AppLaunchContext::NONE);
            h();
        });
        flow.append(&btn);
    }
    let s2 = search.clone();
    flow.set_filter_func(move |child| {
        let q = s2.text().to_lowercase();
        q.is_empty() || child.child().map(|c| c.widget_name().contains(q.as_str())).unwrap_or(true)
    });
    let f2 = flow.clone();
    search.connect_search_changed(move |_| f2.invalidate_filter());
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&flow));
    left.append(&scroll);
    root.append(&left);

    // Right: widgets.
    let right = gtk::Box::new(gtk::Orientation::Vertical, 16);
    right.set_width_request(360);
    right.set_hexpand(false);

    let status = card("STATUS");
    labels.tor.push(stat_row(&status, "TOR"));
    labels.time.push(stat_row(&status, "TIME SYNC"));
    labels.cpu.push(stat_row(&status, "CPU"));
    labels.ram.push(stat_row(&status, "RAM"));
    labels.net.push(stat_row(&status, "NET"));
    right.append(&status);

    if let Some(simplex) = gio::DesktopAppInfo::from_filename(SIMPLEX_DESKTOP) {
        let msg = card("MESSAGING");
        let b = gtk::Button::with_label("Open SimpleX");
        b.add_css_class("sx-action");
        let h = hide.clone();
        b.connect_clicked(move |_| {
            launch(&simplex);
            h();
        });
        msg.append(&b);
        right.append(&msg);
    }

    let notes = card("NOTES");
    notes.set_vexpand(true);
    let tv = gtk::TextView::new();
    tv.add_css_class("sx-notes");
    tv.set_wrap_mode(gtk::WrapMode::WordChar);
    tv.set_vexpand(true);
    let buf = tv.buffer();
    buf.set_text(&std::fs::read_to_string(notes_path()).unwrap_or_default());
    buf.connect_changed(|b| {
        let text = b.text(&b.start_iter(), &b.end_iter(), false);
        let p = notes_path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, text.as_str());
    });
    let ns = gtk::ScrolledWindow::new();
    ns.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    ns.set_vexpand(true);
    ns.set_child(Some(&tv));
    notes.append(&ns);
    right.append(&notes);

    // Power: only actions logind allows without a password prompt are shown.
    let power = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    power.set_homogeneous(true);
    if logind_allows("CanReboot") {
        let b = gtk::Button::with_label("Restart");
        b.add_css_class("sx-action");
        b.add_css_class("danger");
        b.connect_clicked(|_| {
            let _ = std::process::Command::new("systemctl").arg("reboot").spawn();
        });
        power.append(&b);
    }
    if can_power_off() {
        let b = gtk::Button::with_label("Power off");
        b.add_css_class("sx-action");
        b.add_css_class("danger");
        b.connect_clicked(|_| power_off_now("home"));
        power.append(&b);
    }
    if power.first_child().is_some() {
        right.append(&power);
    }
    root.append(&right);

    (root.upcast(), search)
}

// ---------------------------------------------------------------- updates

/// Every second: which programs have windows and which one is active (wlrctl, off the
/// main thread). Updates the sidebar markers and shows the Tor Browser buttons.
fn start_window_tracking(marks: Vec<RunMark>, tor_ctx: Option<gtk::Box>, winctl: Option<gtk::Box>) {
    let busy = Rc::new(std::cell::Cell::new(false));
    // Number of Tor Browser windows last time: a new one gets maximized.
    let tb_count = Rc::new(std::cell::Cell::new(0usize));
    // Blink the icon of a program that is still launching, until its window appears. GTK4 CSS
    // animations are unreliable, so the blink is driven from here (dim every other tick).
    {
        let blink = marks.clone();
        let dim = Rc::new(std::cell::Cell::new(false));
        glib::timeout_add_local(std::time::Duration::from_millis(750), move || {
            let on = !dim.get();
            dim.set(on);
            for (_, _, btn, loading) in &blink {
                if loading.get().is_some() {
                    if on { btn.add_css_class("sx-blink-off") } else { btn.remove_css_class("sx-blink-off") }
                }
            }
            glib::ControlFlow::Continue
        });
    }
    glib::timeout_add_local(std::time::Duration::from_millis(800), move || {
        if busy.replace(true) {
            return glib::ControlFlow::Continue;
        }
        let (marks, tor_ctx, winctl, busy, tb_count) = (marks.clone(), tor_ctx.clone(), winctl.clone(), busy.clone(), tb_count.clone());
        glib::spawn_future_local(async move {
            if let Ok(Some(w)) = gio::spawn_blocking(windows::query).await {
                let n = w.open.iter().filter(|a| a.as_str() == torpage::TB_APP_ID).count();
                if n > tb_count.get() {
                    torpage::maximize_windows();
                }
                tb_count.set(n);
                for (ids, mark, btn, loading) in &marks {
                    let active = w.active.as_deref().map(|a| windows::matches(a, ids)).unwrap_or(false);
                    let running = w.open.iter().any(|a| windows::matches(a, ids));
                    // The window appeared, or the launch gave up: stop the blink.
                    let waiting = match loading.get() {
                        Some(_) if running => { loading.set(None); false }
                        Some(t) if t.elapsed() > LOADING_TIMEOUT => { loading.set(None); false }
                        Some(_) => true,
                        None => false,
                    };
                    if waiting {
                        // The icon pulses (blink timer); also show the steady running bar so the
                        // program reads as present while it is starting.
                        mark.remove_css_class("active");
                        mark.set_visible(true);
                    } else {
                        btn.remove_css_class("sx-blink-off");
                        mark.set_visible(running);
                        if active { mark.add_css_class("active") } else { mark.remove_css_class("active") }
                    }
                }
                if let Some(c) = &tor_ctx {
                    c.set_visible(w.active.as_deref() == Some(torpage::TB_APP_ID));
                }
                if let Some(c) = &winctl {
                    c.set_visible(w.active.is_some());
                }
            }
            busy.set(false);
        });
        glib::ControlFlow::Continue
    });
}

fn start_updates(labels: Labels) {
    start_window_tracking(labels.run_marks.clone(), labels.tor_ctx.clone(), labels.winctl.clone());
    let tor = sys::TorCheck::start();
    let sampler = Rc::new(RefCell::new(sys::Sampler::default()));
    let tick = Rc::new(RefCell::new(0u32));
    let set_all = |ls: &[gtk::Label], text: &str, state: Option<State>| {
        for l in ls {
            l.set_text(text);
            set_state_class(l, state);
        }
    };
    let update = move || {
        if let Ok(n) = glib::DateTime::now_local() {
            if let Some(c) = &labels.clock {
                c.set_text(&n.format("%H:%M").map(|s| s.to_string()).unwrap_or_default());
            }
            if let Some(d) = &labels.date {
                d.set_text(&n.format("%a %d %b").map(|s| s.to_string()).unwrap_or_default());
            }
        }
        let mut t = tick.borrow_mut();
        *t += 1;
        if *t % 2 == 0 {
            return;
        }
        let mut s = sampler.borrow_mut();
        set_all(&labels.cpu, &format!("{}%", s.cpu_percent()), None);
        set_all(&labels.ram, &format!("{}%", sys::ram_percent()), None);
        set_all(&labels.net, &format!("{}/s", sys::fmt_rate(s.net_bytes_per_sec())), None);

        let ts = tor.state();
        let tor_txt = match ts { State::Ok => "ON", State::Wait => "WAIT", State::Fail => "OFF" };
        set_all(&labels.tor, tor_txt, Some(ts));

        let st = sys::time_sync();
        let time_txt = match st { State::Ok => "OK", State::Wait => "SYNC", State::Fail => "ERR" };
        set_all(&labels.time, time_txt, Some(st));
        if let Some(c) = &labels.time_chip {
            c.set_text(time_txt);
            set_state_class(c, Some(st));
        }

        if let Some(c) = &labels.server_chip {
            let (txt, st) = match sys::server_level().as_deref() {
                Some("ok") => ("OK", Some(State::Ok)),
                Some("warn") => ("WARN", Some(State::Wait)),
                Some("fail") => ("FAIL", Some(State::Fail)),
                Some("lost") => ("LOST", None),
                Some(_) => ("?", None),
                None => ("N/A", None),
            };
            c.set_text(txt);
            set_state_class(c, st);
        }

        if let Some(c) = &labels.vol_chip {
            match sys::volume() {
                Some((_, true)) => {
                    c.set_text("MUTE");
                    set_state_class(c, Some(State::Fail));
                }
                Some((v, false)) => {
                    c.set_text(&format!("{v}%"));
                    set_state_class(c, None);
                }
                None => {
                    c.set_text("N/A");
                    set_state_class(c, None);
                }
            }
        }
    };
    update();
    glib::timeout_add_seconds_local(1, move || {
        update();
        glib::ControlFlow::Continue
    });
}

// ---------------------------------------------------------------- windows

fn setup_display() {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(true);
        // No animations anywhere: the workstation has no graphics acceleration.
        settings.set_gtk_enable_animations(false);
        let theme_ok = ["/usr/share/icons/Papirus-Dark", "/usr/share/icons/Papirus"]
            .iter()
            .find(|p| std::path::Path::new(p).is_dir());
        if let Some(p) = theme_ok {
            let name = p.rsplit('/').next().unwrap_or("Papirus-Dark");
            settings.set_gtk_icon_theme_name(Some(name));
        }
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_USER);
        // Development only: SX_EXTRA_CSS=<file> layers design variants on top for screenshots.
        if let Some(extra) = std::env::var_os("SX_EXTRA_CSS") {
            let p2 = gtk::CssProvider::new();
            p2.load_from_path(extra);
            gtk::style_context_add_provider_for_display(&display, &p2, gtk::STYLE_PROVIDER_PRIORITY_USER + 1);
        }
    }
}

#[cfg(feature = "layer-shell")]
mod layer {
    use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

    /// Bars live on the overlay layer, so they always stay above the Home surface
    /// and the menu button can always be reached.
    pub fn bar(win: &gtk::ApplicationWindow, top: bool) {
        win.init_layer_shell();
        win.set_layer(Layer::Overlay);
        win.set_namespace(if top { "sentinel-topbar" } else { "sentinel-sidebar" });
        let edges: &[Edge] = if top {
            &[Edge::Top, Edge::Left, Edge::Right]
        } else {
            &[Edge::Top, Edge::Bottom, Edge::Left]
        };
        for e in edges {
            win.set_anchor(*e, true);
        }
        win.auto_exclusive_zone_enable();
    }

    /// Home fills the area left free by the bars. It stays mapped all the time:
    /// parked on the bottom layer it is the desktop (SimpleGo), raised to the
    /// top layer it is the menu. Opening is a layer switch, not a new window.
    pub fn home(win: &gtk::ApplicationWindow) {
        win.init_layer_shell();
        win.set_namespace("sentinel-home");
        for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            win.set_anchor(e, true);
        }
        win.set_exclusive_zone(0);
        park(win);
    }

    pub fn raise(win: &gtk::ApplicationWindow) {
        win.set_layer(Layer::Top);
        win.set_keyboard_mode(KeyboardMode::Exclusive);
    }

    pub fn park(win: &gtk::ApplicationWindow) {
        win.set_layer(Layer::Bottom);
        win.set_keyboard_mode(KeyboardMode::None);
    }
}

/// Appends one line to ~/.cache/sentinel-shell/timing.log (best effort).
fn timing_log(line: &str) {
    use std::io::Write;
    let dir = glib::user_cache_dir().join("sentinel-shell");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("timing.log")) {
        let _ = writeln!(f, "{} {line}", glib::DateTime::now_local().map(|d| d.format("%F %T").map(|s| s.to_string()).unwrap_or_default()).unwrap_or_default());
    }
}

/// Logs how long GTK needs until the next frame is painted (open/close cost).
fn measure_next_paint(win: &gtk::ApplicationWindow, what: &'static str) {
    let Some(fc) = win.frame_clock() else { return };
    let start = std::time::Instant::now();
    let id: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::new(RefCell::new(None));
    let id2 = id.clone();
    let handler = fc.connect_after_paint(move |fc| {
        timing_log(&format!("{what}: painted after {} ms", start.elapsed().as_millis()));
        if let Some(h) = id2.borrow_mut().take() {
            fc.disconnect(h);
        }
    });
    *id.borrow_mut() = Some(handler);
}

/// Desktop page shown while Home is parked: the SimpleGo wallpaper, centered in
/// the free area. Drawn by the Home window itself, so nothing depends on transparency.
fn build_desk() -> gtk::Widget {
    let pic = gtk::Picture::for_filename(DESK_WALLPAPER);
    pic.set_content_fit(gtk::ContentFit::Cover);
    pic.set_can_shrink(true);
    pic.set_hexpand(true);
    pic.set_vexpand(true);
    // Twinkling gold stars between the satin and the ribbons (small widgets, 12 updates per second).
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&pic));
    let sky = gtk::Fixed::new();
    sky.set_can_target(false);
    overlay.add_overlay(&sky);
    stars::start(&sky);
    // The same picture as the boot and lock screens: satin below, stars, ribbons on top.
    if std::path::Path::new(DESK_RIBBONS).exists() {
        let ribbons = gtk::Picture::for_filename(DESK_RIBBONS);
        ribbons.set_content_fit(gtk::ContentFit::Cover);
        ribbons.set_can_shrink(true);
        ribbons.set_can_target(false);
        overlay.add_overlay(&ribbons);
    }
    overlay.upcast()
}

/// Twinkling stars on the desktop: every star breathes slowly at its own pace, and every
/// few seconds one of them flares up. Positions are fractions of the free area, so they
/// follow any screen size. No randomness crate: a small xorshift seeded from the clock.
mod stars {
    use gtk::prelude::*;
    use gtk::glib;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    const COUNT: usize = 110;

    struct Star {
        w: gtk::Box,
        fx: f64,
        fy: f64,
        base: f64,
        speed: f64,
        phase: f64,
        flare_until: f64,
    }

    struct Sky {
        stars: Vec<Star>,
        size: (i32, i32),
        seed: u64,
        next_flare: f64,
    }

    fn rnd(seed: &mut u64) -> f64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn start(fixed: &gtk::Fixed) {
        let mut seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e3779b97f4a7c15)
            | 1;
        let mut stars = Vec::with_capacity(COUNT);
        for _ in 0..COUNT {
            let r = rnd(&mut seed);
            let (d, class) = if r < 0.55 { (2, "sx-star-s") } else if r < 0.9 { (2, "sx-star-m") } else { (3, "sx-star-l") };
            let w = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            w.add_css_class("sx-star");
            w.add_css_class(class);
            w.set_size_request(d, d);
            w.set_can_target(false);
            w.set_opacity(0.0);
            fixed.put(&w, 0.0, 0.0);
            stars.push(Star {
                w,
                fx: rnd(&mut seed),
                fy: rnd(&mut seed),
                base: 0.18 + 0.5 * rnd(&mut seed),
                speed: 0.35 + 1.1 * rnd(&mut seed),
                phase: rnd(&mut seed) * std::f64::consts::TAU,
                flare_until: 0.0,
            });
        }
        let sky = Rc::new(RefCell::new(Sky { stars, size: (0, 0), seed, next_flare: 2.0 }));
        let t0 = Instant::now();
        let fixed = fixed.clone();
        glib::timeout_add_local(Duration::from_millis(80), move || {
            let t = t0.elapsed().as_secs_f64();
            let mut s = sky.borrow_mut();
            let size = (fixed.width(), fixed.height());
            if size != s.size && size.0 > 0 && size.1 > 0 {
                s.size = size;
                for st in &s.stars {
                    fixed.move_(&st.w, st.fx * size.0 as f64, st.fy * size.1 as f64);
                }
            }
            if t > s.next_flare {
                let mut seed = s.seed;
                let i = (rnd(&mut seed) * COUNT as f64) as usize % COUNT;
                s.next_flare = t + 1.5 + 2.5 * rnd(&mut seed);
                s.seed = seed;
                s.stars[i].flare_until = t + 0.9;
                s.stars[i].w.add_css_class("sx-star-flare");
            }
            for st in &mut s.stars {
                let mut o = st.base * (0.5 + 0.5 * (t * st.speed + st.phase).sin());
                if st.flare_until > 0.0 {
                    let left = st.flare_until - t;
                    if left > 0.0 {
                        o = o.max((left / 0.9 * std::f64::consts::PI).sin().abs());
                    } else {
                        st.flare_until = 0.0;
                        st.w.remove_css_class("sx-star-flare");
                    }
                }
                st.w.set_opacity(o.clamp(0.0, 1.0));
            }
            glib::ControlFlow::Continue
        });
    }
}

fn shell(app: &gtk::Application) {
    let mut labels = Labels::default();

    // Bars first, so Home is laid out with their reserved space from the start.
    let top_win = gtk::ApplicationWindow::new(app);
    top_win.add_css_class("sx-topbar");
    let side_win = gtk::ApplicationWindow::new(app);
    side_win.add_css_class("sx-sidebar");

    let home_win = gtk::ApplicationWindow::new(app);
    home_win.add_css_class("sx-home");
    let open = Rc::new(std::cell::Cell::new(false));
    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::None);
    stack.set_transition_duration(0);

    // Closing: switch to the desktop page and park on the bottom layer.
    let hw = home_win.clone();
    let st = stack.clone();
    let o1 = open.clone();
    let hide: Rc<dyn Fn()> = Rc::new(move || {
        st.set_visible_child_name("desk");
        hw.add_css_class("sx-parked");
        #[cfg(feature = "layer-shell")]
        layer::park(&hw);
        #[cfg(not(feature = "layer-shell"))]
        hw.set_visible(false);
        if o1.get() {
            measure_next_paint(&hw, "close");
        }
        o1.set(false);
    });
    let (home_w, search) = build_home(&mut labels, hide.clone());
    let tor_page = Rc::new(torpage::build(hide.clone()));
    stack.add_named(&build_desk(), Some("desk"));
    stack.add_named(&home_w, Some("home"));
    stack.add_named(&tor_page.widget, Some("tor"));
    home_win.set_child(Some(&stack));
    #[cfg(feature = "layer-shell")]
    layer::home(&home_win);
    #[cfg(not(feature = "layer-shell"))]
    home_win.set_default_size(1200, 680);

    // Esc closes Home. Capture phase, so the search field cannot swallow it.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let h2 = hide.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            h2();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    home_win.add_controller(keys);
    let h4 = hide.clone();
    search.connect_stop_search(move |_| h4());

    let hw2 = home_win.clone();
    let st2 = stack.clone();
    let o2 = open.clone();
    let h3 = hide.clone();
    // Opens a page of the Home surface ("home" = menu, "tor" = Tor page). The same
    // button again, while its page is open, closes it.
    let tp = tor_page.clone();
    let open_page: Rc<dyn Fn(&str)> = Rc::new(move |name: &str| {
        if o2.get() && st2.visible_child_name().as_deref() == Some(name) {
            h3();
            return;
        }
        hw2.remove_css_class("sx-parked");
        st2.set_visible_child_name(name);
        #[cfg(feature = "layer-shell")]
        layer::raise(&hw2);
        #[cfg(not(feature = "layer-shell"))]
        hw2.present();
        if name == "tor" {
            tp.refresh();
            tp.entry.set_text("");
            tp.entry.grab_focus();
        } else {
            search.set_text("");
            search.grab_focus();
        }
        if !o2.get() {
            measure_next_paint(&hw2, "open");
        }
        o2.set(true);
    });
    let op1 = open_page.clone();
    let toggle: Rc<dyn Fn()> = Rc::new(move || op1("home"));
    let op2 = open_page.clone();
    let on_tor: Rc<dyn Fn()> = Rc::new(move || op2("tor"));

    let (top_w, _menu, srv_seg) = build_topbar(&mut labels, toggle);
    // SERVER field: info panel below the bar on hover; a click opens the app, a
    // second click closes it again.
    let info = srvinfo::InfoPanel::new(app);
    info.attach(&srv_seg, &top_w);
    let srv_click = gtk::GestureClick::new();
    let info2 = info.clone();
    srv_click.connect_released(move |_, _, _, _| {
        info2.close_now();
        toggle_server_app();
    });
    srv_seg.add_controller(srv_click);
    if std::env::var_os("SX_PREVIEW_SRVINFO").is_some() {
        info.preview_open();
    }
    let side_w = build_sidebar(&mut labels, on_tor);
    top_win.set_child(Some(&top_w));
    side_win.set_child(Some(&side_w));
    #[cfg(feature = "layer-shell")]
    {
        layer::bar(&top_win, true);
        layer::bar(&side_win, false);
    }
    #[cfg(not(feature = "layer-shell"))]
    {
        top_win.set_default_size(1280, TOPBAR_H);
        side_win.set_default_size(SIDEBAR_W, 720);
    }
    top_win.present();
    side_win.present();

    // Map Home once, parked as the desktop, so every opening is instant.
    hide();
    #[cfg(feature = "layer-shell")]
    home_win.present();
    start_updates(labels);
}

fn preview(app: &gtk::Application, wallpaper: Option<String>, show_home: bool) {
    let mut labels = Labels::default();
    let (top_w, _, _) = build_topbar(&mut labels, Rc::new(|| {}));
    let side_w = build_sidebar(&mut labels, Rc::new(|| {}));
    // Screenshots during development: SX_PREVIEW_TOR=1 shows the Tor page and the
    // Tor Browser buttons.
    let show_tor = std::env::var_os("SX_PREVIEW_TOR").is_some();
    if show_tor {
        if let Some(c) = &labels.tor_ctx {
            c.set_visible(true);
        }
    }
    let content: gtk::Widget = if show_tor {
        let page = torpage::build(Rc::new(|| {}));
        page.refresh();
        let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
        b.add_css_class("sx-content");
        b.append(&page.widget);
        page.widget.set_vexpand(true);
        b.upcast()
    } else if show_home {
        let (h, _) = build_home(&mut labels, Rc::new(|| {}));
        let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
        b.add_css_class("sx-content");
        b.append(&h);
        h.set_vexpand(true);
        b.upcast()
    } else if let Some(p) = wallpaper {
        let pic = gtk::Picture::for_filename(p);
        pic.set_content_fit(gtk::ContentFit::Cover);
        pic.upcast()
    } else {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
        b.add_css_class("sx-content");
        b.upcast()
    };
    content.set_hexpand(true);
    content.set_vexpand(true);

    let grid = gtk::Grid::new();
    grid.attach(&top_w, 0, 0, 2, 1);
    grid.attach(&side_w, 0, 1, 1, 1);
    grid.attach(&content, 1, 1, 1, 1);

    let win = gtk::ApplicationWindow::new(app);
    win.add_css_class("sx-preview");
    win.set_decorated(false);
    win.set_default_size(1280, 720);
    win.set_child(Some(&grid));
    win.present();
    if !show_tor {
        start_updates(labels);
    }
}

fn main() -> glib::ExitCode {
    // Software rendering with cairo is the fastest path inside a VM without a GPU;
    // the default OpenGL renderer would fall back to slow CPU emulation.
    if std::env::var_os("GSK_RENDERER").is_none() {
        std::env::set_var("GSK_RENDERER", "cairo");
    }
    let args: Vec<String> = std::env::args().collect();
    let preview_mode = args.iter().any(|a| a == "--preview");
    let show_home = args.iter().any(|a| a == "--home");
    let wallpaper = args
        .iter()
        .position(|a| a == "--preview")
        .and_then(|i| args.get(i + 1).cloned())
        .filter(|a| !a.starts_with("--"));

    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(|_| setup_display());
    app.connect_activate(move |app| {
        if preview_mode {
            preview(app, wallpaper.clone(), show_home);
        } else {
            shell(app);
        }
    });
    app.run_with_args(&args[..1])
}
