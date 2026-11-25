//! Clean Drop: shows and removes hidden metadata (GPS, camera, author, software, ...)
//! from files before they are sent. Cleaned copies go to ~/Outbox; the original is
//! kept or deleted, the user decides per file. Cleaning is done by mat2; every result
//! is checked again with mat2 before it is shown as clean.
//!
//! `sentinel-cleandrop [FILES...]` opens the window (files are added at once).
//! `sentinel-cleandrop --preview` renders a demo state for screenshots.

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::path::{Path, PathBuf};
use std::rc::Rc;

const APP_ID: &str = "org.sentinelx.CleanDrop";

// ---------------------------------------------------------------- mat2

/// What mat2 reports for one file.
#[derive(Clone, Debug)]
enum Report {
    /// Metadata found: (key, value) pairs.
    Found(Vec<(String, String)>),
    Clean,
    Unsupported(String),
    Error(String),
}

fn mat2_cmd() -> Vec<String> {
    // SX_MAT2 allows a different interpreter on development machines.
    std::env::var("SX_MAT2")
        .unwrap_or_else(|_| "mat2".into())
        .split_whitespace()
        .map(String::from)
        .collect()
}

fn run_mat2(args: &[&str], file: &Path) -> Result<String, String> {
    let cmd = mat2_cmd();
    let out = std::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .args(args)
        .arg(file)
        .output()
        .map_err(|e| format!("mat2 not available: {e}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(text)
    } else {
        Err(text.lines().last().unwrap_or("mat2 failed").trim().to_string())
    }
}

fn parse_show(text: &str) -> Report {
    if text.contains("is not supported") {
        let reason = text
            .lines()
            .find(|l| l.contains("is not supported"))
            .and_then(|l| l.find('(').map(|i| l[i..].trim().to_string()))
            .unwrap_or_default();
        return Report::Unsupported(reason);
    }
    if text.contains("No metadata found") {
        return Report::Clean;
    }
    let mut fields = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with("[+]") || l.starts_with("[-]") {
            continue;
        }
        if let Some((k, v)) = l.split_once(": ") {
            if is_technical(k, v) {
                continue;
            }
            fields.push((k.trim_start_matches("- ").to_string(), v.to_string()));
        } else {
            fields.push((l.trim_end_matches(':').to_string(), String::new()));
        }
    }
    if fields.is_empty() {
        Report::Clean
    } else {
        Report::Found(fields)
    }
}

/// Purely technical values that identify nobody: the PDF version and unset dates (-1).
fn is_technical(key: &str, value: &str) -> bool {
    let k = key.trim().to_lowercase();
    k == "format" || ((k == "creation-date" || k == "mod-date") && value.trim() == "-1")
}

fn inspect(file: &Path) -> Report {
    match run_mat2(&["--show"], file) {
        Ok(t) => parse_show(&t),
        Err(e) => Report::Error(e),
    }
}

fn outbox_dir() -> PathBuf {
    glib::home_dir().join("Outbox")
}

/// Free name in the outbox: name.ext, name-1.ext, name-2.ext, ...
fn outbox_target(src: &Path) -> PathBuf {
    let dir = outbox_dir();
    let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
    let ext = src.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut p = dir.join(format!("{stem}{ext}"));
    let mut i = 1;
    while p.exists() {
        p = dir.join(format!("{stem}-{i}{ext}"));
        i += 1;
    }
    p
}

/// Copies the file into the outbox, cleans the copy in place and checks it again.
/// Returns the cleaned file and what mat2 still finds in it.
fn clean_to_outbox(src: &Path) -> Result<(PathBuf, Report), String> {
    std::fs::create_dir_all(outbox_dir()).map_err(|e| e.to_string())?;
    let dst = outbox_target(src);
    std::fs::copy(src, &dst).map_err(|e| format!("copy failed: {e}"))?;
    let result = run_mat2(&["--inplace"], &dst);
    let text = match result {
        Ok(t) => t,
        Err(e) => {
            let _ = std::fs::remove_file(&dst);
            return Err(e);
        }
    };
    if text.contains("is not supported") || text.contains("[-]") {
        let _ = std::fs::remove_file(&dst);
        return Err(text.lines().next().unwrap_or("cleaning failed").trim().to_string());
    }
    let after = inspect(&dst);
    Ok((dst, after))
}

