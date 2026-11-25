//! Tor page: a fullscreen page in the Home surface, opened from the Tor icon in the
//! sidebar instead of starting the browser directly. It offers an address field (address
//! or search), a new browser window, your own onion services and the bookmarks saved in
//! Tor Browser. Everything opens in the running Tor Browser (it accepts remote commands).
//!
//! Also holds the actions behind the context buttons in the top bar (new circuit for
//! the current site, new identity, new window), which reach Tor Browser
//! through the compositor: wlrctl brings its window to the front, wtype sends the shortcut.

use gtk::prelude::*;
use gtk::{gio, glib};
use std::path::PathBuf;
use std::rc::Rc;

/// app_id of Tor Browser windows (Wayland).
pub const TB_APP_ID: &str = "Tor Browser";
const TORBROWSER: &str = "/usr/bin/torbrowser";
/// DuckDuckGo's onion service, the search engine Tor Browser uses by default.
const SEARCH: &str = "https://duckduckgogg42xjoc72x3sjasowoarfbgcmvfimaftt6twagswzczad.onion/?q=";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

/// Runs a command in the background and reaps it, so nothing is left behind.
fn spawn(cmd: &str, args: &[&str]) {
    if let Ok(mut c) = std::process::Command::new(cmd).args(args).spawn() {
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
}

/// Tor Browser's own start script; with --allow-remote --new-tab it hands an address to
/// the running browser as a new tab and exits.
const START_TB: &str = ".tb/tor-browser/Browser/start-tor-browser";

fn wlrctl_ok(args: &[&str]) -> bool {
    std::process::Command::new("wlrctl").args(args).status().map(|s| s.success()).unwrap_or(false)
}

fn tb_running() -> bool {
    wlrctl_ok(&["toplevel", "find", &format!("app_id:{TB_APP_ID}")])
}

/// Opens a URL in Tor Browser. The Tor page hands it to the running browser directly as
/// a new tab: Whonix's "Confirm Open" question is meant for other programs passing links
/// unnoticed, while here the user clicked the address. If the browser is not running, it
/// is started through Whonix's starter and the address is loaded into its first tab
/// (instead of the start page), so only the chosen page is open. Only if the direct
/// hand-over fails, the address goes through the starter (which asks).
pub fn open_url(url: &str) {
    let url = url.to_string();
    std::thread::spawn(move || {
        if !tb_running() {
            if let Ok(mut c) = std::process::Command::new(TORBROWSER).spawn() {
                std::thread::spawn(move || {
                    let _ = c.wait();
                });
            }
            let _ = std::process::Command::new("timeout")
                .args(["120", "wlrctl", "toplevel", "waitfor", &format!("app_id:{TB_APP_ID}")])
                .status();
            std::thread::sleep(std::time::Duration::from_millis(2500));
            if load_in_current_tab(&url) {
                return;
            }
        }
        let direct = std::process::Command::new(home().join(START_TB))
            .args(["--allow-remote", "--new-tab", &url])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !direct {
            let _ = std::process::Command::new(TORBROWSER).arg(&url).status();
        }
    });
}

/// Loads a URL into the tab that is open in front: address bar (Ctrl+L), the address,
/// Enter. Used right after the browser started, so its start page is replaced.
fn load_in_current_tab(url: &str) -> bool {
    if !wlrctl_ok(&["toplevel", "focus", &format!("app_id:{TB_APP_ID}")]) {
        return false;
    }
    std::thread::sleep(std::time::Duration::from_millis(250));
    let wtype = |args: &[&str]| std::process::Command::new("wtype").args(args).status().map(|s| s.success()).unwrap_or(false);
    wtype(&["-M", "ctrl", "-k", "l", "-m", "ctrl"])
        && {
            std::thread::sleep(std::time::Duration::from_millis(150));
            wtype(&[url])
        }
        && wtype(&["-k", "Return"])
}

/// Maximizes Tor Browser windows that are not maximized. Called shortly after a new
/// window appears: Tor Browser sets its own window size right after opening, so the
/// compositor's open-maximized rule alone would be undone.
pub fn maximize_windows() {
    std::thread::spawn(|| {
        for wait in [1500u64, 3000] {
            std::thread::sleep(std::time::Duration::from_millis(wait));
            let _ = wlrctl_ok(&["toplevel", "maximize", &format!("app_id:{TB_APP_ID}"), "state:unmaximized"]);
        }
    });
}

/// Turns what was typed into a URL: an address stays an address (onion addresses over
/// http, everything else over https), anything else becomes a search.
pub fn url_for(input: &str) -> Option<String> {
    let t = input.trim();
    if t.is_empty() {
        return None;
    }
    if t.contains("://") {
        return Some(t.to_string());
    }
    let host = t.split('/').next().unwrap_or("");
    if !t.contains(' ') && host.contains('.') && !host.ends_with('.') {
        let scheme = if host.ends_with(".onion") { "http" } else { "https" };
        return Some(format!("{scheme}://{t}"));
    }
    let q: String = glib::Uri::escape_string(t, None, false).into();
    Some(format!("{SEARCH}{q}"))
}

/// Brings Tor Browser to the front and sends a shortcut (Ctrl, optionally Shift, key).
fn tb_shortcut(key: &'static str, shift: bool) {
    std::thread::spawn(move || {
        let ok = std::process::Command::new("wlrctl")
            .args(["toplevel", "focus", &format!("app_id:{TB_APP_ID}")])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
        let mut args = vec!["-M", "ctrl"];
        if shift {
            args.extend(["-M", "shift"]);
        }
        args.extend(["-k", key]);
        if shift {
            args.extend(["-m", "shift"]);
        }
        args.extend(["-m", "ctrl"]);
        let _ = std::process::Command::new("wtype").args(&args).status();
    });
}

/// "New Tor circuit for this site" (Ctrl+Shift+L).
pub fn new_circuit() {
    tb_shortcut("l", true);
}

/// "New identity": closes all tabs, clears everything, new circuits (Ctrl+Shift+U).
pub fn new_identity() {
    tb_shortcut("u", true);
}

/// A new browser window (Ctrl+N) if Tor Browser runs, otherwise it starts.
pub fn new_window() {
    if tb_running() {
        tb_shortcut("n", false);
    } else {
        spawn(TORBROWSER, &[]);
    }
}

struct Link {
    title: String,
    url: String,
}

/// Bookmarks from Tor Browser (read through a small helper on a copy of the database).
fn load_bookmarks() -> Vec<Link> {
    let out = std::process::Command::new("sentinel-tor-bookmarks").output();
    let Ok(o) = out else { return Vec::new() };
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| {
            let (t, u) = l.split_once('\t')?;
            Some(Link { title: t.to_string(), url: u.to_string() })
        })
        .collect()
}

