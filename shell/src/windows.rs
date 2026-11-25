//! Open windows as the compositor reports them (wlr foreign toplevel, through wlrctl):
//! which app_ids have a window, and which one is active. Used for the running/active
//! markers in the sidebar and for the context buttons in the top bar.

pub struct Windows {
    pub open: Vec<String>,
    pub active: Option<String>,
}

fn list(extra: &[&str]) -> Option<Vec<String>> {
    let out = std::process::Command::new("wlrctl").arg("toplevel").arg("list").args(extra).output().ok()?;
    if !out.status.success() && out.stdout.is_empty() {
        return None;
    }
    // Lines look like "app_id: title".
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.split_once(": ").map(|(a, _)| a.to_string()).or_else(|| (!l.is_empty()).then(|| l.to_string())))
            .collect(),
    )
}

/// None if wlrctl is missing or the compositor does not answer.
pub fn query() -> Option<Windows> {
    let open = list(&[])?;
    let active = list(&["state:active"]).and_then(|v| v.into_iter().next());
    Some(Windows { open, active })
}

/// True if a window's app_id belongs to a launcher with these candidate ids.
pub fn matches(app_id: &str, candidates: &[String]) -> bool {
    let a = app_id.to_lowercase();
    candidates.iter().any(|c| a == *c || (c.len() >= 5 && a.contains(c.as_str())))
}
