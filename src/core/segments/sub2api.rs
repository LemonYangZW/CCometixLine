use super::usage_common::{
    self, UsageBarOpts, UsageData,
};
use super::{Segment, SegmentData};
use crate::config::{InputData, SegmentConfig, SegmentId};
use chrono::Utc;
use serde::{Deserialize, Serialize};

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
// Sub2ApiSegment
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Sub2ApiSegment;

impl Sub2ApiSegment {
    pub fn new() -> Self {
        Self
    }

    // ---- Cache paths ----

    fn auth_cache_path() -> Option<std::path::PathBuf> {
        Some(usage_common::cache_dir()?.join(".sub2api_auth.json"))
    }

    fn usage_cache_path() -> Option<std::path::PathBuf> {
        Some(usage_common::cache_dir()?.join(".api_usage_cache.json"))
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
        usage_common::is_timestamp_valid(&cache.cached_at, ttl)
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

    // ================================================================
    //  MAIN DISPATCHER
    // ================================================================

    fn get_usage_data(&self, opts: &Sub2ApiOpts) -> Option<UsageData> {
        // Check usage cache (recompute remaining_seconds from elapsed time)
        if let Some(cached) = Self::load_usage_cache() {
            if usage_common::is_timestamp_valid(&cached.cached_at, opts.cache_duration) {
                let elapsed = usage_common::elapsed_secs(&cached.cached_at);
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

        // Source 1: Sub2API Admin (5H) + Gateway fallback (7D if missing)
        if let Some(mut data) = self.try_sub2api_admin(opts) {
            if data.second_pct == 0.0 {
                if let Some(gw) = self.try_sub2api_gateway(opts) {
                    data.second_pct = gw.second_pct;
                    data.second_label = gw.second_label;
                    data.second_resets_in = gw.second_resets_in;
                }
            }
            return Some(data);
        }

        // Source 2: Sub2API Gateway  GET /v1/usage (API key auth)
        if let Some(data) = self.try_sub2api_gateway(opts) {
            return Some(data);
        }

        // Last resort: stale cache
        Self::load_usage_cache().map(|c| {
            let elapsed = usage_common::elapsed_secs(&c.cached_at);
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

    // ---- Sub2API Admin (full automated chain) ----

    fn try_sub2api_admin(&self, opts: &Sub2ApiOpts) -> Option<UsageData> {
        let email = opts.admin_email.as_ref()?;
        let password = opts.admin_password.as_ref()?;
        let base = opts.base_url.as_ref()?;
        let api_key_str = opts.api_key.as_ref()?;

        let (jwt, api_key_id) = self.ensure_auth(base, email, password, api_key_str, opts)?;
        let account_id = self.fetch_latest_account_id(base, &jwt, api_key_id, opts.timeout)?;
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

    fn ensure_auth(
        &self,
        base: &str,
        email: &str,
        password: &str,
        api_key_str: &str,
        opts: &Sub2ApiOpts,
    ) -> Option<(String, i64)> {
        if let Some(cache) = Self::load_auth_cache() {
            if Self::is_auth_cache_valid(&cache, opts.auth_cache_duration) {
                if let Some(kid) = cache.api_key_id {
                    return Some((cache.access_token, kid));
                }
            }
        }

        let jwt = self.login(base, email, password, opts.timeout)?;
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
        let agent = usage_common::build_http_agent();
        let body = LoginRequest {
            email: email.to_string(),
            password: password.to_string(),
        };
        let json = serde_json::to_string(&body).ok()?;

        let resp = agent
            .post(&url)
            .header("Content-Type", "application/json")
            .config()
            .timeout_global(Some(usage_common::timeout_cfg(timeout)))
            .build()
            .send(json.as_bytes())
            .ok()?;

        let wrapper: ApiWrapper<LoginData> = resp.into_body().read_json().ok()?;
        wrapper.data.map(|d| d.access_token)
    }

    /// Admin: search all keys → get user keys → match key string → api_key_id
    fn resolve_api_key_id(
        &self,
        base: &str,
        jwt: &str,
        target_key: &str,
        timeout: u64,
    ) -> Option<i64> {
        let base = base.trim_end_matches('/');
        let agent = usage_common::build_http_agent();
        let dur = usage_common::timeout_cfg(timeout);
        let auth = format!("Bearer {}", jwt);

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

        let mut user_ids: Vec<i64> = search_items.iter().map(|k| k.user_id).collect();
        user_ids.sort_unstable();
        user_ids.dedup();

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
        let agent = usage_common::build_http_agent();

        let resp = agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", jwt))
            .config()
            .timeout_global(Some(usage_common::timeout_cfg(timeout)))
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
        let agent = usage_common::build_http_agent();

        let resp = agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", jwt))
            .config()
            .timeout_global(Some(usage_common::timeout_cfg(timeout)))
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
        if let Ok(tz) = std::env::var("TZ") {
            if !tz.is_empty() {
                return tz;
            }
        }
        let offset_secs = chrono::Local::now().offset().local_minus_utc();
        match offset_secs {
            28800 => "Asia/Shanghai".into(),
            32400 => "Asia/Tokyo".into(),
            -18000 => "America/New_York".into(),
            -28800 => "America/Los_Angeles".into(),
            0 => "UTC".into(),
            _ => format!("Etc/GMT{:+}", -(offset_secs / 3600)),
        }
    }

    // ---- Sub2API Gateway ----

    fn try_sub2api_gateway(&self, opts: &Sub2ApiOpts) -> Option<UsageData> {
        let base = opts.base_url.as_ref()?;
        let api_key = opts.api_key.as_ref()?;

        let url = format!("{}/v1/usage", base.trim_end_matches('/'));
        let agent = usage_common::build_http_agent();

        let resp = agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", api_key))
            .config()
            .timeout_global(Some(usage_common::timeout_cfg(opts.timeout)))
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
}

// ---------------------------------------------------------------------------
// Config options helper
// ---------------------------------------------------------------------------

struct Sub2ApiOpts {
    base_url: Option<String>,
    api_key: Option<String>,
    admin_email: Option<String>,
    admin_password: Option<String>,
    cache_duration: u64,
    auth_cache_duration: u64,
    timeout: u64,
    bar_width: usize,
    bar_style: String,
    bar_colored: bool,
}

impl Sub2ApiOpts {
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

        let settings = usage_common::read_settings_json();
        let setting_env = |key: &str| -> Option<String> {
            settings
                .as_ref()
                .and_then(|s| usage_common::get_setting_env(s, key))
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

impl Segment for Sub2ApiSegment {
    fn collect(&self, _input: &InputData) -> Option<SegmentData> {
        let config = crate::config::Config::load().ok()?;
        let segment_config = config.segments.iter().find(|s| s.id == SegmentId::Sub2Api);
        let opts = Sub2ApiOpts::from_config(segment_config);

        let data = self.get_usage_data(&opts)?;

        let bar_opts = UsageBarOpts {
            bar_width: opts.bar_width,
            bar_style: opts.bar_style,
            bar_colored: opts.bar_colored,
        };

        Some(usage_common::render_usage_output(&data, &bar_opts))
    }

    fn id(&self) -> SegmentId {
        SegmentId::Sub2Api
    }
}
