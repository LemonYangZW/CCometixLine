use super::SegmentData;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Shared data types
// ---------------------------------------------------------------------------

pub struct UsageData {
    pub first_pct: f64,
    pub first_label: String,
    pub first_resets_in: Option<i64>, // seconds until window reset
    pub second_pct: f64,
    pub second_label: String,
    pub second_resets_in: Option<i64>,
}

pub struct UsageBarOpts {
    pub bar_width: usize,
    pub bar_style: String,
    pub bar_colored: bool,
}

// ---------------------------------------------------------------------------
// Progress bar rendering
// ---------------------------------------------------------------------------

/// Linear interpolation between two u8 values.
pub fn lerp(a: u8, b: u8, t: f64) -> u8 {
    (a as f64 + (b as f64 - a as f64) * t).clamp(0.0, 255.0) as u8
}

/// Smooth heat-map via multi-stop spline: cyan→green→yellow→orange→red
pub fn heat_color(t: f64) -> (u8, u8, u8) {
    const STOPS: [(f64, u8, u8, u8); 6] = [
        (0.00,  0, 200, 180), // teal
        (0.20, 76, 210, 100), // green
        (0.40, 180, 230,  60), // lime
        (0.60, 255, 220,  40), // yellow
        (0.80, 255, 140,   0), // orange
        (1.00, 240,  60,  50), // red
    ];
    let t = t.clamp(0.0, 1.0);
    let mut i = 0;
    while i < STOPS.len() - 2 && t > STOPS[i + 1].0 {
        i += 1;
    }
    let (t0, r0, g0, b0) = STOPS[i];
    let (t1, r1, g1, b1) = STOPS[i + 1];
    let s = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
    (lerp(r0, r1, s), lerp(g0, g1, s), lerp(b0, b1, s))
}

/// Heat-gradient progress bar with sub-block precision.
pub fn generate_bar(percentage: f64, width: usize, _style: &str, colored: bool) -> String {
    let clamped = percentage.clamp(0.0, 100.0);
    let fill_exact = (clamped / 100.0) * width as f64;
    let filled = fill_exact.floor() as usize;
    let fraction = fill_exact - filled as f64;

    const PARTIALS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];

    if !colored {
        let pi = (fraction * 8.0).round() as usize;
        let has_p = pi > 0 && filled < width;
        let empty = width - filled - if has_p { 1 } else { 0 };
        let p = if has_p {
            PARTIALS[pi.min(7)].to_string()
        } else {
            String::new()
        };
        return format!("▕{}{}{}▏", "█".repeat(filled), p, "░".repeat(empty));
    }

    let bracket = "\x1b[38;2;70;70;70m";
    let dim = "\x1b[38;2;40;40;40m";
    let mut bar = format!("{}▕", bracket);

    for i in 0..filled {
        let pos = (i as f64 + 0.5) / width as f64;
        let (r, g, b) = heat_color(pos);
        bar.push_str(&format!("\x1b[38;2;{};{};{}m█", r, g, b));
    }

    let pi = (fraction * 8.0).round() as usize;
    let has_p = pi > 0 && filled < width;
    if has_p {
        let pos = filled as f64 / width as f64;
        let (r, g, b) = heat_color(pos);
        bar.push_str(&format!(
            "\x1b[38;2;{};{};{}m{}",
            r,
            g,
            b,
            PARTIALS[pi.min(7)]
        ));
    }

    let empty = width - filled - if has_p { 1 } else { 0 };
    if empty > 0 {
        bar.push_str(&format!("{}{}", dim, "░".repeat(empty)));
    }

    bar.push_str(&format!("{}▏", bracket));
    bar
}

