use super::{Segment, SegmentData};
use crate::config::{InputData, SegmentConfig, SegmentId};
use crate::utils::credentials;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Internal result
// ---------------------------------------------------------------------------

struct UsageData {
    first_pct: f64,
    first_label: String,
    first_resets_in: Option<i64>, // seconds until window reset
    second_pct: f64,
    second_label: String,
    second_resets_in: Option<i64>,
}

// ---------------------------------------------------------------------------
// Sub2API Admin response types (wrapped in {"code":0,"data":...})
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ApiWrapper<T> {
    #[allow(dead_code)]
    code: Option<i32>,
    data: Option<T>,
}

// POST /api/v1/auth/login
#[derive(Debug, Serialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct LoginData {
    access_token: String,
    #[allow(dead_code)]
    refresh_token: Option<String>,
    #[allow(dead_code)]
    expires_in: Option<i64>,
}

// GET /api/v1/admin/usage/search-api-keys
#[derive(Debug, Deserialize)]
struct SearchApiKeyItem {
    #[allow(dead_code)]
    id: i64,
    #[allow(dead_code)]
    name: Option<String>,
    user_id: i64,
}

// GET /api/v1/admin/users/:id/api-keys  (paginated)
#[derive(Debug, Deserialize)]
struct PaginatedKeys {
    items: Option<Vec<ApiKeyItem>>,
}

#[derive(Debug, Deserialize)]
struct ApiKeyItem {
    id: i64,
    key: String,
}

// GET /api/v1/admin/usage  (paginated)
#[derive(Debug, Deserialize)]
struct PaginatedUsageLogs {
    items: Option<Vec<UsageLogItem>>,
}

#[derive(Debug, Deserialize)]
struct UsageLogItem {
    account_id: i64,
}

// GET /api/v1/admin/accounts/:id/usage
#[derive(Debug, Deserialize)]
struct AccountUsageInfo {
    five_hour: Option<AccountUsageProgress>,
    seven_day: Option<AccountUsageProgress>,
}

#[derive(Debug, Deserialize)]
struct AccountUsageProgress {
    utilization: f64,
    remaining_seconds: Option<i64>,
}

// ---------------------------------------------------------------------------
// Sub2API Gateway types  (GET /v1/usage, raw JSON, no wrapper)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GatewayUsageResponse {
    #[allow(dead_code)]
    mode: Option<String>,
    rate_limits: Option<Vec<GatewayRateLimit>>,
    subscription: Option<GatewaySubscription>,
}

#[derive(Debug, Deserialize)]
struct GatewayRateLimit {
    window: String,
    limit: f64,
    used: f64,
}

#[derive(Debug, Deserialize)]
struct GatewaySubscription {
    daily_usage_usd: Option<f64>,
    weekly_usage_usd: Option<f64>,
    daily_limit_usd: Option<f64>,
    weekly_limit_usd: Option<f64>,
}

// ---------------------------------------------------------------------------
// Anthropic OAuth types (GET /api/oauth/usage, raw JSON)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OAuthUsageResponse {
    five_hour: OAuthUsagePeriod,
    seven_day: OAuthUsagePeriod,
}

#[derive(Debug, Deserialize)]
struct OAuthUsagePeriod {
    utilization: f64,
}

// ---------------------------------------------------------------------------
// Cache: auth (JWT + api_key_id) — long-lived
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct AuthCache {
    access_token: String,
    api_key_id: Option<i64>,
    cached_at: String,
}

// ---------------------------------------------------------------------------
// Cache: usage (5H/7D + account_id) — short-lived
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct UsageCache {
    first_pct: f64,
    second_pct: f64,
    first_label: String,
    second_label: String,
    #[serde(default)]
    first_resets_in: Option<i64>,
    #[serde(default)]
    second_resets_in: Option<i64>,
    account_id: Option<i64>,
    cached_at: String,
}

// ---------------------------------------------------------------------------
// UsageSegment
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct UsageSegment;