// ---------------------------------------------------------------- formatting

fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else if b >= 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Fields that reveal a person, a place or a device are marked as critical.
fn is_critical(key: &str) -> bool {
    let k = key.to_lowercase();
    ["gps", "location", "author", "artist", "creator", "owner", "serial", "make", "model",
     "lens", "software", "producer", "company", "manager", "user", "copyright", "hostname",
     "device", "camera", "lastmodifiedby", "last modified by"]
        .iter()
        .any(|w| k.contains(w))
}

// ---------------------------------------------------------------- UI

struct Row {
    path: PathBuf,
    widget: gtk::Box,
    status: gtk::Label,
    details: gtk::Box,
    toggle: gtk::Button,
    actions: gtk::Box,
    report: std::cell::RefCell<Option<Report>>,
    done: std::cell::Cell<bool>,
}

struct Ui {
    list: gtk::Box,
    empty: gtk::Widget,
    scroller: gtk::ScrolledWindow,
    clean_btn: gtk::Button,
    summary: gtk::Label,
    rows: std::cell::RefCell<Vec<Rc<Row>>>,
}

fn set_status(label: &gtk::Label, text: &str, class: &str) {
    for c in ["ok", "wait", "fail", "muted"] {
        label.remove_css_class(c);
    }
    label.set_text(text);
    label.add_css_class(class);
}

fn fill_details(row: &Row, fields: &[(String, String)]) {
    while let Some(c) = row.details.first_child() {
        row.details.remove(&c);
    }
    for (k, v) in fields {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        line.add_css_class("cd-field");
        let kl = gtk::Label::new(Some(k));
        kl.add_css_class("cd-field-key");
        if is_critical(k) {
            kl.add_css_class("critical");
        }
        kl.set_xalign(0.0);
        kl.set_width_chars(26);
        kl.set_max_width_chars(26);
        kl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let vl = gtk::Label::new(Some(v));
        vl.add_css_class("cd-field-val");
        vl.set_xalign(0.0);
        vl.set_hexpand(true);
        vl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        vl.set_selectable(true);
        line.append(&kl);
        line.append(&vl);
        row.details.append(&line);
    }
}

fn show_report(row: &Row, report: &Report) {
    match report {
        Report::Found(fields) => {
            let critical = fields.iter().filter(|(k, _)| is_critical(k)).count();
            let text = if critical > 0 {
                format!("{} FIELDS · {} CRITICAL", fields.len(), critical)
            } else {
                format!("{} FIELDS", fields.len())
            };
            set_status(&row.status, &text, "fail");
            fill_details(row, fields);
            row.toggle.set_visible(true);
        }
        Report::Clean => {
            set_status(&row.status, "NO METADATA", "ok");
            row.toggle.set_visible(false);
        }
        Report::Unsupported(r) => {
            set_status(&row.status, "NOT SUPPORTED", "wait");
            row.status.set_tooltip_text(Some(&format!("mat2 cannot clean this format {r}")));
            row.toggle.set_visible(false);
        }
        Report::Error(e) => {
            set_status(&row.status, "ERROR", "fail");
            row.status.set_tooltip_text(Some(e));
            row.toggle.set_visible(false);
        }
    }
}

fn update_summary(ui: &Ui) {
    let rows = ui.rows.borrow();
    ui.empty.set_visible(rows.is_empty());
    ui.scroller.set_visible(!rows.is_empty());
    let pending: Vec<_> = rows
        .iter()
        .filter(|r| !r.done.get() && matches!(*r.report.borrow(), Some(Report::Found(_)) | Some(Report::Clean)))
        .collect();
    let fields: usize = pending
        .iter()
        .map(|r| match &*r.report.borrow() {
            Some(Report::Found(f)) => f.len(),
            _ => 0,
        })
        .sum();
    ui.clean_btn.set_sensitive(!pending.is_empty());
    ui.summary.set_text(&if rows.is_empty() {
        String::new()
    } else if pending.is_empty() {
        "Nothing left to clean".into()
    } else {
        format!("{} file(s) · {} metadata field(s) to remove", pending.len(), fields)
    });
}