/// Format remaining seconds as a styled countdown with pulsing dot.
pub fn format_countdown(secs: Option<i64>) -> String {
    match secs {
        Some(s) if s > 0 => {
            let text = if s >= 86400 {
                let d = s / 86400;
                let h = (s % 86400) / 3600;
                if h > 0 {
                    format!("{}d {}h", d, h)
                } else {
                    format!("{}d", d)
                }
            } else if s >= 3600 {
                let h = s / 3600;
                let m = (s % 3600) / 60;
                if m > 0 {
                    format!("{}h {}m", h, m)
                } else {
                    format!("{}h", h)
                }
            } else {
                format!("{}m", (s / 60).max(1))
            };
            let frame = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() % 2)
                .unwrap_or(0);
            let dot_color = if frame == 0 {
                "0;220;180"
            } else {
                "0;140;120"
            };
            format!(
                " \x1b[38;2;{}m◆\x1b[38;2;100;100;100m {}",
                dot_color, text
            )
        }
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Settings / env helpers
// ---------------------------------------------------------------------------

pub fn read_settings_json() -> Option<serde_json::Value> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    let path = format!("{}/.claude/settings.json", home);
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn get_setting_env(settings: &serde_json::Value, key: &str) -> Option<String> {
    settings
        .get("env")?
        .get(key)?
        .as_str()
        .map(|s| s.to_string())
}

pub fn get_proxy_from_settings() -> Option<String> {
    let s = read_settings_json()?;
    get_setting_env(&s, "HTTPS_PROXY").or_else(|| get_setting_env(&s, "HTTP_PROXY"))
}

pub fn build_http_agent() -> ureq::Agent {
    if let Some(proxy_url) = get_proxy_from_settings() {
        if let Ok(proxy) = ureq::Proxy::new(&proxy_url) {
            return ureq::Agent::config_builder()
                .proxy(Some(proxy))
                .build()
                .new_agent();
        }
    }
    ureq::Agent::new_with_defaults()
}

pub fn timeout_cfg(secs: u64) -> std::time::Duration {
    std::time::Duration::from_secs(secs)
}

// ---------------------------------------------------------------------------
// Cache helpers
// ---------------------------------------------------------------------------

pub fn cache_dir() -> Option<std::path::PathBuf> {
    Some(dirs::home_dir()?.join(".claude").join("ccline"))
}

pub fn is_timestamp_valid(ts: &str, ttl: u64) -> bool {
    if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
        let elapsed = Utc::now().signed_duration_since(dt.with_timezone(&Utc));
        elapsed.num_seconds() < ttl as i64
    } else {
        false
    }
}

pub fn elapsed_secs(ts: &str) -> i64 {
    DateTime::parse_from_rfc3339(ts)
        .map(|dt| {
            Utc::now()
                .signed_duration_since(dt.with_timezone(&Utc))
                .num_seconds()
        })
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Shared rendering: UsageData + BarOpts → SegmentData
// ---------------------------------------------------------------------------

pub fn render_usage_output(data: &UsageData, bar_opts: &UsageBarOpts) -> SegmentData {
    let first_bar = generate_bar(
        data.first_pct,
        bar_opts.bar_width,
        &bar_opts.bar_style,
        bar_opts.bar_colored,
    );
    let second_bar = generate_bar(
        data.second_pct,
        bar_opts.bar_width,
        &bar_opts.bar_style,
        bar_opts.bar_colored,
    );
    let first_cd = format_countdown(data.first_resets_in);
    let second_cd = format_countdown(data.second_resets_in);

    let primary = format!(
        "{} {} {}%{}  {} {} {}%{}",
        data.first_label,
        first_bar,
        data.first_pct.round() as u8,
        first_cd,
        data.second_label,
        second_bar,
        data.second_pct.round() as u8,
        second_cd,
    );

    let mut metadata = HashMap::new();
    metadata.insert("dynamic_icon".to_string(), String::new());
    metadata.insert("block_display".to_string(), "true".to_string());
    metadata.insert(
        "five_hour_utilization".to_string(),
        data.first_pct.to_string(),
    );
    metadata.insert(
        "seven_day_utilization".to_string(),
        data.second_pct.to_string(),
    );

    SegmentData {
        primary,
        secondary: String::new(),
        metadata,
    }
}
