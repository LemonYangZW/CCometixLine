use super::usage_common::{self, UsageBarOpts, UsageData};
use super::{Segment, SegmentData};
use crate::config::{InputData, SegmentConfig, SegmentId};
use crate::utils::credentials;
use serde::Deserialize;

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
// UsageSegment — stdin rate_limits + Anthropic OAuth
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct UsageSegment;

impl UsageSegment {
    pub fn new() -> Self {
        Self
    }

    fn get_usage_data(&self, input: &InputData, opts: &UsageOpts) -> Option<UsageData> {
        // Source 1: InputData.rate_limits from Claude Code stdin
        if let Some(data) = Self::try_input_rate_limits(input) {
            return Some(data);
        }

        // Source 2: Anthropic OAuth
        if let Some(data) = self.try_anthropic_oauth(opts) {
            return Some(data);
        }

        None
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
                first_resets_in: None,
                second_pct: seven.unwrap_or(0.0),
                second_label: "7D".into(),
                second_resets_in: None,
            })
        } else {
            None
        }
    }

    // ---- Source 2: Anthropic OAuth ----

    fn try_anthropic_oauth(&self, opts: &UsageOpts) -> Option<UsageData> {
        let token = credentials::get_oauth_token()?;
        let base = opts
            .base_url
            .as_deref()
            .unwrap_or("https://api.anthropic.com");
        let url = format!("{}/api/oauth/usage", base.trim_end_matches('/'));
        let agent = usage_common::build_http_agent();

        let resp = agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", token))
            .header("anthropic-beta", "oauth-2025-04-20")
            .config()
            .timeout_global(Some(usage_common::timeout_cfg(opts.timeout)))
            .build()
            .call()
            .ok()?;

        let body: OAuthUsageResponse = resp.into_body().read_json().ok()?;
        Some(UsageData {
            first_pct: body.five_hour.utilization,
            first_label: "5H".into(),
            first_resets_in: None,
            second_pct: body.seven_day.utilization,
            second_label: "7D".into(),
            second_resets_in: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Config options helper (simplified — no Sub2API fields)
// ---------------------------------------------------------------------------

struct UsageOpts {
    base_url: Option<String>,
    timeout: u64,
    bar_width: usize,
    bar_style: String,
    bar_colored: bool,
}

impl UsageOpts {
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
        let opts = UsageOpts::from_config(segment_config);

        let data = self.get_usage_data(input, &opts)?;

        let bar_opts = UsageBarOpts {
            bar_width: opts.bar_width,
            bar_style: opts.bar_style,
            bar_colored: opts.bar_colored,
        };

        Some(usage_common::render_usage_output(&data, &bar_opts))
    }

    fn id(&self) -> SegmentId {
        SegmentId::Usage
    }
}