fn add_file(ui: &Rc<Ui>, path: PathBuf) {
    if !path.is_file() || ui.rows.borrow().iter().any(|r| r.path == path) {
        return;
    }
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("cd-row");

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let file = gio::File::for_path(&path);
    let icon = file
        .query_info("standard::icon", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
        .ok()
        .and_then(|i| i.icon());
    let img = match icon {
        Some(i) => gtk::Image::from_gicon(&i),
        None => gtk::Image::from_icon_name("text-x-generic"),
    };
    img.set_pixel_size(36);
    head.append(&img);

    let names = gtk::Box::new(gtk::Orientation::Vertical, 2);
    names.set_hexpand(true);
    names.set_valign(gtk::Align::Center);
    let name = gtk::Label::new(path.file_name().map(|n| n.to_string_lossy().to_string()).as_deref());
    name.add_css_class("cd-name");
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let parent = path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let sub = gtk::Label::new(Some(&format!("{} · {}", human_size(size), parent)));
    sub.add_css_class("cd-sub");
    sub.set_xalign(0.0);
    sub.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    names.append(&name);
    names.append(&sub);
    head.append(&names);

    let toggle = gtk::Button::with_label("Show");
    toggle.add_css_class("cd-link");
    toggle.set_valign(gtk::Align::Center);
    toggle.set_visible(false);
    head.append(&toggle);

    let status = gtk::Label::new(Some("CHECKING"));
    status.add_css_class("cd-status");
    status.add_css_class("muted");
    status.set_valign(gtk::Align::Center);
    head.append(&status);

    let remove = gtk::Button::from_icon_name("window-close-symbolic");
    remove.add_css_class("cd-icon-btn");
    remove.set_valign(gtk::Align::Center);
    remove.set_tooltip_text(Some("Remove from list"));
    head.append(&remove);
    card.append(&head);

    let details = gtk::Box::new(gtk::Orientation::Vertical, 0);
    details.add_css_class("cd-details");
    details.set_visible(false);
    card.append(&details);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    actions.add_css_class("cd-actions");
    actions.set_visible(false);
    card.append(&actions);

    let row = Rc::new(Row {
        path: path.clone(),
        widget: card.clone(),
        status,
        details: details.clone(),
        toggle: toggle.clone(),
        actions,
        report: std::cell::RefCell::new(None),
        done: std::cell::Cell::new(false),
    });
    toggle.connect_clicked(move |b| {
        let open = !details.is_visible();
        details.set_visible(open);
        b.set_label(if open { "Hide" } else { "Show" });
    });
    {
        let ui = ui.clone();
        let row = row.clone();
        remove.connect_clicked(move |_| {
            ui.list.remove(&row.widget);
            ui.rows.borrow_mut().retain(|r| !Rc::ptr_eq(r, &row));
            update_summary(&ui);
        });
    }
    ui.list.append(&card);
    ui.rows.borrow_mut().push(row.clone());
    update_summary(ui);

    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let p = row.path.clone();
        let report = gio::spawn_blocking(move || inspect(&p)).await.unwrap_or(Report::Error("check failed".into()));
        show_report(&row, &report);
        *row.report.borrow_mut() = Some(report);
        update_summary(&ui);
    });
}

