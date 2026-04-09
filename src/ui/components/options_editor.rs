use crate::config::SegmentId;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Field definition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum FieldType {
    Text,
    Password,
    Number,
}

#[derive(Debug, Clone)]
pub struct OptionField {
    pub key: String,
    pub label: String,
    pub value: String,
    pub field_type: FieldType,
    pub description: String,
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

pub struct OptionsEditorComponent {
    pub is_open: bool,
    pub fields: Vec<OptionField>,
    pub selected: usize,
    pub editing: bool,
    pub edit_buffer: String,
    segment_id: Option<SegmentId>,
}

impl Default for OptionsEditorComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl OptionsEditorComponent {
    pub fn new() -> Self {
        Self {
            is_open: false,
            fields: Vec::new(),
            selected: 0,
            editing: false,
            edit_buffer: String::new(),
            segment_id: None,
        }
    }

    /// Open editor for a specific segment's options
    pub fn open(&mut self, segment_id: SegmentId, options: &HashMap<String, serde_json::Value>) {
        self.is_open = true;
        self.selected = 0;
        self.editing = false;
        self.edit_buffer.clear();
        self.segment_id = Some(segment_id);
        self.fields = Self::build_fields(segment_id, options);
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.editing = false;
    }

    /// Return modified options as HashMap for saving back to config
    pub fn get_options(&self) -> HashMap<String, serde_json::Value> {
        let mut opts = HashMap::new();
        for f in &self.fields {
            if f.value.is_empty() {
                continue;
            }
            let val = match f.field_type {
                FieldType::Number => {
                    if let Ok(n) = f.value.parse::<u64>() {
                        serde_json::Value::Number(n.into())
                    } else if let Ok(n) = f.value.parse::<f64>() {
                        serde_json::json!(n)
                    } else {
                        serde_json::Value::String(f.value.clone())
                    }
                }
                _ => serde_json::Value::String(f.value.clone()),
            };
            opts.insert(f.key.clone(), val);
        }
        opts
    }

    // ---- Navigation ----

    pub fn move_selection(&mut self, delta: i32) {
        if self.editing || self.fields.is_empty() {
            return;
        }
        let len = self.fields.len() as i32;
        self.selected = ((self.selected as i32 + delta).rem_euclid(len)) as usize;
    }

    pub fn start_editing(&mut self) {
        if let Some(field) = self.fields.get(self.selected) {
            self.editing = true;
            self.edit_buffer = field.value.clone();
        }
    }

    pub fn confirm_edit(&mut self) {
        if let Some(field) = self.fields.get_mut(self.selected) {
            // Validate number fields
            if field.field_type == FieldType::Number && !self.edit_buffer.is_empty() {
                if self.edit_buffer.parse::<u64>().is_err() {
                    return; // reject invalid number
                }
            }
            field.value = self.edit_buffer.clone();
        }
        self.editing = false;
    }

    pub fn cancel_edit(&mut self) {
        self.editing = false;
        self.edit_buffer.clear();
    }

    pub fn input_char(&mut self, c: char) {
        if !self.editing {
            return;
        }
        if let Some(field) = self.fields.get(self.selected) {
            match field.field_type {
                FieldType::Number => {
                    if c.is_ascii_digit() {
                        self.edit_buffer.push(c);
                    }
                }
                _ => {
                    self.edit_buffer.push(c);
                }
            }
        }
    }

    pub fn backspace(&mut self) {
        if self.editing {
            self.edit_buffer.pop();
        }
    }

    // ---- Field definitions per segment ----