impl UsageSegment {
    pub fn new() -> Self {
        Self
    }

    // ---- Icon ----

    #[allow(dead_code)]
    fn get_circle_icon(ratio: f64) -> String {
        let percent = (ratio * 100.0) as u8;
        match percent {
            0..=12 => "\u{f0a9e}".to_string(),
            13..=25 => "\u{f0a9f}".to_string(),
            26..=37 => "\u{f0aa0}".to_string(),
            38..=50 => "\u{f0aa1}".to_string(),
            51..=62 => "\u{f0aa2}".to_string(),
            63..=75 => "\u{f0aa3}".to_string(),
            76..=87 => "\u{f0aa4}".to_string(),
            _ => "\u{f0aa5}".to_string(),
        }
    }

    // ---- Progress bar ----

    /// Linear interpolation between two u8 values.
    fn lerp(a: u8, b: u8, t: f64) -> u8 {
        (a as f64 + (b as f64 - a as f64) * t).clamp(0.0, 255.0) as u8
    }

    /// Smooth heat-map via multi-stop spline: cyan→green→yellow→orange→red
    fn heat_color(t: f64) -> (u8, u8, u8) {
        // (position, R, G, B)
        const STOPS: [(f64, u8, u8, u8); 6] = [
            (0.00,  0, 200, 180),  // teal
            (0.20, 76, 210, 100),  // green
            (0.40, 180, 230,  60), // lime
            (0.60, 255, 220,  40), // yellow
            (0.80, 255, 140,   0), // orange
            (1.00, 240,  60,  50), // red
        ];
        let t = t.clamp(0.0, 1.0);
        // Find surrounding stops
        let mut i = 0;
        while i < STOPS.len() - 2 && t > STOPS[i + 1].0 {
            i += 1;
        }
        let (t0, r0, g0, b0) = STOPS[i];
        let (t1, r1, g1, b1) = STOPS[i + 1];
        let s = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
        (Self::lerp(r0, r1, s), Self::lerp(g0, g1, s), Self::lerp(b0, b1, s))
    }

    /// Heat-gradient progress bar with sub-block precision.
    ///
    /// Each filled block is colored by its *position* in the bar (green→yellow→
    /// orange→red), creating a speedometer effect. The fill boundary uses
    /// Unicode partial-block characters (▏▎▍▌▋▊▉) for smooth precision.
    fn generate_bar(percentage: f64, width: usize, _style: &str, colored: bool) -> String {
        let clamped = percentage.clamp(0.0, 100.0);
        let fill_exact = (clamped / 100.0) * width as f64;
        let filled = fill_exact.floor() as usize;
        let fraction = fill_exact - filled as f64;

        const PARTIALS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];

        if !colored {
            let pi = (fraction * 8.0).round() as usize;
            let has_p = pi > 0 && filled < width;
            let empty = width - filled - if has_p { 1 } else { 0 };
            let p = if has_p { PARTIALS[pi.min(7)].to_string() } else { String::new() };
            return format!("▕{}{}{}▏", "█".repeat(filled), p, "░".repeat(empty));
        }

        let bracket = "\x1b[38;2;70;70;70m";
        let dim = "\x1b[38;2;40;40;40m";
        let mut bar = format!("{}▕", bracket);

        // Filled blocks with position-based heat gradient
        for i in 0..filled {
            let pos = (i as f64 + 0.5) / width as f64;
            let (r, g, b) = Self::heat_color(pos);
            bar.push_str(&format!("\x1b[38;2;{};{};{}m█", r, g, b));
        }

        // Partial block at boundary
        let pi = (fraction * 8.0).round() as usize;
        let has_p = pi > 0 && filled < width;
        if has_p {
            let pos = filled as f64 / width as f64;
            let (r, g, b) = Self::heat_color(pos);
            bar.push_str(&format!("\x1b[38;2;{};{};{}m{}", r, g, b, PARTIALS[pi.min(7)]));
        }