fn show_done(ui: &Rc<Ui>, row: &Rc<Row>, result: Result<(PathBuf, Report), String>) {
    row.details.set_visible(false);
    row.toggle.set_label("Show");
    match result {
        Ok((dst, after)) => {
            row.done.set(true);
            let name = dst.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            match &after {
                Report::Clean => set_status(&row.status, "CLEAN · VERIFIED", "ok"),
                Report::Found(f) => {
                    set_status(&row.status, &format!("CLEANED · {} LEFT", f.len()), "wait");
                    row.status.set_tooltip_text(Some("mat2 cannot remove these fields; see Show"));
                    fill_details(row, f);
                    row.toggle.set_visible(true);
                }
                _ => set_status(&row.status, "CLEANED", "ok"),
            }
            if matches!(after, Report::Clean) {
                row.toggle.set_visible(false);
            }
            let a = &row.actions;
            while let Some(c) = a.first_child() {
                a.remove(&c);
            }
            let saved = gtk::Label::new(Some(&format!("Saved to Outbox/{name}")));
            saved.add_css_class("cd-sub");
            saved.set_hexpand(true);
            saved.set_xalign(0.0);
            a.append(&saved);
            let q = gtk::Label::new(Some("Original:"));
            q.add_css_class("cd-sub");
            a.append(&q);
            let keep = gtk::Button::with_label("Keep");
            keep.add_css_class("cd-small");
            let del = gtk::Button::with_label("Delete");
            del.add_css_class("cd-small");
            del.add_css_class("danger");
            a.append(&keep);
            a.append(&del);
            a.set_visible(true);
            {
                let a2 = a.clone();
                keep.connect_clicked(move |_| {
                    a2.set_visible(false);
                });
            }
            {
                let row2 = row.clone();
                del.connect_clicked(move |_| {
                    let a = &row2.actions;
                    while let Some(c) = a.first_child() {
                        a.remove(&c);
                    }
                    let msg = match std::fs::remove_file(&row2.path) {
                        Ok(_) => "Original deleted".to_string(),
                        Err(e) => format!("Could not delete the original: {e}"),
                    };
                    let l = gtk::Label::new(Some(&msg));
                    l.add_css_class("cd-sub");
                    l.set_xalign(0.0);
                    a.append(&l);
                });
            }
        }
        Err(e) => {
            set_status(&row.status, "FAILED", "fail");
            row.status.set_tooltip_text(Some(&e));
        }
    }
    update_summary(ui);
}

fn clean_all(ui: &Rc<Ui>) {
    let rows: Vec<_> = ui
        .rows
        .borrow()
        .iter()
        .filter(|r| !r.done.get() && matches!(*r.report.borrow(), Some(Report::Found(_)) | Some(Report::Clean)))
        .cloned()
        .collect();
    ui.clean_btn.set_sensitive(false);
    for row in rows {
        set_status(&row.status, "CLEANING", "muted");
        let ui = ui.clone();
        glib::spawn_future_local(async move {
            let p = row.path.clone();
            let result = gio::spawn_blocking(move || clean_to_outbox(&p))
                .await
                .unwrap_or_else(|_| Err("cleaning crashed".into()));
            show_done(&ui, &row, result);
        });
    }
}

fn open_outbox() {
    let _ = std::fs::create_dir_all(outbox_dir());
    let uri = gio::File::for_path(outbox_dir()).uri();
    let _ = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE);
}