    fn build_fields(
        segment_id: SegmentId,
        options: &HashMap<String, serde_json::Value>,
    ) -> Vec<OptionField> {
        let schema = Self::get_schema(segment_id);
        schema
            .into_iter()
            .map(|(key, label, ft, desc)| {
                let value = options
                    .get(&key)
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Number(n) => n.to_string(),
                        serde_json::Value::Bool(b) => b.to_string(),
                        _ => v.to_string(),
                    })
                    .unwrap_or_default();
                OptionField {
                    key,
                    label,
                    value,
                    field_type: ft,
                    description: desc,
                }
            })
            .collect()
    }

    fn get_schema(segment_id: SegmentId) -> Vec<(String, String, FieldType, String)> {
        match segment_id {
            SegmentId::Usage => vec![
                (
                    "admin_email".into(),
                    "Admin Email".into(),
                    FieldType::Text,
                    "Sub2API login email".into(),
                ),
                (
                    "admin_password".into(),
                    "Admin Password".into(),
                    FieldType::Password,
                    "Sub2API login password".into(),
                ),
                (
                    "api_base_url".into(),
                    "API Base URL".into(),
                    FieldType::Text,
                    "Override (auto from settings.json)".into(),
                ),
                (
                    "bar_style".into(),
                    "Bar Style".into(),
                    FieldType::Text,
                    "heat (gradient) / block (classic)".into(),
                ),
                (
                    "bar_colored".into(),
                    "Bar Colored".into(),
                    FieldType::Text,
                    "true / false  (ANSI RGB colors)".into(),
                ),
                (
                    "bar_width".into(),
                    "Bar Width".into(),
                    FieldType::Number,
                    "Progress bar width in chars, default 20".into(),
                ),
                (
                    "cache_duration".into(),
                    "Usage Cache (s)".into(),
                    FieldType::Number,
                    "5H/7D refresh interval, default 60".into(),
                ),
                (
                    "auth_cache_duration".into(),
                    "Auth Cache (s)".into(),
                    FieldType::Number,
                    "JWT token cache TTL, default 3600".into(),
                ),
                (
                    "timeout".into(),
                    "Timeout (s)".into(),
                    FieldType::Number,
                    "HTTP request timeout, default 5".into(),
                ),
            ],
            SegmentId::Git => vec![(
                "show_sha".into(),
                "Show SHA".into(),
                FieldType::Text,
                "true / false".into(),
            )],
            _ => {
                // Generic: show existing keys
                Vec::new()
            }
        }
    }

    // ---- Render ----

    pub fn render(&self, f: &mut Frame, area: Rect) {
        if !self.is_open {
            return;
        }

        let title = match self.segment_id {
            Some(id) => format!(" Options: {:?} ", id),
            None => " Options ".to_string(),
        };

        let field_count = self.fields.len();
        let popup_height = (field_count as u16 * 2 + 5).min(area.height.saturating_sub(4));
        let popup_width = 62_u16.min(area.width.saturating_sub(4));

        let popup_area = Rect {
            x: area.width.saturating_sub(popup_width) / 2,
            y: area.height.saturating_sub(popup_height) / 2,
            width: popup_width,
            height: popup_height,
        };

        f.render_widget(Clear, popup_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(title);
        let inner = block.inner(popup_area);
        f.render_widget(block, popup_area);

        // Split: fields area + help bar
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);

        // Render fields
        let mut lines: Vec<Line> = Vec::new();
        for (i, field) in self.fields.iter().enumerate() {
            let is_selected = i == self.selected;
            let arrow = if is_selected { "▶ " } else { "  " };

            let display_value = if self.editing && is_selected {
                // Show edit buffer with cursor
                format!("{}▌", self.edit_buffer)
            } else if field.field_type == FieldType::Password && !field.value.is_empty() {
                "••••••••".to_string()
            } else if field.value.is_empty() {
                "(empty)".to_string()
            } else {
                field.value.clone()
            };

            let label_style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let value_style = if self.editing && is_selected {
                Style::default().fg(Color::Yellow)
            } else if field.value.is_empty() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Green)
            };

            lines.push(Line::from(vec![
                Span::styled(arrow, Style::default().fg(Color::Cyan)),
                Span::styled(format!("{}: ", field.label), label_style),
                Span::styled(display_value, value_style),
            ]));

            // Description line
            if is_selected {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(&field.description, Style::default().fg(Color::DarkGray)),
                ]));
            } else {
                lines.push(Line::from(""));
            }
        }

        f.render_widget(Paragraph::new(lines), chunks[0]);

        // Help bar
        let help_text = if self.editing {
            "[Enter] Confirm  [Esc] Cancel"
        } else {
            "[↑↓] Navigate  [Enter] Edit  [Esc] Back"
        };
        f.render_widget(
            Paragraph::new(help_text).style(Style::default().fg(Color::DarkGray)),
            chunks[1],
        );
    }
}
