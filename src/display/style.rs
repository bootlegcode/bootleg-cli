//! Shared human output policy. Machine output never passes through this module.
use comfy_table::{ContentArrangement, Table, modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL};
use owo_colors::OwoColorize;
use std::{io::IsTerminal, sync::OnceLock};

static NO_COLOR: OnceLock<bool> = OnceLock::new();
pub fn configure(no_color: bool) {
    let _ = NO_COLOR.set(no_color);
}

#[derive(Clone, Copy)]
pub enum Tone {
    Info,
    Success,
    Warning,
    Error,
}
#[derive(Clone, Copy)]
pub struct Renderer {
    pub decorated: bool,
    pub color: bool,
    pub width: usize,
}
impl Renderer {
    pub fn stdout() -> Self {
        Self::for_stream(std::io::stdout().is_terminal())
    }
    pub fn stderr() -> Self {
        Self::for_stream(std::io::stderr().is_terminal())
    }
    fn for_stream(tty: bool) -> Self {
        let decorated = tty && std::env::var("TERM").as_deref() != Ok("dumb");
        let color = !NO_COLOR.get().copied().unwrap_or(false);
        Self {
            decorated,
            color,
            width: terminal_size::terminal_size()
                .map(|(w, _)| usize::from(w.0))
                .unwrap_or(100)
                .max(12),
        }
    }
    fn heading(self, title: &str, tone: Tone) -> String {
        let title = crate::domain::terminal_text(title);
        if !self.color {
            return title;
        }
        match tone {
            Tone::Info => title.cyan().bold().to_string(),
            Tone::Success => title.green().bold().to_string(),
            Tone::Warning => title.yellow().bold().to_string(),
            Tone::Error => title.red().bold().to_string(),
        }
    }
    pub fn panel(self, title: &str, body: &str, tone: Tone) -> String {
        let body = body
            .lines()
            .map(crate::domain::terminal_text)
            .collect::<Vec<_>>()
            .join("\n");
        let heading = self.heading(title, tone);
        if !self.decorated {
            return format!("{heading}\n{body}\n");
        }
        // Wrap before inserting ANSI escapes, so escape sequences never affect widths.
        let wrapped = textwrap::fill(&body, self.width.saturating_sub(4));
        let mut table = Table::new();
        table
            .load_preset(UTF8_FULL)
            .apply_modifier(UTF8_ROUND_CORNERS)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_width(self.width.min(u16::MAX as usize) as u16)
            .add_row([wrapped]);
        format!("{heading}\n{table}\n")
    }
    pub fn table(self, title: &str, headers: &[&str], rows: Vec<Vec<String>>) -> String {
        self.table_with_selection(title, headers, rows, None)
    }
    pub fn table_with_selection(
        self,
        title: &str,
        headers: &[&str],
        rows: Vec<Vec<String>>,
        selected: Option<usize>,
    ) -> String {
        if rows.is_empty() {
            return self.panel(
                title,
                &format!("No {} found.", title.to_lowercase()),
                Tone::Info,
            );
        }
        let title = if self.color && selected.is_some() {
            format!("{title} (Current in green)")
        } else {
            title.to_owned()
        };
        let title = title.as_str();
        let rows: Vec<Vec<String>> = rows
            .into_iter()
            .map(|r| {
                r.into_iter()
                    .map(|s| crate::domain::terminal_text(&s))
                    .collect()
            })
            .collect();
        let current = if !self.color {
            selected
                .and_then(|i| rows.get(i))
                .map(|row| format!("Current: {} ({})\n", row[1], row[0]))
                .unwrap_or_default()
        } else {
            String::new()
        };
        if self.decorated && self.width < 90 {
            let panels: String = rows
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    let panel = self.panel(
                        title,
                        &headers
                            .iter()
                            .zip(row)
                            .map(|(h, v)| format!("{h}: {v}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                        if selected == Some(i) && self.color {
                            Tone::Success
                        } else {
                            Tone::Info
                        },
                    );
                    if selected == Some(i) && self.color {
                        panel.green().to_string()
                    } else {
                        panel
                    }
                })
                .collect();
            return format!("{current}{panels}");
        }
        if !self.decorated {
            return format!(
                "{current}{}\n{}\n{}\n",
                self.heading(title, Tone::Info),
                headers.join("\t"),
                rows.iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let row = r.join("\t");
                        if selected == Some(i) && self.color {
                            row.green().to_string()
                        } else {
                            row
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        let mut table = Table::new();
        table
            .load_preset(comfy_table::presets::UTF8_FULL_CONDENSED)
            .apply_modifier(UTF8_ROUND_CORNERS)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_width(self.width.min(u16::MAX as usize) as u16)
            .set_header(headers.iter().map(|h| {
                let cell = comfy_table::Cell::new(h);
                if self.color {
                    cell.add_attribute(comfy_table::Attribute::Bold)
                } else {
                    cell
                }
            }));
        if self.color {
            table.enforce_styling();
        } else {
            table.force_no_tty();
        }
        for (i, row) in rows.into_iter().enumerate() {
            table.add_row(row.into_iter().map(|value| {
                let cell = comfy_table::Cell::new(value);
                if selected == Some(i) && self.color {
                    cell.fg(comfy_table::Color::Green)
                } else {
                    cell
                }
            }));
        }
        format!("{current}{}\n{table}\n", self.heading(title, Tone::Info))
    }
}
pub fn message(title: &str, body: &str, tone: Tone) {
    eprint!("{}", Renderer::stderr().panel(title, body, tone));
}

/// Render safe, explicitly selected result DTOs, never raw credential responses.
pub fn result(value: &serde_json::Value) -> String {
    fn flatten(value: &serde_json::Value, prefix: &str, rows: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    let label = key.replace('_', " ");
                    flatten(
                        value,
                        &if prefix.is_empty() {
                            label
                        } else {
                            format!("{prefix} · {label}")
                        },
                        rows,
                    );
                }
            }
            serde_json::Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    flatten(item, &format!("{prefix} {}", i + 1), rows);
                }
            }
            _ => rows.push(format!(
                "{prefix}: {}",
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            )),
        }
    }
    let renderer = Renderer::stdout();
    if value.get("stale").and_then(|v| v.as_bool()) == Some(true) {
        return renderer.panel("Warning · Instance data is stale", "Refresh failed. Previous results are not current.\nPresence means control-plane connectivity, not readiness.", Tone::Warning);
    }
    if let Some(items) = value.get("items").and_then(|v| v.as_array()) {
        let text = |v: &serde_json::Value| {
            v.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string())
        };
        let rows = items
            .iter()
            .map(|item| {
                let c = &item["container"];
                vec![
                    text(&c["display_name"]),
                    text(&c["container_id"]),
                    text(&c["presence"]),
                    text(&c["application_state"]),
                    text(&c["build_commit"]),
                    text(&item["baseline"]),
                ]
            })
            .collect();
        return format!(
            "{}{}",
            renderer.table(
                "Instances",
                &["Name", "ID", "Presence", "Application", "Build", "Baseline"],
                rows
            ),
            renderer.panel(
                "Snapshot",
                &format!(
                    "Server time: {}\nPresence means control-plane connectivity, not readiness.",
                    text(&value["server_time"])
                ),
                Tone::Info
            )
        );
    }
    let mut rows = Vec::new();
    flatten(value, "", &mut rows);
    let title = if value.get("logged_in").is_some() {
        "Success · Logged in"
    } else if value.get("logged_out").is_some() {
        "Success · Logged out"
    } else if value.get("api_url").is_some() {
        "Success · Configuration saved"
    } else if value.get("project_id").is_some() {
        "Success · Project updated"
    } else {
        "Result"
    };
    renderer.panel(title, &rows.join("\n"), Tone::Success)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn render_modes_and_hostile_text() {
        let plain = Renderer {
            decorated: false,
            color: false,
            width: 40,
        };
        let text = plain.panel("Error", "bad\x1b[31m\u{202e} input", Tone::Error);
        assert!(!text.contains('\x1b'));
        assert!(!text.contains('\u{202e}'));
        let styled = Renderer {
            decorated: true,
            color: true,
            width: 40,
        };
        let text = styled.panel("Error", "Try: bootleg login", Tone::Error);
        assert!(text.contains('\x1b'));
        assert!(text.contains('╭'));
        let narrow = Renderer {
            decorated: true,
            color: false,
            width: 30,
        };
        let text = narrow.table(
            "Projects",
            &["ID", "Name"],
            vec![vec!["full-project-id".into(), "日本語".into()]],
        );
        assert!(text.contains("full-project-id"));
        assert!(!text.contains('\x1b'));
    }
}
