//! SimpleGo Server: fullscreen monitoring of the onion-only server.
//!
//! The laptop asks the server once a minute through Tor with a read-only key
//! (sentinel-server-poll) and keeps every report in a long-term SQLite history.
//! This program only reads that data through `sentinel-server-data`, so it needs no
//! network access and no extra libraries.
//!
//! Pages: Overview (verdict, reasons, key figures), History (charts over 24 h,
//! 7 d, 30 d or everything), Events (chronicle of everything that changed).
//!
//! `sentinel-server` opens the window. `SX_SRV_PAGE=history|events` and `SX_SRV_HOURS=N`
//! open another page or range first (screenshots during development).

use gtk::prelude::*;
use gtk::{cairo, gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

const APP_ID: &str = "org.sentinelx.Server";
const ACCENT: (f64, f64, f64) = (0xd4 as f64 / 255.0, 0xaf as f64 / 255.0, 0x37 as f64 / 255.0);
const FAIL: (f64, f64, f64) = (0xf8 as f64 / 255.0, 0x71 as f64 / 255.0, 0x71 as f64 / 255.0);
const OKC: (f64, f64, f64) = (0x4a as f64 / 255.0, 0xde as f64 / 255.0, 0x80 as f64 / 255.0);
const WAITC: (f64, f64, f64) = (0xfb as f64 / 255.0, 0x92 as f64 / 255.0, 0x3c as f64 / 255.0);
const SUBC: (f64, f64, f64) = (0x9a as f64 / 255.0, 0x9a as f64 / 255.0, 0x9a as f64 / 255.0);
const GRID: (f64, f64, f64) = (0x22 as f64 / 255.0, 0x22 as f64 / 255.0, 0x22 as f64 / 255.0);

// ---------------------------------------------------------------- data access

fn data_cmd() -> String {
    std::env::var("SX_SRV_DATA").unwrap_or_else(|_| "sentinel-server-data".into())
}

/// Runs `sentinel-server-data ARGS` and splits its output into tab-separated rows.
fn data(args: &[&str]) -> Vec<Vec<String>> {
    match std::process::Command::new(data_cmd()).args(args).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.split('\t').map(str::to_string).collect())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[derive(Default, Clone)]
struct Now {
    kv: HashMap<String, String>,
    services: Vec<(String, String)>,
    issues: Vec<(String, String)>,
}

impl Now {
    fn get(&self, k: &str) -> &str {
        self.kv.get(k).map(String::as_str).unwrap_or("")
    }
    fn num(&self, k: &str) -> Option<f64> {
        self.get(k).parse().ok()
    }
}

fn load_now() -> Now {
    let mut n = Now::default();
    for r in data(&["now"]) {
        match r.first().map(String::as_str) {
            Some("service") if r.len() >= 3 => n.services.push((r[1].clone(), r[2].clone())),
            Some("issue") if r.len() >= 3 => n.issues.push((r[1].clone(), r[2].clone())),
            Some(k) if r.len() >= 2 => {
                n.kv.insert(k.to_string(), r[1].clone());
            }
            _ => {}
        }
    }
    n
}

fn load_kv(args: &[&str]) -> HashMap<String, String> {
    data(args).into_iter().filter(|r| r.len() >= 2).map(|r| (r[0].clone(), r[1].clone())).collect()
}

type Series = Vec<(i64, Option<f64>)>;

fn load_series(metric: &str, hours: u32) -> Series {
    data(&["series", metric, &hours.to_string()])
        .into_iter()
        .filter_map(|r| Some((r.first()?.parse().ok()?, r.get(1).and_then(|v| v.parse().ok()))))
        .collect()
}

struct Event {
    ts: i64,
    level: String,
    text: String,
}

fn load_events(limit: u32) -> Vec<Event> {
    data(&["events", &limit.to_string()])
        .into_iter()
        .filter(|r| r.len() >= 4)
        .map(|r| Event { ts: r[0].parse().unwrap_or(0), level: r[1].clone(), text: r[3].clone() })
        .collect()
}

// ---------------------------------------------------------------- formatting

fn fmt_time(ts: i64, with_date: bool) -> String {
    glib::DateTime::from_unix_local(ts)
        .ok()
        .and_then(|d| d.format(if with_date { "%d.%m. %H:%M" } else { "%H:%M" }).ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn fmt_duration(secs: f64) -> String {
    let s = secs as i64;
    let (d, h, m) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else {
        format!("{m} min")
    }
}

fn fmt_age(secs: Option<f64>) -> String {
    match secs {
        Some(s) if s < 90.0 => format!("{} s ago", s as i64),
        Some(s) => format!("{} ago", fmt_duration(s)),
        None => "never".into(),
    }
}

/// Level of the whole server as the user should read it.
fn verdict(level: &str) -> (&'static str, &'static str, &'static str) {
    match level {
        "ok" => ("ALL GOOD", "ok", "Every check passed."),
        "warn" => ("ATTENTION", "wait", "The server works, but something needs a look."),
        "fail" => ("PROBLEM", "fail", "At least one protection or service is not working."),
        "unreachable" => ("NO CONTACT", "lost", "No report through Tor for more than five minutes."),
        _ => ("NO DATA YET", "lost", "Monitoring has not delivered a report yet."),
    }
}

fn state_class(level: &str) -> &'static str {
    match level {
        "ok" => "ok",
        "warn" => "wait",
        "fail" | "unreachable" => "fail",
        _ => "lost",
    }
}

fn set_class(w: &impl IsA<gtk::Widget>, class: &str) {
    for c in ["ok", "wait", "fail", "lost"] {
        w.remove_css_class(c);
    }
    if !class.is_empty() {
        w.add_css_class(class);
    }
}

// ---------------------------------------------------------------- overview

struct Tile {
    value: gtk::Label,
    sub: gtk::Label,
}

fn tile(grid: &gtk::Grid, idx: i32, key: &str) -> Tile {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    b.add_css_class("srv-tile");
    b.set_hexpand(true);
    let k = gtk::Label::new(Some(key));
    k.add_css_class("srv-tile-key");
    k.set_xalign(0.0);
    let v = gtk::Label::new(Some("..."));
    v.add_css_class("srv-tile-val");
    v.set_xalign(0.0);
    let s = gtk::Label::new(None);
    s.add_css_class("srv-tile-sub");
    s.set_xalign(0.0);
    s.set_ellipsize(gtk::pango::EllipsizeMode::End);
    b.append(&k);
    b.append(&v);
    b.append(&s);
    grid.attach(&b, idx % 4, idx / 4, 1, 1);
    Tile { value: v, sub: s }
}

struct Overview {
    page: gtk::ScrolledWindow,
    dot: gtk::DrawingArea,
    dot_class: Rc<RefCell<String>>,
    verdict: gtk::Label,
    verdict_sub: gtk::Label,
    issues: gtk::Box,
    tiles: HashMap<&'static str, Tile>,
    summary: gtk::Label,
}

const TILE_KEYS: &[&str] = &[
    "TOR", "WEBSITE", "SSH ONION", "VANGUARDS",
    "FIREWALL", "BLOCKED OUT 24 H", "FAILED LOGINS 24 H", "UPDATES",
    "UPTIME", "MEMORY", "DISK", "TIME SYNC",
];

fn build_overview() -> Overview {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 18);

    let vcard = gtk::Box::new(gtk::Orientation::Horizontal, 22);
    vcard.add_css_class("srv-verdict");
    let dot_class = Rc::new(RefCell::new(String::from("lost")));
    let dot = gtk::DrawingArea::new();
    dot.set_content_width(56);
    dot.set_content_height(56);
    dot.set_valign(gtk::Align::Center);
    {
        let dc = dot_class.clone();
        dot.set_draw_func(move |_, cr, w, h| {
            let c = match dc.borrow().as_str() {
                "ok" => OKC,
                "wait" => WAITC,
                "fail" => FAIL,
                _ => SUBC,
            };
            let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
            cr.set_source_rgba(c.0, c.1, c.2, 0.18);
            cr.arc(cx, cy, 26.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
            cr.set_source_rgb(c.0, c.1, c.2);
            cr.arc(cx, cy, 13.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        });
    }
    vcard.append(&dot);
    let vt = gtk::Box::new(gtk::Orientation::Vertical, 4);
    vt.set_valign(gtk::Align::Center);
    let verdict_l = gtk::Label::new(Some("..."));
    verdict_l.add_css_class("srv-verdict-title");
    verdict_l.set_xalign(0.0);
    let verdict_sub = gtk::Label::new(None);
    verdict_sub.add_css_class("cd-sub");
    verdict_sub.set_xalign(0.0);
    verdict_sub.set_wrap(true);
    vt.append(&verdict_l);
    vt.append(&verdict_sub);
    vcard.append(&vt);
    page.append(&vcard);

    let issues = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.append(&issues);

    let grid = gtk::Grid::new();
    grid.set_row_spacing(12);
    grid.set_column_spacing(12);
    grid.set_column_homogeneous(true);
    let mut tiles = HashMap::new();
    for (i, k) in TILE_KEYS.iter().enumerate() {
        tiles.insert(*k, tile(&grid, i as i32, k));
    }
    page.append(&grid);

    let summary = gtk::Label::new(None);
    summary.add_css_class("cd-sub");
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    page.append(&summary);

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&page));
    scroller.set_vexpand(true);
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.add_css_class("cd-scroll");
    Overview { page: scroller, dot, dot_class, verdict: verdict_l, verdict_sub, issues, tiles, summary }
}

fn fill_tile(o: &Overview, key: &str, value: &str, class: &str, sub: &str) {
    if let Some(t) = o.tiles.get(key) {
        t.value.set_text(value);
        set_class(&t.value, class);
        t.sub.set_text(sub);
    }
}

fn fill_overview(o: &Overview, n: &Now, s24: &HashMap<String, String>, s7: &HashMap<String, String>) {
    let level = n.get("level");
    let (title, class, sub) = verdict(level);
    o.verdict.set_text(title);
    set_class(&o.verdict, class);
    *o.dot_class.borrow_mut() = class.to_string();
    o.dot.queue_draw();
    let age = n.num("age_s");
    o.verdict_sub.set_text(&format!("{sub}  Last report {}.", fmt_age(age)));

    while let Some(c) = o.issues.first_child() {
        o.issues.remove(&c);
    }
    for (lvl, text) in &n.issues {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        row.add_css_class("srv-issue");
        let chip = gtk::Label::new(Some(if lvl == "fail" { "PROBLEM" } else { "CHECK" }));
        chip.add_css_class("cd-status");
        set_class(&chip, state_class(lvl));
        let t = gtk::Label::new(Some(text));
        t.add_css_class("srv-issue-text");
        t.set_xalign(0.0);
        t.set_wrap(true);
        t.set_hexpand(true);
        row.append(&chip);
        row.append(&t);
        o.issues.append(&row);
    }
    o.issues.set_visible(!n.issues.is_empty());

    let int = |k: &str| n.num(k).map(|v| v as i64);
    let boot = int("bootstrap");
    fill_tile(o, "TOR", &boot.map(|b| format!("{b} %")).unwrap_or("-".into()),
        if boot == Some(100) { "ok" } else { "fail" },
        &format!("Tor {}", n.get("tor_version")));
    let http = n.get("web_http");
    let iw = int("intro_web").unwrap_or(0);
    fill_tile(o, "WEBSITE", if http == "200" { "ONLINE" } else if http.is_empty() { "-" } else { http },
        if http == "200" && iw > 0 { "ok" } else { "fail" }, &format!("{iw} introduction circuits"));
    let is = int("intro_ssh").unwrap_or(0);
    fill_tile(o, "SSH ONION", if is > 0 { "ONLINE" } else { "OFFLINE" }, if is > 0 { "ok" } else { "fail" },
        &format!("{is} introduction circuits, client key required"));
    let vg = n.get("vanguards");
    fill_tile(o, "VANGUARDS", &vg.to_uppercase(),
        match vg { "active" => "ok", "not installed" => "", _ => "fail" },
        if vg == "active" { "full guard-discovery protection" } else { "built-in vanguards-lite only" });
    let fw = n.get("firewall");
    let base = n.get("firewall_baseline");
    fill_tile(o, "FIREWALL", &fw.to_uppercase(), if fw == "active" && base == "unchanged" { "ok" } else { "fail" },
        &format!("only Tor may connect out, rules {base}"));
    let drops = int("out_drops_24h").unwrap_or(0);
    fill_tile(o, "BLOCKED OUT 24 H", &drops.to_string(), if drops == 0 { "ok" } else { "wait" },
        "attempts to bypass Tor, dropped");
    let failed = int("ssh_failed_24h").unwrap_or(0);
    fill_tile(o, "FAILED LOGINS 24 H", &failed.to_string(), if failed == 0 { "ok" } else { "wait" },
        "should stay 0 behind the onion key");
    let sec = int("security_pending").unwrap_or(0);
    let pend = int("updates_pending").unwrap_or(0);
    let reboot = n.get("reboot_required") == "True";
    fill_tile(o, "UPDATES", &if sec > 0 { format!("{sec} SECURITY") } else if pend > 0 { format!("{pend} PENDING") } else { "CURRENT".into() },
        if sec > 0 || reboot { "wait" } else { "ok" },
        if reboot { "reboot required" } else { "installed automatically through Tor" });
    fill_tile(o, "UPTIME", &n.num("uptime_s").map(fmt_duration).unwrap_or("-".into()), "",
        &format!("load {}", n.get("load1")));
    let avail = int("mem_avail_mb").unwrap_or(0);
    fill_tile(o, "MEMORY", &format!("{avail} MB"), if avail >= 100 { "ok" } else { "wait" },
        &format!("free of {} MB", n.get("mem_total_mb")));
    let disk = n.num("disk_used_pct").unwrap_or(0.0);
    fill_tile(o, "DISK", &format!("{disk:.0} %"), if disk <= 85.0 { "ok" } else { "wait" }, "used");
    let ts = n.get("time_synced") == "True";
    fill_tile(o, "TIME SYNC", if ts { "SYNCED" } else { "OFF" }, if ts { "ok" } else { "wait" }, "fixed PTB time servers");

    let pct = |m: &HashMap<String, String>| m.get("availability_pct").filter(|v| !v.is_empty()).map(|v| format!("{v} %")).unwrap_or("-".into());
    let secs = |k: &str| s24.get(k).and_then(|v| v.parse::<f64>().ok()).map(|v| format!("{:.1} s", v / 1000.0)).unwrap_or("-".into());
    let lat = secs("latency_median_ms");
    let fresh = secs("fresh_median_ms");
    let since = s7.get("recording_since").and_then(|v| v.parse::<i64>().ok()).map(|t| fmt_time(t, true)).unwrap_or("-".into());
    o.summary.set_text(&format!(
        "Reachable through Tor: {} in 24 h, {} in 7 days.  Typical response: {lat} over the kept-open link, {fresh} for a new connection.  Recording since {since}.",
        pct(s24), pct(s7)));
}

// ---------------------------------------------------------------- charts

struct ChartSpec {
    title: &'static str,
    metric: &'static str,
    unit: &'static str,
    /// Fixed y range (min, max); None = from the data.
    fixed: Option<(f64, f64)>,
    scale: f64,
    decimals: usize,
}

const CHARTS: &[ChartSpec] = &[
    ChartSpec { title: "REACHABLE THROUGH TOR", metric: "availability", unit: "%", fixed: Some((0.0, 100.0)), scale: 1.0, decimals: 1 },
    ChartSpec { title: "RESPONSE (KEPT-OPEN LINK)", metric: "latency_ms", unit: "s", fixed: None, scale: 0.001, decimals: 1 },
    ChartSpec { title: "NEW CONNECTION THROUGH TOR", metric: "fresh_ms", unit: "s", fixed: None, scale: 0.001, decimals: 1 },
    ChartSpec { title: "INTRODUCTION CIRCUITS (WEBSITE)", metric: "intro_web", unit: "", fixed: None, scale: 1.0, decimals: 0 },
    ChartSpec { title: "INTRODUCTION CIRCUITS (SSH)", metric: "intro_ssh", unit: "", fixed: None, scale: 1.0, decimals: 0 },
    ChartSpec { title: "BLOCKED OUTBOUND (24 H WINDOW)", metric: "out_drops_24h", unit: "", fixed: None, scale: 1.0, decimals: 0 },
    ChartSpec { title: "FAILED LOGINS (24 H WINDOW)", metric: "ssh_failed_24h", unit: "", fixed: None, scale: 1.0, decimals: 0 },
    ChartSpec { title: "GUARD WARNINGS (24 H WINDOW)", metric: "guard_warn_24h", unit: "", fixed: None, scale: 1.0, decimals: 0 },
    ChartSpec { title: "VANGUARDS WARNINGS (24 H WINDOW)", metric: "vg_warn_24h", unit: "", fixed: None, scale: 1.0, decimals: 0 },
    ChartSpec { title: "FREE MEMORY", metric: "mem_avail_mb", unit: "MB", fixed: None, scale: 1.0, decimals: 0 },
    ChartSpec { title: "DISK USED", metric: "disk_used_pct", unit: "%", fixed: Some((0.0, 100.0)), scale: 1.0, decimals: 0 },
    ChartSpec { title: "LOAD", metric: "load1", unit: "", fixed: None, scale: 1.0, decimals: 2 },
    ChartSpec { title: "UPDATES PENDING", metric: "updates_pending", unit: "", fixed: None, scale: 1.0, decimals: 0 },
];

struct Chart {
    spec: &'static ChartSpec,
    area: gtk::DrawingArea,
    value: gtk::Label,
    data: Rc<RefCell<Series>>,
}

fn fmt_val(v: f64, spec: &ChartSpec) -> String {
    let s = format!("{:.*}", spec.decimals, v * spec.scale);
    if spec.unit.is_empty() { s } else { format!("{s} {}", spec.unit) }
}

fn draw_chart(cr: &cairo::Context, w: f64, h: f64, spec: &ChartSpec, pts: &Series, range: (i64, i64)) {
    let (l, r, t, b) = (48.0, 10.0, 8.0, 22.0);
    let (pw, ph) = (w - l - r, h - t - b);
    let vals: Vec<f64> = pts.iter().filter_map(|p| p.1).map(|v| v * spec.scale).collect();
    let (mut lo, mut hi) = match spec.fixed {
        Some((a, b)) => (a, b),
        None => (
            vals.iter().cloned().fold(f64::INFINITY, f64::min).min(0.0),
            vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        ),
    };
    if !hi.is_finite() || hi <= lo {
        hi = lo + 1.0;
    }
    if !lo.is_finite() {
        lo = 0.0;
    }
    cr.select_font_face("DejaVu Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(10.0);
    // Grid and y labels.
    for i in 0..=2 {
        let y = t + ph * i as f64 / 2.0;
        cr.set_source_rgb(GRID.0, GRID.1, GRID.2);
        cr.set_line_width(1.0);
        cr.move_to(l, y.round() + 0.5);
        cr.line_to(w - r, y.round() + 0.5);
        let _ = cr.stroke();
        let v = hi - (hi - lo) * i as f64 / 2.0;
        let txt = format!("{:.*}", if spec.decimals == 0 && (hi - lo) >= 2.0 { 0 } else { spec.decimals.max(1) }, v);
        cr.set_source_rgb(SUBC.0, SUBC.1, SUBC.2);
        if let Ok(ext) = cr.text_extents(&txt) {
            cr.move_to(l - 8.0 - ext.width(), y + 3.5);
            let _ = cr.show_text(&txt);
        }
    }
    // x labels: start and end of the range.
    cr.set_source_rgb(SUBC.0, SUBC.1, SUBC.2);
    let long = range.1 - range.0 >= 20 * 3600;
    let a = fmt_time(range.0, long);
    let z = fmt_time(range.1, long);
    cr.move_to(l, h - 6.0);
    let _ = cr.show_text(&a);
    if let Ok(ext) = cr.text_extents(&z) {
        cr.move_to(w - r - ext.width(), h - 6.0);
        let _ = cr.show_text(&z);
    }
    if pts.is_empty() {
        cr.move_to(l + pw / 2.0 - 30.0, t + ph / 2.0);
        let _ = cr.show_text("no data yet");
        return;
    }
    let span = (range.1 - range.0).max(1) as f64;
    let x = |ts: i64| l + pw * ((ts - range.0) as f64 / span).clamp(0.0, 1.0);
    let y = |v: f64| t + ph * (1.0 - ((v - lo) / (hi - lo)).clamp(0.0, 1.0));
    // Line, broken where there is no value.
    cr.set_source_rgb(ACCENT.0, ACCENT.1, ACCENT.2);
    cr.set_line_width(1.8);
    cr.set_line_join(cairo::LineJoin::Round);
    let mut started = false;
    for (ts, v) in pts {
        match v {
            Some(v) => {
                let (px, py) = (x(*ts), y(v * spec.scale));
                if started { cr.line_to(px, py) } else { cr.move_to(px, py) }
                started = true;
            }
            None => started = false,
        }
    }
    let _ = cr.stroke();
    // Single points stay visible as dots.
    if pts.len() == 1 {
        if let Some(v) = pts[0].1 {
            cr.arc(x(pts[0].0), y(v * spec.scale), 3.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
    }
    // Availability: every gap below 100 % marked in red along the bottom.
    if spec.metric == "availability" {
        cr.set_source_rgb(FAIL.0, FAIL.1, FAIL.2);
        for (ts, v) in pts {
            if v.map(|v| v < 99.95).unwrap_or(false) {
                cr.rectangle(x(*ts) - 1.5, t + ph - 4.0, 3.0, 4.0);
            }
        }
        let _ = cr.fill();
    }
}

struct History {
    page: gtk::ScrolledWindow,
    charts: Vec<Chart>,
    range: Rc<RefCell<(i64, i64)>>,
    hours: Rc<Cell<u32>>,
    buttons: Vec<(u32, gtk::Button)>,
}

const RANGES: &[(&str, u32)] = &[("24 h", 24), ("7 days", 168), ("30 days", 720), ("All", 87600)];

fn build_history() -> History {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 16);
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let mut buttons = Vec::new();
    for (label, h) in RANGES {
        let b = gtk::Button::with_label(label);
        b.add_css_class("cd-small");
        bar.append(&b);
        buttons.push((*h, b));
    }
    page.append(&bar);

    let grid = gtk::Grid::new();
    grid.set_row_spacing(12);
    grid.set_column_spacing(12);
    grid.set_column_homogeneous(true);
    let range = Rc::new(RefCell::new((0i64, 1i64)));
    let mut charts = Vec::new();
    for (i, spec) in CHARTS.iter().enumerate() {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        card.add_css_class("srv-tile");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let k = gtk::Label::new(Some(spec.title));
        k.add_css_class("srv-tile-key");
        k.set_xalign(0.0);
        k.set_hexpand(true);
        let v = gtk::Label::new(None);
        v.add_css_class("srv-chart-val");
        head.append(&k);
        head.append(&v);
        card.append(&head);
        let area = gtk::DrawingArea::new();
        area.set_content_height(150);
        area.set_hexpand(true);
        let d: Rc<RefCell<Series>> = Rc::new(RefCell::new(Vec::new()));
        {
            let d = d.clone();
            let range = range.clone();
            area.set_draw_func(move |_, cr, w, h| draw_chart(cr, w as f64, h as f64, spec, &d.borrow(), *range.borrow()));
        }
        card.append(&area);
        // Reachability spans the full width, the others sit in two columns below.
        if i == 0 {
            grid.attach(&card, 0, 0, 2, 1);
        } else {
            grid.attach(&card, ((i - 1) % 2) as i32, ((i - 1) / 2 + 1) as i32, 1, 1);
        }
        charts.push(Chart { spec, area, value: v, data: d });
    }
    page.append(&grid);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&page));
    scroller.set_vexpand(true);
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.add_css_class("cd-scroll");
    // SX_SRV_HOURS selects the first range (screenshots during development).
    let first = std::env::var("SX_SRV_HOURS").ok().and_then(|h| h.parse().ok()).unwrap_or(24);
    History { page: scroller, charts, range, hours: Rc::new(Cell::new(first)), buttons }
}

// ---------------------------------------------------------------- events

fn build_events() -> (gtk::ScrolledWindow, gtk::Box) {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&list));
    scroller.set_vexpand(true);
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.add_css_class("cd-scroll");
    (scroller, list)
}

