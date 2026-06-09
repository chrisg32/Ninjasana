//! User configuration, read from `~/.config/ninjasana/config.toml` (honoring
//! `XDG_CONFIG_HOME`). The shipped default is intentionally generic — no
//! workspace-specific custom fields — so the tool is safe to publish. On first
//! run, if no config exists, the default is written so the user has something
//! to edit.

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use crate::asana::Task;

/// A column in the task table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Column {
    Name,
    DueDate,
    Assignee,
    Projects,
    Tags,
    Completed,
    /// A custom field, referenced by its exact Asana name.
    Custom(String),
}

impl Column {
    /// Parse one config token, e.g. `"due_date"` or `"custom:Dev Status v2"`.
    fn parse(token: &str) -> Option<Self> {
        let token = token.trim();
        if let Some(name) = token.strip_prefix("custom:") {
            let name = name.trim();
            return (!name.is_empty()).then(|| Column::Custom(name.to_string()));
        }
        match token.to_ascii_lowercase().as_str() {
            "name" => Some(Column::Name),
            "due_date" | "due" => Some(Column::DueDate),
            "assignee" => Some(Column::Assignee),
            "projects" | "project" => Some(Column::Projects),
            "tags" | "tag" => Some(Column::Tags),
            "completed" | "done" => Some(Column::Completed),
            _ => None,
        }
    }

    pub fn title(&self) -> String {
        match self {
            Column::Name => "Name".to_string(),
            Column::DueDate => "Due Date".to_string(),
            Column::Assignee => "Assignee".to_string(),
            Column::Projects => "Projects".to_string(),
            Column::Tags => "Tags".to_string(),
            Column::Completed => "Done".to_string(),
            Column::Custom(name) => name.clone(),
        }
    }

    /// Fixed display width. `Name` returns 0, meaning "flex to fill".
    pub fn width(&self) -> usize {
        match self {
            Column::Name => 0,
            Column::DueDate => 10,
            Column::Assignee => 16,
            Column::Projects => 22,
            Column::Tags => 18,
            Column::Completed => 4,
            Column::Custom(_) => 16,
        }
    }

    pub fn is_name(&self) -> bool {
        matches!(self, Column::Name)
    }

    /// The cell value for a task.
    pub fn value(&self, task: &Task) -> String {
        match self {
            Column::Name => task.name.clone(),
            Column::DueDate => task.due_on.clone().unwrap_or_else(|| "—".to_string()),
            Column::Assignee => task
                .assignee
                .as_ref()
                .map(|a| a.name.clone())
                .unwrap_or_default(),
            Column::Projects => task.project_names().join(", "),
            Column::Tags => task.tag_names().join(", "),
            Column::Completed => if task.completed { "✓" } else { "" }.to_string(),
            Column::Custom(name) => task.custom_field(name).unwrap_or_default(),
        }
    }
}

pub struct Settings {
    pub columns: Vec<Column>,
}

impl Settings {
    pub fn load() -> Self {
        let columns = match config_path() {
            Some(path) => match fs::read_to_string(&path) {
                Ok(text) => parse(&text).unwrap_or_else(default_columns),
                // Missing (or unreadable): seed a default the user can edit.
                Err(_) => {
                    let _ = write_default(&path);
                    default_columns()
                }
            },
            None => default_columns(),
        };
        Settings { columns }
    }
}

#[derive(Debug, Deserialize, Default)]
struct RawConfig {
    #[serde(default)]
    columns: Vec<String>,
}

fn parse(text: &str) -> Option<Vec<Column>> {
    let raw: RawConfig = toml::from_str(text).ok()?;
    let columns: Vec<Column> = raw.columns.iter().filter_map(|c| Column::parse(c)).collect();
    (!columns.is_empty()).then_some(columns)
}

fn default_columns() -> Vec<Column> {
    vec![
        Column::Name,
        Column::DueDate,
        Column::Assignee,
        Column::Projects,
        Column::Tags,
    ]
}

const DEFAULT_TOML: &str = "\
# Ninjasana configuration.
#
# Columns shown in the task table, in order. Built-in columns:
#   \"name\", \"due_date\", \"assignee\", \"projects\", \"tags\", \"completed\"
# Custom fields use a \"custom:\" prefix with the exact Asana field name, e.g.:
#   \"custom:Dev Status v2\"
columns = [\"name\", \"due_date\", \"assignee\", \"projects\", \"tags\"]
";

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("ninjasana").join("config.toml"))
}

fn write_default(path: &PathBuf) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, DEFAULT_TOML)
}
