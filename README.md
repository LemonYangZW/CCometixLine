# CCometixLine (Fork)

[English](README.md) | [中文](README.zh.md)

A high-performance Claude Code statusline tool written in Rust. This fork adds **Sub2API enterprise usage tracking** with heat-gradient progress bars, automated admin API chain, and TUI options editor.

> Forked from [Haleclipse/CCometixLine](https://github.com/Haleclipse/CCometixLine)

![Language:Rust](https://img.shields.io/static/v1?label=Language&message=Rust&color=orange&style=flat-square)
![License:MIT](https://img.shields.io/static/v1?label=License&message=MIT&color=blue&style=flat-square)

## What's New in This Fork

### Sub2API Usage Tracking (Dedicated Segment)
- **Independent Sub2Api segment** — separated from Usage, directly visible in TUI configurator
- **5H / 7D progress bars** with real-time Anthropic account utilization
- **Heat-gradient rendering**: teal -> green -> lime -> yellow -> orange -> red
- **Sub-block precision**: Unicode partial blocks (160 discrete levels at width=20)
- **Reset countdown**: pulsing diamond indicator with time-to-reset
- **Data sources** (priority order):
  1. Sub2API Admin API (login -> key resolve -> usage log -> account usage)
  2. Sub2API Gateway `/v1/usage`

### Usage Segment (Native)
- **Claude Code stdin** `rate_limits` (5H / 7D utilization from stdin)
- **Anthropic OAuth** token-based usage query

### Auto Admin Chain
- **Zero-config account detection**: Automatically discovers which upstream Claude account is in use via the latest usage log record
- **JWT + API key caching**: Auth cache (1h TTL) and usage cache (60s TTL) minimize API calls
- **Admin key resolution**: Searches all users' keys via admin endpoints to match `ANTHROPIC_AUTH_TOKEN`

### TUI Options Editor
- Configure Sub2API credentials, bar style, cache durations directly in `ccline -c`
- Sub2Api segment has its own dedicated Options panel — no longer hidden inside Usage
- Schema-driven modal popup with Text / Password / Number field types

## Screenshots

```
🤖 Opus 4.6 | 📁 CCometixLine | 🌿 master ● | ⚡️ 10.3% · 103.5k | 💰 $17.95 | 🎯 nekomata-engineer
5H ▕████████████▋░░░░░░░▏ 63% ◆ 27m  7D ▕█▊░░░░░░░░░░░░░░░░░░▏ 9% ◆ 6d 14h
```

## Installation

### Quick Install (Recommended)

```bash
# Install globally
npm install -g @nekoline/ccline

# Or using yarn/pnpm
yarn global add @nekoline/ccline
pnpm add -g @nekoline/ccline
```

### Claude Code Configuration

Add to your Claude Code `settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/.claude/ccline/ccline",
    "padding": 0
  }
}
```

> **Windows:** Use `~/.claude/ccline/ccline` (Unix-style path works on Claude Code v2.1.47+).

### Sub2API Usage Configuration

Run `ccline -c`, select the **Sub2Api** segment in the left panel, then navigate to **Options** to configure:

| Field | Description |
|-------|-------------|
| Admin Email | Sub2API admin login email |
| Admin Password | Sub2API admin login password |
| API Base URL | Your Sub2API gateway URL (auto-detected from `ANTHROPIC_BASE_URL`) |
| Bar Style | `heat` (gradient) / `block` (classic) |
| Bar Colored | `true` / `false` (ANSI RGB colors) |
| Bar Width | Progress bar width in chars (default: 20) |
| Cache Duration | Usage data refresh interval in seconds (default: 60) |
| Auth Cache Duration | JWT token cache TTL in seconds (default: 3600) |
| Timeout | HTTP request timeout in seconds (default: 5) |

Or edit `~/.claude/ccline/config.toml` directly (under the Sub2Api segment):

```toml
[[segments]]
id = "sub2_api"
enabled = true

[segments.options]
admin_email = "admin@sub2api.local"
admin_password = "your-password"
api_base_url = "https://your-sub2api.com"
bar_style = "heat"
bar_colored = "true"
bar_width = 20
cache_duration = 60
auth_cache_duration = 3600
timeout = 5
```

### Build from Source

```bash
git clone https://github.com/LemonYangZW/CCometixLine.git
cd CCometixLine
cargo build --release

# Linux/macOS
mkdir -p ~/.claude/ccline
cp target/release/ccometixline ~/.claude/ccline/ccline
chmod +x ~/.claude/ccline/ccline

# Windows (PowerShell)
New-Item -ItemType Directory -Force -Path "$env:USERPROFILE\.claude\ccline"
copy target\release\ccometixline.exe "$env:USERPROFILE\.claude\ccline\ccline.exe"
```

## Features (Inherited)

- **Git integration** with branch, status, and tracking info
- **Model display** with simplified Claude model names
- **Context window** token usage tracking
- **Cost tracking** per session
- **Interactive TUI** with real-time preview and theme system
- **Theme presets**: cometix, default, minimal, gruvbox, nord, powerline-dark/light
- **Claude Code patcher**: Disable context warnings, enable verbose mode

## Configuration

- **Config file**: `~/.claude/ccline/config.toml`
- **Interactive TUI**: `ccline -c`
- **Theme files**: `~/.claude/ccline/themes/*.toml`
- **Model config**: `~/.claude/ccline/models.toml`

## Requirements

- **Git**: 1.5+ (2.22+ recommended)
- **Terminal**: Nerd Font support for icons ([nerdfonts.com](https://www.nerdfonts.com/))
- **Claude Code**: For statusline integration

## License

[MIT License](LICENSE)

## Credits

- Original project: [Haleclipse/CCometixLine](https://github.com/Haleclipse/CCometixLine)
- Sub2API: [Wei-Shaw/sub2api](https://github.com/Wei-Shaw/sub2api)