/// Own services: ~/.config/sentinel-shell/services.conf, one "Name|URL" per line.
fn load_services() -> Vec<Link> {
    std::fs::read_to_string(home().join(".config/sentinel-shell/services.conf"))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (t, u) = l.split_once('|')?;
            Some(Link { title: t.trim().to_string(), url: u.trim().to_string() })
        })
        .collect()
}

/// Host part for the second line of a tile; long onion addresses are shortened.
fn short_host(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split('/').next().unwrap_or(rest);
    match host.strip_suffix(".onion") {
        Some(h) if h.len() > 16 => format!("{}…{}.onion", &h[..8], &h[h.len() - 4..]),
        _ => host.to_string(),
    }
}

fn tile(link: &Link, hide: &Rc<dyn Fn()>) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("sx-bm");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let letter = link.title.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or("·".into());
    let av = gtk::Label::new(Some(&letter));
    av.add_css_class("sx-bm-avatar");
    av.set_valign(gtk::Align::Center);
    let txt = gtk::Box::new(gtk::Orientation::Vertical, 2);
    txt.set_valign(gtk::Align::Center);
    let t = gtk::Label::new(Some(if link.title.is_empty() { &link.url } else { &link.title }));
    t.add_css_class("sx-bm-title");
    t.set_xalign(0.0);
    t.set_ellipsize(gtk::pango::EllipsizeMode::End);
    t.set_max_width_chars(24);
    let h = gtk::Label::new(Some(&short_host(&link.url)));
    h.add_css_class("sx-bm-host");
    h.set_xalign(0.0);
    h.set_ellipsize(gtk::pango::EllipsizeMode::End);
    h.set_max_width_chars(28);
    txt.append(&t);
    txt.append(&h);
    row.append(&av);
    row.append(&txt);
    b.set_child(Some(&row));
    b.set_tooltip_text(Some(&link.url));
    let url = link.url.clone();
    let hide = hide.clone();
    b.connect_clicked(move |_| {
        open_url(&url);
        hide();
    });
    b
}

