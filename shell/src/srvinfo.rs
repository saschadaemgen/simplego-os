//! Info panel of the SERVER field: slides out directly below the top bar while the
//! pointer rests on the field, and closes again shortly after the pointer has left
//! both the field and the panel. A click on the field opens SimpleGo Server, a second
//! click closes it.
//!
//! Content: verdict with a status light, the current warnings in plain language and
//! four figures (reachability over 24 h, typical response time, vanguards, age of the
//! last report). Data comes from `sentinel-server-data`, read in the background.

use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

const PANEL_W: i32 = 400;
const SLIDE_MS: f64 = 180.0;
const SHOW_DELAY_MS: u64 = 180;
const HIDE_DELAY_MS: u64 = 260;

struct Data {
    now: HashMap<String, String>,
    issues: Vec<(String, String)>,
    summary: HashMap<String, String>,
}

fn rows(args: &[&str]) -> Vec<Vec<String>> {
    std::process::Command::new("sentinel-server-data")
        .args(args)
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| l.split('\t').map(str::to_string).collect())
                .collect()
        })
        .unwrap_or_default()
}

fn load() -> Data {
    let mut now = HashMap::new();
    let mut issues = Vec::new();
    for r in rows(&["now"]) {
        match r.first().map(String::as_str) {
            Some("issue") if r.len() >= 3 => issues.push((r[1].clone(), r[2].clone())),
            Some("service") => {}
            Some(k) if r.len() >= 2 => {
                now.insert(k.to_string(), r[1].clone());
            }
            _ => {}
        }
    }
    let summary = rows(&["summary", "24"]).into_iter().filter(|r| r.len() >= 2).map(|r| (r[0].clone(), r[1].clone())).collect();
    Data { now, issues, summary }
}

fn class_of(level: &str) -> &'static str {
    match level {
        "ok" => "ok",
        "warn" => "wait",
        "fail" => "fail",
        _ => "lost",
    }
}

fn set_class(w: &impl IsA<gtk::Widget>, class: &str) {
    for c in ["ok", "wait", "fail", "lost"] {
        w.remove_css_class(c);
    }
    w.add_css_class(class);
}

fn age_text(s: Option<i64>) -> String {
    match s {
        None => "never".into(),
        Some(s) if s < 90 => format!("{s} s ago"),
        Some(s) if s < 5400 => format!("{} min ago", s / 60),
        Some(s) => format!("{} h ago", s / 3600),
    }
}

fn set_height(sw: &gtk::ScrolledWindow, h: i32) {
    // Order matters: the minimum may never exceed the maximum.
    sw.set_max_content_height(-1);
    sw.set_min_content_height(h);
    sw.set_max_content_height(h);
    sw.set_size_request(PANEL_W, h);
}

pub struct InfoPanel {
    win: gtk::ApplicationWindow,
    clip: gtk::ScrolledWindow,
    card: gtk::Box,
    dot: gtk::Box,
    title: gtk::Label,
    issues: gtk::Box,
    figs: Vec<(gtk::Label, gtk::Label)>,
    /// Current revealed height, 0 = closed.
    shown: Cell<f64>,
    target_open: Cell<bool>,
    anim: RefCell<Option<gtk::TickCallbackId>>,
    pending: RefCell<Option<glib::SourceId>>,
    over_field: Cell<bool>,
    over_panel: Cell<bool>,
}

fn figure(grid: &gtk::Grid, idx: i32, key: &str) -> (gtk::Label, gtk::Label) {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    b.add_css_class("sx-info-fig");
    let v = gtk::Label::new(Some("..."));
    v.add_css_class("sx-info-fig-val");
    v.set_xalign(0.0);
    let k = gtk::Label::new(Some(key));
    k.add_css_class("sx-info-fig-key");
    k.set_xalign(0.0);
    b.append(&v);
    b.append(&k);
    grid.attach(&b, idx % 2, idx / 2, 1, 1);
    (v, k)
}

impl InfoPanel {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let win = gtk::ApplicationWindow::new(app);
        win.add_css_class("sx-info-win");
        win.set_decorated(false);
        win.set_resizable(false);

        let card = gtk::Box::new(gtk::Orientation::Vertical, 14);
        card.add_css_class("sx-info");
        card.set_size_request(PANEL_W, -1);
        card.set_valign(gtk::Align::End);

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("sx-info-dot");
        dot.set_valign(gtk::Align::Center);
        let tb = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let kicker = gtk::Label::new(Some("SERVER"));
        kicker.add_css_class("sx-info-kicker");
        kicker.set_xalign(0.0);
        let title = gtk::Label::new(Some("..."));
        title.add_css_class("sx-info-title");
        title.set_xalign(0.0);
        tb.append(&kicker);
        tb.append(&title);
        head.append(&dot);
        head.append(&tb);
        card.append(&head);

        let issues = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.append(&issues);

