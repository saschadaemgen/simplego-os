//! System status sources for the shell: CPU, memory, network, Tor, time sync, volume.
//! Everything is read from /proc, /sys, small status files or short-lived helper
//! commands. Nothing here keeps a process running in the background.

use std::fs;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Tracks counters between two samples so rates and loads can be derived.
#[derive(Default)]
pub struct Sampler {
    cpu_prev: Option<(u64, u64)>,
    net_prev: Option<(u64, Instant)>,
}

impl Sampler {
    /// CPU load in percent since the previous call (0 on the first call).
    pub fn cpu_percent(&mut self) -> u32 {
        let Ok(stat) = fs::read_to_string("/proc/stat") else { return 0 };
        let Some(line) = stat.lines().next() else { return 0 };
        let v: Vec<u64> = line
            .split_whitespace()
            .skip(1)
            .take(8)
            .filter_map(|x| x.parse().ok())
            .collect();
        if v.len() < 5 {
            return 0;
        }
        let total: u64 = v.iter().sum();
        let idle = v[3] + v[4];
        let pct = match self.cpu_prev {
            Some((pt, pi)) if total > pt => {
                let dt = total - pt;
                let di = idle.saturating_sub(pi);
                ((dt - di.min(dt)) * 100 / dt) as u32
            }
            _ => 0,
        };
        self.cpu_prev = Some((total, idle));
        pct
    }

    /// Network throughput (rx + tx) in bytes per second on the default route interface.
    pub fn net_bytes_per_sec(&mut self) -> u64 {
        let iface = default_iface().unwrap_or_else(|| "eth0".into());
        let read = |f: &str| -> u64 {
            fs::read_to_string(format!("/sys/class/net/{iface}/statistics/{f}"))
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0)
        };
        let bytes = read("rx_bytes") + read("tx_bytes");
        let now = Instant::now();
        let rate = match self.net_prev {
            Some((pb, pt)) => {
                let secs = now.duration_since(pt).as_secs_f64().max(0.5);
                (bytes.saturating_sub(pb) as f64 / secs) as u64
            }
            None => 0,
        };
        self.net_prev = Some((bytes, now));
        rate
    }
}

fn default_iface() -> Option<String> {
    let route = fs::read_to_string("/proc/net/route").ok()?;
    route
        .lines()
        .skip(1)
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
        .find(|f| f.len() > 1 && f[1] == "00000000")
        .map(|f| f[0].to_string())
}

/// Memory in use in percent (MemTotal minus MemAvailable).
pub fn ram_percent() -> u32 {
    let Ok(info) = fs::read_to_string("/proc/meminfo") else { return 0 };
    let get = |key: &str| -> u64 {
        info.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    let (total, avail) = (get("MemTotal:"), get("MemAvailable:"));
    if total == 0 {
        return 0;
    }
    ((total - avail.min(total)) * 100 / total) as u32
}

/// Human readable rate, e.g. "0K", "24K", "3M".
pub fn fmt_rate(bps: u64) -> String {
    // At most four characters, so the value always fits the narrow sidebar.
    let k = bps / 1024;
    let m = bps as f64 / (1024.0 * 1024.0);
    if k < 1000 {
        format!("{k}K")
    } else if m < 9.95 {
        format!("{m:.1}M")
    } else {
        format!("{}M", m.round() as u64)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Ok,
    Wait,
    Fail,
}

/// State of the Whonix time synchronisation (sdwdate), which runs over Tor.
pub fn time_sync() -> State {
    // Without sdwdate (SimpleGo OS desktop edition) the clock comes from the RTC: nothing to wait for.
    if !std::path::Path::new("/run/sdwdate").exists() {
        return State::Ok;
    }
    let s = fs::read_to_string("/run/sdwdate/status").unwrap_or_default();
    if s.contains("\"success\"") {
        State::Ok
    } else if s.contains("\"error\"") {
        State::Fail
    } else {
        State::Wait
    }
}

/// Live Tor check: asks check.torproject.org every two minutes in a worker thread.
/// The UI only reads the last result, so it never blocks.
pub struct TorCheck {
    last: Arc<Mutex<Option<bool>>>,
}

impl TorCheck {
    pub fn start() -> Self {
        let last = Arc::new(Mutex::new(None));
        let shared = Arc::clone(&last);
        thread::spawn(move || loop {
            // On the desktop edition the check goes through the local Tor SOCKS port named in
            // /etc/sentinelx/tor-socks (the firewall lets only Tor out); in the Whonix
            // Workstation every connection already goes through the gateway.
            let mut cmd = Command::new("curl");
            cmd.args(["-s", "--max-time", "25"]);
            if let Ok(p) = fs::read_to_string("/etc/sentinelx/tor-socks") {
                let p = p.trim();
                if !p.is_empty() {
                    cmd.args(["--socks5-hostname", p]);
                }
            }
            let ok = cmd
                .arg("https://check.torproject.org/api/ip")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains("\"IsTor\":true"))
                .unwrap_or(false);
            if let Ok(mut g) = shared.lock() {
                *g = Some(ok);
            }
            thread::sleep(Duration::from_secs(120));
        });
        TorCheck { last }
    }

    /// Ok = verified Tor exit, Wait = no answer yet or time sync still running, Fail = check failed.
    pub fn state(&self) -> State {
        match self.last.lock().ok().and_then(|g| *g) {
            Some(true) => State::Ok,
            None => State::Wait,
            Some(false) if time_sync() != State::Ok => State::Wait,
            Some(false) => State::Fail,
        }
    }
}

/// Output volume in percent and mute flag; None if no audio server answers.
pub fn volume() -> Option<(u32, bool)> {
    let out = Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let v: f64 = s.split_whitespace().nth(1)?.parse().ok()?;
    Some(((v * 100.0).round() as u32, s.contains("MUTED")))
}

/// Volume actions: "5%+", "5%-" or toggle mute.
pub fn volume_change(step: Option<&str>) {
    let mut cmd = Command::new("wpctl");
    match step {
        Some(s) => cmd.args(["set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@", s]),
        None => cmd.args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]),
    };
    let _ = cmd.status();
}

/// Server status as kept by the monitoring poller (~/.cache/sentinel-server):
/// None = monitoring not set up; "lost" = no report for more than five minutes;
/// otherwise the server's own verdict ("ok", "warn", "fail").
pub fn server_level() -> Option<String> {
    let dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join(".cache/sentinel-server");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    let read_ts = |f: &str| fs::read_to_string(dir.join(f)).ok().and_then(|s| s.trim().parse::<i64>().ok());
    match read_ts("last_ok") {
        Some(t) if now - t <= 300 => fs::read_to_string(dir.join("level")).ok().map(|s| s.trim().to_string()),
        Some(_) => Some("lost".into()),
        None if read_ts("last_fail").is_some() => Some("lost".into()),
        None => None,
    }
}