fn fill_events(list: &gtk::Box, events: &[Event]) {
    while let Some(c) = list.first_child() {
        list.remove(&c);
    }
    if events.is_empty() {
        let l = gtk::Label::new(Some("No events recorded yet."));
        l.add_css_class("cd-sub");
        list.append(&l);
        return;
    }
    for e in events {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        row.add_css_class("srv-event");
        let t = gtk::Label::new(Some(&fmt_time(e.ts, true)));
        t.add_css_class("srv-event-time");
        let chip = gtk::Label::new(Some(match e.level.as_str() {
            "fail" => "PROBLEM",
            "warn" => "CHECK",
            _ => "OK",
        }));
        chip.add_css_class("cd-status");
        chip.set_width_chars(8);
        set_class(&chip, state_class(&e.level));
        let x = gtk::Label::new(Some(&e.text));
        x.add_css_class("srv-issue-text");
        x.set_xalign(0.0);
        x.set_wrap(true);
        x.set_hexpand(true);
        row.append(&t);
        row.append(&chip);
        row.append(&x);
        list.append(&row);
    }
}

// ---------------------------------------------------------------- window

struct Ui {
    overview: Overview,
    history: History,
    events: gtk::Box,
    status: gtk::Label,
    busy: Cell<bool>,
}

fn reload_overview(ui: &Rc<Ui>) {
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let r = gio::spawn_blocking(|| (load_now(), load_kv(&["summary", "24"]), load_kv(&["summary", "168"]))).await;
        if let Ok((n, s24, s7)) = r {
            fill_overview(&ui.overview, &n, &s24, &s7);
            ui.status.set_text(&format!(
                "Checked once a minute through Tor with a read-only key.  Last report {}.",
                fmt_age(n.num("age_s"))));
        }
    });
}