        // Empty slots
        let empty = width - filled - if has_p { 1 } else { 0 };
        if empty > 0 {
            bar.push_str(&format!("{}{}", dim, "░".repeat(empty)));
        }

        bar.push_str(&format!("{}▏", bracket));
        bar
    }

    /// Format remaining seconds as a styled countdown with pulsing dot.
    fn format_countdown(secs: Option<i64>) -> String {
        match secs {
            Some(s) if s > 0 => {
                let text = if s >= 86400 {
                    let d = s / 86400;
                    let h = (s % 86400) / 3600;
                    if h > 0 { format!("{}d {}h", d, h) } else { format!("{}d", d) }
                } else if s >= 3600 {
                    let h = s / 3600;
                    let m = (s % 3600) / 60;
                    if m > 0 { format!("{}h {}m", h, m) } else { format!("{}h", h) }
                } else {
                    format!("{}m", (s / 60).max(1))
                };
                // Pulsing dot alternates brightness each second
                let frame = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() % 2)
                    .unwrap_or(0);
                let dot_color = if frame == 0 { "0;220;180" } else { "0;140;120" };
                format!(" \x1b[38;2;{}m◆\x1b[38;2;100;100;100m {}", dot_color, text)
            }
            _ => String::new(),
        }
    }

    // ---- Settings / env helpers ----

    fn read_settings_json() -> Option<serde_json::Value> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()?;
        let path = format!("{}/.claude/settings.json", home);
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    fn get_setting_env(settings: &serde_json::Value, key: &str) -> Option<String> {
        settings
            .get("env")?
            .get(key)?
            .as_str()
            .map(|s| s.to_string())
    }

    fn get_proxy_from_settings() -> Option<String> {
        let s = Self::read_settings_json()?;
        Self::get_setting_env(&s, "HTTPS_PROXY")
            .or_else(|| Self::get_setting_env(&s, "HTTP_PROXY"))
    }

    fn build_http_agent() -> ureq::Agent {
        if let Some(proxy_url) = Self::get_proxy_from_settings() {
            if let Ok(proxy) = ureq::Proxy::new(&proxy_url) {
                return ureq::Agent::config_builder()
                    .proxy(Some(proxy))
                    .build()
                    .new_agent();
            }
        }
        ureq::Agent::new_with_defaults()
    }

    fn timeout_cfg(secs: u64) -> std::time::Duration {
        std::time::Duration::from_secs(secs)
    }

    // ---- Cache paths ----

    fn cache_dir() -> Option<std::path::PathBuf> {
        Some(dirs::home_dir()?.join(".claude").join("ccline"))
    }

    fn auth_cache_path() -> Option<std::path::PathBuf> {
        Some(Self::cache_dir()?.join(".sub2api_auth.json"))
    }

    fn usage_cache_path() -> Option<std::path::PathBuf> {
        Some(Self::cache_dir()?.join(".api_usage_cache.json"))
    }

    // ---- Auth cache ----

    fn load_auth_cache() -> Option<AuthCache> {
        let path = Self::auth_cache_path()?;
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    fn save_auth_cache(cache: &AuthCache) {
        if let Some(path) = Self::auth_cache_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(cache) {
                let _ = std::fs::write(path, json);
            }
        }
    }

    fn is_auth_cache_valid(cache: &AuthCache, ttl: u64) -> bool {
        Self::is_timestamp_valid(&cache.cached_at, ttl)
    }

    // ---- Usage cache ----

    fn load_usage_cache() -> Option<UsageCache> {
        let path = Self::usage_cache_path()?;
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    fn save_usage_cache(cache: &UsageCache) {
        if let Some(path) = Self::usage_cache_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(cache) {
                let _ = std::fs::write(path, json);
            }
        }
    }

    fn is_timestamp_valid(ts: &str, ttl: u64) -> bool {
        if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
            let elapsed = Utc::now().signed_duration_since(dt.with_timezone(&Utc));
            elapsed.num_seconds() < ttl as i64
        } else {
            false
        }
    }

    /// Seconds elapsed since a cached timestamp.
    fn elapsed_secs(ts: &str) -> i64 {
        DateTime::parse_from_rfc3339(ts)
            .map(|dt| Utc::now().signed_duration_since(dt.with_timezone(&Utc)).num_seconds())
            .unwrap_or(0)
    }

    // ================================================================
    //  MAIN DISPATCHER
    // ================================================================

    fn get_usage_data(
        &self,
        input: &InputData,
        segment_config: Option<&SegmentConfig>,
    ) -> Option<UsageData> {
        let opts = SegmentOpts::from_config(segment_config);

        // Source 1: InputData.rate_limits from Claude Code stdin
        if let Some(data) = Self::try_input_rate_limits(input) {
            return Some(data);
        }

        // Check usage cache (recompute remaining_seconds from elapsed time)
        if let Some(cached) = Self::load_usage_cache() {
            if Self::is_timestamp_valid(&cached.cached_at, opts.cache_duration) {
                let elapsed = Self::elapsed_secs(&cached.cached_at);
                return Some(UsageData {
                    first_pct: cached.first_pct,
                    first_label: cached.first_label,
                    first_resets_in: cached.first_resets_in.map(|s| (s - elapsed).max(0)),
                    second_pct: cached.second_pct,
                    second_label: cached.second_label,
                    second_resets_in: cached.second_resets_in.map(|s| (s - elapsed).max(0)),
                });
            }
        }

        // Source 2: Sub2API Admin (5H) + Gateway fallback (7D if missing)
        if let Some(mut data) = self.try_sub2api_admin(&opts) {
            // If admin had no 7D data, supplement from gateway
            if data.second_pct == 0.0 {
                if let Some(gw) = self.try_sub2api_gateway(&opts) {
                    data.second_pct = gw.second_pct;
                    data.second_label = gw.second_label;
                    data.second_resets_in = gw.second_resets_in;
                }
            }
            return Some(data);
        }

        // Source 3: Sub2API Gateway  GET /v1/usage (API key auth)
        if let Some(data) = self.try_sub2api_gateway(&opts) {
            return Some(data);
        }

        // Source 4: Anthropic OAuth
        if let Some(data) = self.try_anthropic_oauth(&opts) {
            return Some(data);
        }

        // Last resort: stale cache
        Self::load_usage_cache().map(|c| {
            let elapsed = Self::elapsed_secs(&c.cached_at);
            UsageData {
                first_pct: c.first_pct,
                first_label: c.first_label,
                first_resets_in: c.first_resets_in.map(|s| (s - elapsed).max(0)),
                second_pct: c.second_pct,
                second_label: c.second_label,
                second_resets_in: c.second_resets_in.map(|s| (s - elapsed).max(0)),
            }
        })
    }

    // ---- Source 1: Claude Code stdin ----

    fn try_input_rate_limits(input: &InputData) -> Option<UsageData> {
        let rl = input.rate_limits.as_ref()?;
        let five = rl.five_hour.as_ref().and_then(|p| p.used_percentage);
        let seven = rl.seven_day.as_ref().and_then(|p| p.used_percentage);
        if five.is_some() || seven.is_some() {
            Some(UsageData {
                first_pct: five.unwrap_or(0.0),
                first_label: "5H".into(),
                first_resets_in: None, // stdin doesn't provide reset info
                second_pct: seven.unwrap_or(0.0),
                second_label: "7D".into(),
                second_resets_in: None,
            })
        } else {
            None
        }
    }

    // ---- Source 2: Sub2API Admin (full automated chain) ----

    fn try_sub2api_admin(&self, opts: &SegmentOpts) -> Option<UsageData> {
        let email = opts.admin_email.as_ref()?;
        let password = opts.admin_password.as_ref()?;
        let base = opts.base_url.as_ref()?;
        let api_key_str = opts.api_key.as_ref()?;

        // Step 1+2: Get JWT + resolve api_key_id (from cache or fresh)
        let (jwt, api_key_id) = self.ensure_auth(base, email, password, api_key_str, opts)?;

        // Step 3: Last usage log → account_id
        let account_id = self.fetch_latest_account_id(base, &jwt, api_key_id, opts.timeout)?;

        // Step 4: Account 5H/7D usage
        let data = self.fetch_account_usage(base, &jwt, account_id, opts.timeout)?;

        Self::save_usage_cache(&UsageCache {
            first_pct: data.first_pct,
            second_pct: data.second_pct,
            first_label: data.first_label.clone(),
            second_label: data.second_label.clone(),
            first_resets_in: data.first_resets_in,
            second_resets_in: data.second_resets_in,
            account_id: Some(account_id),
            cached_at: Utc::now().to_rfc3339(),
        });

        Some(data)
    }

    /// Ensure we have a valid JWT + api_key_id. Uses auth cache with long TTL.
    fn ensure_auth(
        &self,
        base: &str,
        email: &str,
        password: &str,
        api_key_str: &str,
        opts: &SegmentOpts,
    ) -> Option<(String, i64)> {
        // Try auth cache
        if let Some(cache) = Self::load_auth_cache() {
            if Self::is_auth_cache_valid(&cache, opts.auth_cache_duration) {
                if let Some(kid) = cache.api_key_id {
                    return Some((cache.access_token, kid));
                }
            }
        }

        // Login
        let jwt = self.login(base, email, password, opts.timeout)?;

        // Resolve api_key_id by matching key string
        let api_key_id = self.resolve_api_key_id(base, &jwt, api_key_str, opts.timeout)?;

        Self::save_auth_cache(&AuthCache {
            access_token: jwt.clone(),
            api_key_id: Some(api_key_id),
            cached_at: Utc::now().to_rfc3339(),
        });

        Some((jwt, api_key_id))
    }

    /// POST /api/v1/auth/login → JWT
    fn login(&self, base: &str, email: &str, password: &str, timeout: u64) -> Option<String> {
        let url = format!("{}/api/v1/auth/login", base.trim_end_matches('/'));
        let agent = Self::build_http_agent();
        let body = LoginRequest {
            email: email.to_string(),
            password: password.to_string(),
        };
        let json = serde_json::to_string(&body).ok()?;

        let resp = agent
            .post(&url)
            .header("Content-Type", "application/json")
            .config()
            .timeout_global(Some(Self::timeout_cfg(timeout)))
            .build()
            .send(json.as_bytes())
            .ok()?;

        let wrapper: ApiWrapper<LoginData> = resp.into_body().read_json().ok()?;
        wrapper.data.map(|d| d.access_token)
    }

    /// Admin: search all keys → get user keys → match key string → api_key_id
    ///
    /// 1. GET /api/v1/admin/usage/search-api-keys   → [{id, name, user_id}, ...]
    /// 2. GET /api/v1/admin/users/:uid/api-keys      → full key strings
    /// 3. Match target_key to find api_key_id
    fn resolve_api_key_id(
        &self,
        base: &str,
        jwt: &str,
        target_key: &str,
        timeout: u64,
    ) -> Option<i64> {
        let base = base.trim_end_matches('/');
        let agent = Self::build_http_agent();
        let dur = Self::timeout_cfg(timeout);
        let auth = format!("Bearer {}", jwt);

        // Step 1: Get all key summaries (id, user_id) via admin search
        let search_url = format!("{}/api/v1/admin/usage/search-api-keys", base);
        let resp = agent
            .get(&search_url)
            .header("Authorization", &auth)
            .config()
            .timeout_global(Some(dur))
            .build()
            .call()
            .ok()?;

        let search_items: Vec<SearchApiKeyItem> = {
            let wrapper: ApiWrapper<Vec<SearchApiKeyItem>> =
                resp.into_body().read_json().ok()?;
            wrapper.data?
        };

        // Collect unique user_ids
        let mut user_ids: Vec<i64> = search_items.iter().map(|k| k.user_id).collect();
        user_ids.sort_unstable();
        user_ids.dedup();

        // Step 2: For each user, fetch full keys and try to match
        for uid in user_ids {
            let keys_url = format!("{}/api/v1/admin/users/{}/api-keys", base, uid);
            let resp = agent
                .get(&keys_url)
                .header("Authorization", &auth)
                .config()
                .timeout_global(Some(dur))
                .build()
                .call();
            if let Ok(r) = resp {
                let wrapper: Result<ApiWrapper<PaginatedKeys>, _> =
                    r.into_body().read_json();
                if let Ok(w) = wrapper {
                    if let Some(items) = w.data.and_then(|d| d.items) {
                        if let Some(found) = items.iter().find(|k| k.key == target_key) {
                            return Some(found.id);
                        }
                    }
                }
            }
        }

        None
    }

    /// GET /api/v1/admin/usage?api_key_id={id}&page_size=1 → account_id
    fn fetch_latest_account_id(
        &self,
        base: &str,
        jwt: &str,
        api_key_id: i64,
        timeout: u64,
    ) -> Option<i64> {
        let url = format!(
            "{}/api/v1/admin/usage?api_key_id={}&page_size=1",
            base.trim_end_matches('/'),
            api_key_id
        );
        let agent = Self::build_http_agent();

        let resp = agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", jwt))
            .config()
            .timeout_global(Some(Self::timeout_cfg(timeout)))
            .build()
            .call()
            .ok()?;

        let wrapper: ApiWrapper<PaginatedUsageLogs> = resp.into_body().read_json().ok()?;
        let items = wrapper.data?.items?;
        items.first().map(|log| log.account_id)
    }

    /// GET /api/v1/admin/accounts/{id}/usage?source=passive&timezone=... → 5H/7D
    fn fetch_account_usage(
        &self,
        base: &str,
        jwt: &str,
        account_id: i64,
        timeout: u64,
    ) -> Option<UsageData> {
        let url = format!(
            "{}/api/v1/admin/accounts/{}/usage?source=passive&timezone={}",
            base.trim_end_matches('/'),
            account_id,
            Self::local_timezone_iana(),
        );
        let agent = Self::build_http_agent();

        let resp = agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", jwt))
            .config()
            .timeout_global(Some(Self::timeout_cfg(timeout)))
            .build()
            .call()
            .ok()?;

        let wrapper: ApiWrapper<AccountUsageInfo> = resp.into_body().read_json().ok()?;
        let info = wrapper.data?;

        let five_pct = info.five_hour.as_ref().map(|p| p.utilization).unwrap_or(0.0);
        let five_reset = info.five_hour.as_ref().and_then(|p| p.remaining_seconds);
        let seven_pct = info.seven_day.as_ref().map(|p| p.utilization).unwrap_or(0.0);
        let seven_reset = info.seven_day.as_ref().and_then(|p| p.remaining_seconds);

        if five_pct > 0.0 || seven_pct > 0.0 {
            Some(UsageData {
                first_pct: five_pct,
                first_label: "5H".into(),
                first_resets_in: five_reset,
                second_pct: seven_pct,
                second_label: "7D".into(),
                second_resets_in: seven_reset,
            })
        } else {
            None
        }
    }

    /// Best-effort local IANA timezone (e.g. "Asia/Shanghai").
    fn local_timezone_iana() -> String {
        // Try TZ env first
        if let Ok(tz) = std::env::var("TZ") {
            if !tz.is_empty() {
                return tz;
            }
        }
        // Fallback: use chrono Local offset to guess common zones
        let offset_secs = chrono::Local::now().offset().local_minus_utc();
        match offset_secs {
            28800 => "Asia/Shanghai".into(),   // UTC+8
            32400 => "Asia/Tokyo".into(),      // UTC+9
            -18000 => "America/New_York".into(), // UTC-5
            -28800 => "America/Los_Angeles".into(), // UTC-8
            0 => "UTC".into(),
            _ => format!("Etc/GMT{:+}", -(offset_secs / 3600)),
        }
    }

    // ---- Source 3: Sub2API Gateway ----

    fn try_sub2api_gateway(&self, opts: &SegmentOpts) -> Option<UsageData> {
        let base = opts.base_url.as_ref()?;
        let api_key = opts.api_key.as_ref()?;

        let url = format!("{}/v1/usage", base.trim_end_matches('/'));
        let agent = Self::build_http_agent();

        let resp = agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", api_key))
            .config()
            .timeout_global(Some(Self::timeout_cfg(opts.timeout)))
            .build()
            .call()
            .ok()?;

        let body: GatewayUsageResponse = resp.into_body().read_json().ok()?;
        let data = Self::parse_gateway_response(&body)?;

        Self::save_usage_cache(&UsageCache {
            first_pct: data.first_pct,
            second_pct: data.second_pct,
            first_label: data.first_label.clone(),
            second_label: data.second_label.clone(),
            first_resets_in: data.first_resets_in,
            second_resets_in: data.second_resets_in,
            account_id: None,
            cached_at: Utc::now().to_rfc3339(),
        });

        Some(data)
    }

    fn parse_gateway_response(resp: &GatewayUsageResponse) -> Option<UsageData> {
        // Try rate_limits array
        if let Some(limits) = &resp.rate_limits {
            let five_h = limits.iter().find(|r| r.window == "5h");
            let one_d = limits.iter().find(|r| r.window == "1d");
            let seven_d = limits.iter().find(|r| r.window == "7d");

            let first = five_h.or(one_d);
            if first.is_some() || seven_d.is_some() {
                return Some(UsageData {
                    first_pct: first.map(|r| Self::pct(r.used, r.limit)).unwrap_or(0.0),
                    first_label: if five_h.is_some() { "5H" } else { "1D" }.into(),
                    first_resets_in: None,
                    second_pct: seven_d.map(|r| Self::pct(r.used, r.limit)).unwrap_or(0.0),
                    second_label: "7D".into(),
                    second_resets_in: None,
                });
            }
        }
        // Try subscription
        if let Some(sub) = &resp.subscription {
            let d = Self::opt_pct(sub.daily_usage_usd, sub.daily_limit_usd);
            let w = Self::opt_pct(sub.weekly_usage_usd, sub.weekly_limit_usd);
            if d.is_some() || w.is_some() {
                return Some(UsageData {
                    first_pct: d.unwrap_or(0.0),
                    first_label: "1D".into(),
                    first_resets_in: None,
                    second_pct: w.unwrap_or(0.0),
                    second_label: "1W".into(),
                    second_resets_in: None,
                });
            }
        }
        None
    }

    fn pct(used: f64, limit: f64) -> f64 {
        if limit > 0.0 {
            (used / limit) * 100.0
        } else {
            0.0
        }
    }

    fn opt_pct(used: Option<f64>, limit: Option<f64>) -> Option<f64> {
        match (used, limit) {
            (Some(u), Some(l)) if l > 0.0 => Some((u / l) * 100.0),
            _ => None,
        }
    }

    // ---- Source 4: Anthropic OAuth ----

    fn try_anthropic_oauth(&self, opts: &SegmentOpts) -> Option<UsageData> {
        let token = credentials::get_oauth_token()?;
        let base = opts
            .base_url
            .as_deref()
            .unwrap_or("https://api.anthropic.com");
        let url = format!("{}/api/oauth/usage", base.trim_end_matches('/'));
        let agent = Self::build_http_agent();

        let resp = agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", token))
            .header("anthropic-beta", "oauth-2025-04-20")
            .config()
            .timeout_global(Some(Self::timeout_cfg(opts.timeout)))
            .build()
            .call()
            .ok()?;

        let body: OAuthUsageResponse = resp.into_body().read_json().ok()?;
        let data = UsageData {
            first_pct: body.five_hour.utilization,
            first_label: "5H".into(),
            first_resets_in: None,
            second_pct: body.seven_day.utilization,
            second_label: "7D".into(),
            second_resets_in: None,
        };

        Self::save_usage_cache(&UsageCache {
            first_pct: data.first_pct,
            second_pct: data.second_pct,
            first_label: data.first_label.clone(),
            second_label: data.second_label.clone(),
            first_resets_in: data.first_resets_in,
            second_resets_in: data.second_resets_in,
            account_id: None,
            cached_at: Utc::now().to_rfc3339(),
        });

        Some(data)
    }
}