fn flow() -> gtk::FlowBox {
    let f = gtk::FlowBox::new();
    f.add_css_class("sx-apps");
    f.set_selection_mode(gtk::SelectionMode::None);
    f.set_homogeneous(true);
    f.set_max_children_per_line(5);
    f.set_min_children_per_line(2);
    f.set_row_spacing(8);
    f.set_column_spacing(8);
    f.set_valign(gtk::Align::Start);
    f
}

pub struct TorPage {
    pub widget: gtk::Widget,
    pub entry: gtk::Entry,
    refresh: Rc<dyn Fn()>,
}

impl TorPage {
    /// Reloads bookmarks and services (called each time the page opens).
    pub fn refresh(&self) {
        (self.refresh)();
    }
}

pub fn build(hide: Rc<dyn Fn()>) -> TorPage {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("sx-home-box");

    let kicker = gtk::Label::new(Some("TOR BROWSER"));
    kicker.add_css_class("sx-tor-kicker");
    kicker.set_halign(gtk::Align::Start);
    let title = gtk::Label::new(Some("Where do you want to go?"));
    title.add_css_class("sx-tor-title");
    title.set_halign(gtk::Align::Start);
    root.append(&kicker);
    root.append(&title);

    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    bar.set_margin_top(18);
    let entry = gtk::Entry::new();
    entry.add_css_class("sx-search");
    entry.set_placeholder_text(Some("Address, onion address or search"));
    entry.set_primary_icon_name(Some("web-browser-symbolic"));
    entry.set_width_request(640);
    {
        let hide = hide.clone();
        entry.connect_activate(move |e| {
            if let Some(url) = url_for(&e.text()) {
                open_url(&url);
                e.set_text("");
                hide();
            }
        });
    }
    let go = gtk::Button::with_label("Open");
    go.add_css_class("sx-action");
    {
        let e = entry.clone();
        go.connect_clicked(move |_| e.emit_activate());
    }
    let neww = gtk::Button::with_label("New window");
    neww.add_css_class("sx-action");
    {
        let hide = hide.clone();
        neww.connect_clicked(move |_| {
            // A running browser gets a new window; otherwise the browser starts.
            hide();
            new_window();
        });
    }
    bar.append(&entry);
    bar.append(&go);
    bar.append(&neww);
    root.append(&bar);

    let sec1 = gtk::Label::new(Some("YOUR SERVICES"));
    sec1.add_css_class("sx-section");
    sec1.set_halign(gtk::Align::Start);
    let services = flow();
    let sec2 = gtk::Label::new(Some("FAVORITES"));
    sec2.add_css_class("sx-section");
    sec2.set_halign(gtk::Align::Start);
    let favs = flow();
    let empty = gtk::Label::new(Some("No bookmarks yet. Pages you bookmark in Tor Browser appear here."));
    empty.add_css_class("sx-bm-host");
    empty.set_halign(gtk::Align::Start);
    empty.set_visible(false);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&sec1);
    content.append(&services);
    content.append(&sec2);
    content.append(&favs);
    content.append(&empty);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&content));
    root.append(&scroll);

    let refresh: Rc<dyn Fn()> = {
        let (services, favs, sec1, empty, hide) = (services.clone(), favs.clone(), sec1.clone(), empty.clone(), hide.clone());
        Rc::new(move || {
            while let Some(c) = services.first_child() {
                services.remove(&c);
            }
            let own = load_services();
            sec1.set_visible(!own.is_empty());
            services.set_visible(!own.is_empty());
            for l in &own {
                services.append(&tile(l, &hide));
            }
            let (favs, empty, hide) = (favs.clone(), empty.clone(), hide.clone());
            glib::spawn_future_local(async move {
                let links = gio::spawn_blocking(load_bookmarks).await.unwrap_or_default();
                while let Some(c) = favs.first_child() {
                    favs.remove(&c);
                }
                empty.set_visible(links.is_empty());
                for l in &links {
                    favs.append(&tile(l, &hide));
                }
            });
        })
    };
    TorPage { widget: root.upcast(), entry, refresh }
}