        let grid = gtk::Grid::new();
        grid.set_column_homogeneous(true);
        grid.set_row_spacing(8);
        grid.set_column_spacing(8);
        let figs = vec![
            figure(&grid, 0, "REACHABLE 24 H"),
            figure(&grid, 1, "TYPICAL RESPONSE"),
            figure(&grid, 2, "VANGUARDS"),
            figure(&grid, 3, "LAST REPORT"),
        ];
        card.append(&grid);

        let hint = gtk::Label::new(Some("Click the field to open or close SimpleGo Server"));
        hint.add_css_class("sx-info-hint");
        hint.set_xalign(0.0);
        card.append(&hint);

        // A viewport that grows from 0 to the card's height and always shows the card's
        // lower end, so the card looks as if it slides out from under the bar.
        let clip = gtk::ScrolledWindow::new();
        clip.set_policy(gtk::PolicyType::Never, gtk::PolicyType::External);
        clip.set_propagate_natural_width(true);
        clip.set_child(Some(&card));
        clip.add_css_class("sx-info-clip");
        set_height(&clip, 1);
        win.set_child(Some(&clip));

        #[cfg(feature = "layer-shell")]
        {
            use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
            win.init_layer_shell();
            win.set_namespace("sentinel-srvinfo");
            win.set_layer(Layer::Overlay);
            win.set_anchor(Edge::Top, true);
            win.set_anchor(Edge::Right, true);
            // Exclusive zone 0: the compositor places it below the top bar.
            win.set_exclusive_zone(0);
            win.set_keyboard_mode(KeyboardMode::None);
        }

        let p = Rc::new(InfoPanel {
            win,
            clip,
            card,
            dot,
            title,
            issues,
            figs,
            shown: Cell::new(0.0),
            target_open: Cell::new(false),
            anim: RefCell::new(None),
            pending: RefCell::new(None),
            over_field: Cell::new(false),
            over_panel: Cell::new(false),
        });