fn reload_history(ui: &Rc<Ui>) {
    let hours = ui.history.hours.get();
    for (h, b) in &ui.history.buttons {
        if *h == hours { b.add_css_class("active") } else { b.remove_css_class("active") }
    }
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let metrics: Vec<&'static str> = CHARTS.iter().map(|c| c.metric).collect();
        let r = gio::spawn_blocking(move || {
            let since = load_kv(&["summary", &hours.to_string()]).get("recording_since").and_then(|v| v.parse::<i64>().ok());
            (since, metrics.iter().map(|m| load_series(m, hours)).collect::<Vec<_>>())
        })
        .await;
        if let Ok((since, all)) = r {
            let now = glib::DateTime::now_utc().map(|d| d.to_unix()).unwrap_or(0);
            let mut start = now - hours as i64 * 3600;
            if hours >= 87600 {
                start = since.unwrap_or(now - 86400).min(now - 3600);
            }
            *ui.history.range.borrow_mut() = (start, now);
            for (c, s) in ui.history.charts.iter().zip(all) {
                let last = s.iter().rev().find_map(|p| p.1);
                c.value.set_text(&last.map(|v| fmt_val(v, c.spec)).unwrap_or_default());
                *c.data.borrow_mut() = s;
                c.area.queue_draw();
            }
        }
    });
}