// ---------------------------------------------------------------------------
// Config options helper
// ---------------------------------------------------------------------------

struct SegmentOpts {
    base_url: Option<String>,
    api_key: Option<String>,
    admin_email: Option<String>,
    admin_password: Option<String>,
    cache_duration: u64,      // 5H/7D data TTL (seconds)
    auth_cache_duration: u64, // JWT + api_key_id TTL (seconds)
    timeout: u64,
    bar_width: usize,
    bar_style: String,  // "block" | "cat" | "neko" | "fish" | "paw"
    bar_colored: bool,  // embed ANSI RGB colors in bar
}

impl SegmentOpts {
    fn from_config(sc: Option<&SegmentConfig>) -> Self {
        let opt = |key: &str| -> Option<String> {
            sc.and_then(|c| c.options.get(key))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        let opt_u64 = |key: &str, default: u64| -> u64 {
            sc.and_then(|c| c.options.get(key))
                .and_then(|v| v.as_u64())
                .unwrap_or(default)
        };

        // Resolve credentials from options > settings.json env > env var
        let settings = UsageSegment::read_settings_json();
        let setting_env = |key: &str| -> Option<String> {
            settings
                .as_ref()
                .and_then(|s| UsageSegment::get_setting_env(s, key))
        };
        let env_or = |key: &str| -> Option<String> { std::env::var(key).ok() };

        Self {
            base_url: opt("api_base_url")
                .or_else(|| setting_env("ANTHROPIC_BASE_URL"))
                .or_else(|| env_or("ANTHROPIC_BASE_URL")),
            api_key: opt("api_key")
                .or_else(|| setting_env("ANTHROPIC_AUTH_TOKEN"))
                .or_else(|| env_or("ANTHROPIC_AUTH_TOKEN")),
            admin_email: opt("admin_email"),
            admin_password: opt("admin_password"),
            cache_duration: opt_u64("cache_duration", 60),
            auth_cache_duration: opt_u64("auth_cache_duration", 3600),
            timeout: opt_u64("timeout", 5),
            bar_width: opt_u64("bar_width", 20) as usize,
            bar_style: opt("bar_style").unwrap_or_else(|| "cat".to_string()),
            bar_colored: sc
                .and_then(|c| c.options.get("bar_colored"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
        }
    }
}

// ---------------------------------------------------------------------------
// Segment trait
// ---------------------------------------------------------------------------

impl Segment for UsageSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let config = crate::config::Config::load().ok()?;
        let segment_config = config.segments.iter().find(|s| s.id == SegmentId::Usage);
        let opts = SegmentOpts::from_config(segment_config);

        let data = self.get_usage_data(input, segment_config)?;

        let first_bar =
            Self::generate_bar(data.first_pct, opts.bar_width, &opts.bar_style, opts.bar_colored);
        let second_bar = Self::generate_bar(
            data.second_pct,
            opts.bar_width,
            &opts.bar_style,
            opts.bar_colored,
        );
        let first_cd = Self::format_countdown(data.first_resets_in);
        let second_cd = Self::format_countdown(data.second_resets_in);

        // Single line with both bars + countdowns, rendered as block (own line)
        let primary = format!(
            "{} {} {}%{}  {} {} {}%{}",
            data.first_label, first_bar, data.first_pct.round() as u8, first_cd,
            data.second_label, second_bar, data.second_pct.round() as u8, second_cd,
        );

        let mut metadata = HashMap::new();
        // Empty icon — block segment renders on its own line without icon
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

        Some(SegmentData {
            primary,
            secondary: String::new(),
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::Usage
    }
}