fn build(app: &gtk::Application, files: Vec<PathBuf>) {
    let win = gtk::ApplicationWindow::new(app);
    win.set_title(Some("Clean Drop"));
    win.add_css_class("cd-window");
    win.set_decorated(false);
    win.set_default_size(1280, 800);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("cd-root");

    // Header
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 4);
    titles.set_hexpand(true);
    let kicker = gtk::Label::new(Some("CLEAN DROP"));
    kicker.add_css_class("cd-kicker");
    kicker.set_xalign(0.0);
    let title = gtk::Label::new(Some("Remove hidden metadata before you send"));
    title.add_css_class("cd-title");
    title.set_xalign(0.0);
    titles.append(&kicker);
    titles.append(&title);
    head.append(&titles);
    let add = gtk::Button::with_label("Add files");
    add.add_css_class("cd-btn");
    add.set_valign(gtk::Align::Center);
    let outbox = gtk::Button::with_label("Open Outbox");
    outbox.add_css_class("cd-btn");
    outbox.set_valign(gtk::Align::Center);
    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("cd-close");
    close.set_valign(gtk::Align::Center);
    close.set_tooltip_text(Some("Close (Esc)"));
    head.append(&add);
    head.append(&outbox);
    head.append(&close);
    root.append(&head);

    // Empty state / drop zone
    let empty = gtk::Box::new(gtk::Orientation::Vertical, 8);
    empty.add_css_class("cd-drop");
    empty.set_vexpand(true);
    empty.set_valign(gtk::Align::Fill);
    let e1 = gtk::Label::new(Some("Drop files here"));
    e1.add_css_class("cd-drop-title");
    let e2 = gtk::Label::new(Some("Photos, PDFs, Office documents, audio and video. Originals stay untouched; cleaned copies go to your Outbox."));
    e2.add_css_class("cd-sub");
    e2.set_wrap(true);
    e2.set_justify(gtk::Justification::Center);
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 8);
    inner.set_valign(gtk::Align::Center);
    inner.set_vexpand(true);
    inner.append(&e1);
    inner.append(&e2);
    empty.append(&inner);
    root.append(&empty);

    // File list
    let list = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&list));
    scroller.set_vexpand(true);
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.add_css_class("cd-scroll");
    root.append(&scroller);

    // Footer
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    foot.add_css_class("cd-foot");
    let summary = gtk::Label::new(None);
    summary.add_css_class("cd-sub");
    summary.set_hexpand(true);
    summary.set_xalign(0.0);
    let clean_btn = gtk::Button::with_label("Clean all  →  Outbox");
    clean_btn.add_css_class("cd-primary");
    clean_btn.set_sensitive(false);
    foot.append(&summary);
    foot.append(&clean_btn);
    root.append(&foot);

    let ui = Rc::new(Ui {
        list,
        empty: empty.clone().upcast(),
        scroller: scroller.clone(),
        clean_btn: clean_btn.clone(),
        summary,
        rows: std::cell::RefCell::new(Vec::new()),
    });
    scroller.set_visible(false);

    {
        let ui = ui.clone();
        clean_btn.connect_clicked(move |_| clean_all(&ui));
    }
    outbox.connect_clicked(|_| open_outbox());
    {
        let w = win.clone();
        close.connect_clicked(move |_| w.close());
    }
    {
        let ui = ui.clone();
        let w = win.clone();
        add.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder().title("Add files").modal(true).build();
            let ui = ui.clone();
            dialog.open_multiple(Some(&w), gio::Cancellable::NONE, move |res| {
                if let Ok(model) = res {
                    for i in 0..model.n_items() {
                        if let Some(f) = model.item(i).and_downcast::<gio::File>() {
                            if let Some(p) = f.path() {
                                add_file(&ui, p);
                            }
                        }
                    }
                }
            });
        });
    }

    // Drag and drop from the file manager.
    let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    {
        let ui = ui.clone();
        drop.connect_drop(move |_, value, _, _| {
            if let Ok(list) = value.get::<gdk::FileList>() {
                for f in list.files() {
                    if let Some(p) = f.path() {
                        add_file(&ui, p);
                    }
                }
                return true;
            }
            false
        });
    }
    root.add_controller(drop);

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

    for f in files {
        add_file(&ui, f);
    }

    // Screenshot hooks for development only (no effect unless the variables are set).
    if std::env::var_os("SX_CD_EXPAND").is_some() {
        let ui = ui.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(2500), move || {
            if let Some(r) = ui.rows.borrow().first() {
                r.details.set_visible(true);
                r.toggle.set_label("Hide");
            }
        });
    }
    if std::env::var_os("SX_CD_AUTOCLEAN").is_some() {
        let ui = ui.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(2500), move || clean_all(&ui));
    }
}

fn setup_display() {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(true);
        settings.set_gtk_enable_animations(false);
        settings.set_gtk_icon_theme_name(Some("Papirus-Dark"));
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(concat!(include_str!("../style.css"), include_str!("../cleandrop.css")));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_USER);
    }
}

fn main() -> glib::ExitCode {
    if std::env::var_os("GSK_RENDERER").is_none() {
        std::env::set_var("GSK_RENDERER", "cairo");
    }
    let files: Vec<PathBuf> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .map(|a| match a.strip_prefix("file://") {
            Some(rest) => PathBuf::from(glib::Uri::unescape_string(rest, None::<&str>).map(|s| s.to_string()).unwrap_or_else(|| rest.to_string())),
            None => PathBuf::from(a),
        })
        .collect();
    // Stable window identifier for the compositor rule that removes the title bar.
    glib::set_prgname(Some("sentinel-cleandrop"));
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(|_| setup_display());
    app.connect_activate(move |app| build(app, files.clone()));
    // The sidebar shows a marker while Clean Drop runs and closes it on a second click:
    // record the PID, and quit cleanly on SIGTERM.
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::write(dir.join("sentinel-cleandrop.pid"), std::process::id().to_string());
    let a2 = app.clone();
    glib::unix_signal_add_local(15, move || {
        a2.quit();
        glib::ControlFlow::Break
    });
    app.run_with_args(&[std::env::args().next().unwrap_or_default()])
}