fn reload_events(ui: &Rc<Ui>) {
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        if let Ok(ev) = gio::spawn_blocking(|| load_events(300)).await {
            fill_events(&ui.events, &ev);
        }
    });
}

fn reload_all(ui: &Rc<Ui>) {
    reload_overview(ui);
    reload_history(ui);
    reload_events(ui);
}

fn build(app: &gtk::Application) {
    let win = gtk::ApplicationWindow::new(app);
    win.set_title(Some("SimpleGo Server"));
    win.add_css_class("cd-window");
    win.set_decorated(false);
    win.set_default_size(1280, 800);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("cd-root");

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 4);
    titles.set_hexpand(true);
    let kicker = gtk::Label::new(None);
    kicker.set_markup("<span foreground=\"#ECE6D6\">SIMPLE</span><span foreground=\"#D4AF37\">GO</span><span foreground=\"#ECE6D6\"> SERVER</span>");
    kicker.add_css_class("cd-kicker");
    kicker.set_xalign(0.0);
    let title = gtk::Label::new(Some("Onion-only server, monitored through Tor"));
    title.add_css_class("cd-title");
    title.set_xalign(0.0);
    titles.append(&kicker);
    titles.append(&title);
    head.append(&titles);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::None);
    let nav = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    nav.set_valign(gtk::Align::Center);
    let pages = [("overview", "Overview"), ("history", "History"), ("events", "Events")];
    let mut nav_buttons = Vec::new();
    for (name, label) in pages {
        let b = gtk::Button::with_label(label);
        b.add_css_class("cd-btn");
        b.add_css_class("srv-nav");
        nav.append(&b);
        nav_buttons.push((name, b));
    }
    head.append(&nav);
    let refresh = gtk::Button::with_label("Check now");
    refresh.add_css_class("cd-btn");
    refresh.set_valign(gtk::Align::Center);
    refresh.set_tooltip_text(Some("Ask the server right now through Tor (takes a few seconds)"));
    head.append(&refresh);
    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("cd-close");
    close.set_valign(gtk::Align::Center);
    close.set_tooltip_text(Some("Close (Esc)"));
    head.append(&close);
    root.append(&head);

    let overview = build_overview();
    let history = build_history();
    let (events_page, events) = build_events();
    stack.add_named(&overview.page, Some("overview"));
    stack.add_named(&history.page, Some("history"));
    stack.add_named(&events_page, Some("events"));
    stack.set_margin_top(24);
    stack.set_vexpand(true);
    root.append(&stack);

    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    foot.add_css_class("cd-foot");
    let status = gtk::Label::new(None);
    status.add_css_class("cd-sub");
    status.set_xalign(0.0);
    status.set_hexpand(true);
    foot.append(&status);
    root.append(&foot);

    let ui = Rc::new(Ui { overview, history, events, status, busy: Cell::new(false) });

    let nav_buttons = Rc::new(nav_buttons);
    let show_page: Rc<dyn Fn(&str)> = {
        let stack = stack.clone();
        let nb = nav_buttons.clone();
        Rc::new(move |name: &str| {
            stack.set_visible_child_name(name);
            for (n, b) in nb.iter() {
                if *n == name { b.add_css_class("active") } else { b.remove_css_class("active") }
            }
        })
    };
    for (name, b) in nav_buttons.iter() {
        let sp = show_page.clone();
        let name = *name;
        b.connect_clicked(move |_| sp(name));
    }
    for (h, b) in &ui.history.buttons {
        let ui2 = ui.clone();
        let h = *h;
        b.connect_clicked(move |_| {
            ui2.history.hours.set(h);
            reload_history(&ui2);
        });
    }
    {
        let ui2 = ui.clone();
        refresh.connect_clicked(move |btn| {
            if ui2.busy.replace(true) {
                return;
            }
            btn.set_label("Checking...");
            btn.set_sensitive(false);
            let ui3 = ui2.clone();
            let btn = btn.clone();
            glib::spawn_future_local(async move {
                let _ = gio::spawn_blocking(|| {
                    std::process::Command::new("sentinel-server-poll").status().map(|s| s.success()).unwrap_or(false)
                })
                .await;
                btn.set_label("Check now");
                btn.set_sensitive(true);
                ui3.busy.set(false);
                reload_all(&ui3);
            });
        });
    }
    {
        let w = win.clone();
        close.connect_clicked(move |_| w.close());
    }
    let keys = gtk::EventControllerKey::new();
    {
        let w = win.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                w.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    win.add_controller(keys);

    win.set_child(Some(&root));
    win.maximize();
    win.present();

    let first = std::env::var("SX_SRV_PAGE").unwrap_or_else(|_| "overview".into());
    show_page(if ["history", "events"].contains(&first.as_str()) { &first } else { "overview" });
    reload_all(&ui);
    // The poller writes a new report every minute; follow it while the window is open.
    let ui2 = ui.clone();
    glib::timeout_add_seconds_local(30, move || {
        reload_all(&ui2);
        glib::ControlFlow::Continue
    });
}