        let motion = gtk::EventControllerMotion::new();
        let p1 = Rc::downgrade(&p);
        motion.connect_enter(move |_, _, _| {
            if let Some(p) = p1.upgrade() {
                p.over_panel.set(true);
                p.cancel_pending();
            }
        });
        let p2 = Rc::downgrade(&p);
        motion.connect_leave(move |_| {
            if let Some(p) = p2.upgrade() {
                p.over_panel.set(false);
                p.schedule(false);
            }
        });
        p.card.add_controller(motion);
        p
    }

    /// Hover and click handling for the SERVER field. `bar` is the top bar's root
    /// widget, used to line the panel up with the field's right edge.
    pub fn attach(self: &Rc<Self>, field: &gtk::Box, bar: &gtk::Widget) {
        let motion = gtk::EventControllerMotion::new();
        let p1 = Rc::downgrade(self);
        let f1 = field.clone();
        let b1 = bar.clone();
        motion.connect_enter(move |_, _, _| {
            if let Some(p) = p1.upgrade() {
                p.over_field.set(true);
                p.align_to(&f1, &b1);
                p.schedule(true);
            }
        });
        let p2 = Rc::downgrade(self);
        motion.connect_leave(move |_| {
            if let Some(p) = p2.upgrade() {
                p.over_field.set(false);
                p.schedule(false);
            }
        });
        field.add_controller(motion);
    }

    #[allow(unused_variables)]
    fn align_to(&self, field: &gtk::Box, bar: &gtk::Widget) {
        #[cfg(feature = "layer-shell")]
        if let Some(r) = field.compute_bounds(bar) {
            use gtk4_layer_shell::{Edge, LayerShell};
            let right = bar.width() as f32 - (r.x() + r.width());
            self.win.set_margin(Edge::Right, right.max(0.0).round() as i32);
        }
    }

    fn cancel_pending(&self) {
        if let Some(id) = self.pending.borrow_mut().take() {
            id.remove();
        }
    }

    /// Opens after a short rest on the field, closes after a short grace period, so
    /// passing over the field or moving from the field into the panel does not flicker.
    fn schedule(self: &Rc<Self>, open: bool) {
        self.cancel_pending();
        if !open && (self.over_field.get() || self.over_panel.get()) {
            return;
        }
        let delay = if open { SHOW_DELAY_MS } else { HIDE_DELAY_MS };
        let p = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || {
            if let Some(p) = p.upgrade() {
                p.pending.borrow_mut().take();
                if open {
                    p.open();
                } else if !p.over_field.get() && !p.over_panel.get() {
                    p.slide(false);
                }
            }
        });
        *self.pending.borrow_mut() = Some(id);
    }

    /// Opens without hovering (screenshots during development: SX_PREVIEW_SRVINFO=1).
    pub fn preview_open(self: &Rc<Self>) {
        self.over_field.set(true);
        self.open();
    }

    pub fn close_now(self: &Rc<Self>) {
        self.cancel_pending();
        self.slide(false);
    }

    fn open(self: &Rc<Self>) {
        let p = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(d) = gio::spawn_blocking(load).await {
                p.fill(&d);
            }
            if p.over_field.get() || p.over_panel.get() {
                p.slide(true);
            }
        });
    }

    fn fill(&self, d: &Data) {
        let level = d.now.get("level").map(String::as_str).unwrap_or("");
        let (title, class) = match level {
            "ok" => ("ALL GOOD", "ok"),
            "warn" => ("ATTENTION", "wait"),
            "fail" => ("PROBLEM", "fail"),
            "unreachable" => ("NO CONTACT", "lost"),
            _ => ("NO DATA YET", "lost"),
        };
        self.title.set_text(title);
        set_class(&self.title, class);
        set_class(&self.dot, class);

        while let Some(c) = self.issues.first_child() {
            self.issues.remove(&c);
        }
        let shown: Vec<_> = d.issues.iter().take(4).collect();
        if shown.is_empty() {
            let l = gtk::Label::new(Some(if level == "ok" { "No warnings." } else { "No details available." }));
            l.add_css_class("sx-info-issue");
            l.set_xalign(0.0);
            self.issues.append(&l);
        }
        for (lvl, text) in shown {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let mark = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            mark.add_css_class("sx-info-mark");
            mark.set_valign(gtk::Align::Center);
            set_class(&mark, class_of(lvl));
            let l = gtk::Label::new(Some(text));
            l.add_css_class("sx-info-issue");
            l.set_xalign(0.0);
            l.set_wrap(true);
            l.set_hexpand(true);
            row.append(&mark);
            row.append(&l);
            self.issues.append(&row);
        }
        if d.issues.len() > 4 {
            let l = gtk::Label::new(Some(&format!("and {} more in the app", d.issues.len() - 4)));
            l.add_css_class("sx-info-hint");
            l.set_xalign(0.0);
            self.issues.append(&l);
        }

        let avail = d.summary.get("availability_pct").filter(|v| !v.is_empty());
        let lat = d.summary.get("latency_median_ms").and_then(|v| v.parse::<f64>().ok());
        let vg = d.now.get("vanguards").map(String::as_str).unwrap_or("-");
        let age = d.now.get("age_s").and_then(|v| v.parse::<i64>().ok());
        let vals = [
            (avail.map(|v| format!("{v} %")).unwrap_or("-".into()),
             match avail.and_then(|v| v.parse::<f64>().ok()) { Some(v) if v >= 99.0 => "ok", Some(_) => "wait", None => "" }),
            (lat.map(|v| format!("{:.1} s", v / 1000.0)).unwrap_or("-".into()), ""),
            (vg.to_uppercase(), match vg { "active" => "ok", "not installed" => "", _ => "fail" }),
            (age_text(age), match age { Some(a) if a <= 300 => "", _ => "wait" }),
        ];
        for ((v, _), (txt, cls)) in self.figs.iter().zip(vals) {
            v.set_text(&txt);
            for c in ["ok", "wait", "fail", "lost"] {
                v.remove_css_class(c);
            }
            if !cls.is_empty() {
                v.add_css_class(cls);
            }
        }
    }

    fn full_height(&self) -> f64 {
        let (_, nat, _, _) = self.card.measure(gtk::Orientation::Vertical, PANEL_W);
        nat.max(1) as f64
    }

    /// Slides the panel out (open) or back under the bar (close), eased, about 180 ms.
    fn slide(self: &Rc<Self>, open: bool) {
        self.target_open.set(open);
        if open && !self.win.is_visible() {
            set_height(&self.clip, 1);
            self.win.set_visible(true);
        }
        if self.anim.borrow().is_some() {
            return; // the running animation follows target_open
        }
        let p = Rc::downgrade(self);
        let last: Cell<Option<i64>> = Cell::new(None);
        let id = self.clip.add_tick_callback(move |_, clock| {
            let Some(p) = p.upgrade() else { return glib::ControlFlow::Break };
            let t = clock.frame_time();
            let dt = last.get().map(|l| (t - l) as f64 / 1000.0).unwrap_or(16.0);
            last.set(Some(t));
            let full = p.full_height();
            let step = full * dt / SLIDE_MS;
            let cur = p.shown.get();
            let next = if p.target_open.get() { (cur + step).min(full) } else { (cur - step).max(0.0) };
            p.shown.set(next);
            // Ease-out on the way out, ease-in on the way back.
            let f = next / full;
            let eased = if p.target_open.get() { 1.0 - (1.0 - f).powi(3) } else { f * f * f };
            let h = ((full * eased).round() as i32).max(1);
            set_height(&p.clip, h);
            let adj = p.clip.vadjustment();
            adj.set_value((full - h as f64).max(0.0));
            let done = if p.target_open.get() { next >= full } else { next <= 0.0 };
            if done {
                if !p.target_open.get() {
                    p.win.set_visible(false);
                }
                p.anim.borrow_mut().take();
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
        *self.anim.borrow_mut() = Some(id);
    }
}