fn setup_display() {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(true);
        settings.set_gtk_enable_animations(false);
        settings.set_gtk_icon_theme_name(Some("Papirus-Dark"));
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(concat!(
        include_str!("../style.css"),
        include_str!("../cleandrop.css"),
        include_str!("../serverapp.css")
    ));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_USER);
    }
}

/// One window at a time: a new start replaces a running instance, so a click on the
/// SERVER field or the sidebar icon always brings the app up in front.
fn take_over_instance() {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let pidfile = dir.join("sentinel-server.pid");
    let me = std::process::id();
    if let Some(pid) = std::fs::read_to_string(&pidfile).ok().and_then(|s| s.trim().parse::<u32>().ok()) {
        let comm_ok = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|c| c.trim() == "sentinel-server" || c.trim() == "serverapp")
            .unwrap_or(false);
        // Field 3 of /proc/PID/stat is the state; Z = finished but not reaped.
        let running = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| s.rsplit(')').next().map(|r| !r.trim_start().starts_with('Z')))
            .unwrap_or(false);
        if pid != me && comm_ok && running {
            let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
            for _ in 0..20 {
                if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    let _ = std::fs::write(&pidfile, me.to_string());
}

fn main() -> glib::ExitCode {
    take_over_instance();
    if std::env::var_os("GSK_RENDERER").is_none() {
        std::env::set_var("GSK_RENDERER", "cairo");
    }
    // Stable window identifier for the compositor rule that removes the title bar.
    glib::set_prgname(Some("sentinel-server"));
    // NON_UNIQUE: works without a session bus; the shell starts only one instance.
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(|_| setup_display());
    app.connect_activate(build);
    // A second click on the SERVER field or the sidebar icon sends SIGTERM: quit cleanly.
    let a2 = app.clone();
    glib::unix_signal_add_local(15, move || {
        a2.quit();
        glib::ControlFlow::Break
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}
