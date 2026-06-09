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

    /// Stable identity for persisting per-column state (e.g. resized width).
    pub fn key(&self) -> String {
        match self {
            Column::Name => "name".to_string(),
            Column::DueDate => "due_date".to_string(),
            Column::Assignee => "assignee".to_string(),
            Column::Projects => "projects".to_string(),
            Column::Tags => "tags".to_string(),
            Column::Completed => "completed".to_string(),
            Column::Custom(name) => format!("custom:{name}"),
        }
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

/// Which projects to show in the navigation pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectSource {
    /// Favorited projects, in sidebar order (closest to the web sidebar).
    Favorites,
    /// All projects the user is a member of.
    Member,
    /// An explicit, ordered list of project names (the only way to reproduce
    /// the web sidebar's curated "Projects" list exactly).
    Explicit(Vec<String>),
}

impl ProjectSource {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "favorites" | "favorite" | "starred" => Some(ProjectSource::Favorites),
            "member" | "members" | "all" => Some(ProjectSource::Member),
            _ => None,
        }
    }
}

/// The `projects` config value: either a mode string or an explicit name list.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawProjects {
    Mode(String),
    List(Vec<String>),
}

pub struct Settings {
    pub columns: Vec<Column>,
    pub projects: ProjectSource,
}

impl Settings {
    pub fn load() -> Self {
        let raw = read_raw();
        let columns = raw
            .as_ref()
            .map(|r| parse_columns(&r.columns))
            .filter(|c| !c.is_empty())
            .unwrap_or_else(default_columns);
        let projects = project_source(raw.as_ref().and_then(|r| r.projects.as_ref()));
        Settings { columns, projects }
    }
}

#[derive(Debug, Deserialize, Default)]
struct RawConfig {
    #[serde(default)]
    columns: Vec<String>,
    #[serde(default)]
    projects: Option<RawProjects>,
}

/// Read and parse the config file; seed a default on first run.
fn read_raw() -> Option<RawConfig> {
    let path = config_path()?;
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).ok(),
        Err(_) => {
            let _ = write_default(&path);
            None
        }
    }
}

fn parse_columns(tokens: &[String]) -> Vec<Column> {
    tokens.iter().filter_map(|c| Column::parse(c)).collect()
}

/// Resolve the `projects` config value into a [`ProjectSource`].
fn project_source(raw: Option<&RawProjects>) -> ProjectSource {
    match raw {
        Some(RawProjects::List(names)) if !names.is_empty() => {
            ProjectSource::Explicit(names.clone())
        }
        Some(RawProjects::Mode(mode)) => {
            ProjectSource::parse(mode).unwrap_or(ProjectSource::Favorites)
        }
        _ => ProjectSource::Favorites,
    }
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

# Which projects appear in the navigation pane. Either a mode:
#   \"favorites\" — your favorited projects, in sidebar order (default)
#   \"member\"    — every project you're a member of
# ...or an explicit, ordered list of project names to show exactly those:
#   projects = [\"ISMS\", \"Sprint - Maximilian\", \"Software Department\"]
projects = \"favorites\"
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

#[cfg(test)]
mod tests {
    use super::{parse_columns, project_source, Column, ProjectSource, RawConfig};

    #[test]
    fn parses_builtin_and_custom_columns() {
        assert_eq!(Column::parse("name"), Some(Column::Name));
        assert_eq!(Column::parse("due_date"), Some(Column::DueDate));
        assert_eq!(Column::parse("due"), Some(Column::DueDate));
        assert_eq!(
            Column::parse("custom:Dev Status v2"),
            Some(Column::Custom("Dev Status v2".to_string()))
        );
        assert_eq!(Column::parse("not_a_column"), None);
        assert_eq!(Column::parse("custom:"), None);
    }

    #[test]
    fn parse_columns_skips_unknown_tokens() {
        let tokens = ["name".to_string(), "bogus".to_string(), "tags".to_string()];
        assert_eq!(parse_columns(&tokens), vec![Column::Name, Column::Tags]);
    }

    #[test]
    fn project_source_accepts_mode_string() {
        let raw: RawConfig = toml::from_str(r#"projects = "member""#).unwrap();
        assert_eq!(
            project_source(raw.projects.as_ref()),
            ProjectSource::Member
        );
    }

    #[test]
    fn project_source_accepts_explicit_list() {
        let raw: RawConfig = toml::from_str(r#"projects = ["ISMS", "Sprint - Maximilian"]"#).unwrap();
        assert_eq!(
            project_source(raw.projects.as_ref()),
            ProjectSource::Explicit(vec!["ISMS".to_string(), "Sprint - Maximilian".to_string()])
        );
    }

    #[test]
    fn project_source_defaults_to_favorites() {
        // Absent, unknown, and empty-list all fall back to favorites.
        assert_eq!(project_source(None), ProjectSource::Favorites);
        let bad: RawConfig = toml::from_str(r#"projects = "nonsense""#).unwrap();
        assert_eq!(project_source(bad.projects.as_ref()), ProjectSource::Favorites);
        let empty: RawConfig = toml::from_str(r#"projects = []"#).unwrap();
        assert_eq!(project_source(empty.projects.as_ref()), ProjectSource::Favorites);
    }

    #[test]
    fn full_config_round_trips() {
        let raw: RawConfig = toml::from_str(
            r#"
            columns = ["name", "custom:Dev Status v2", "tags"]
            projects = ["ISMS"]
            "#,
        )
        .unwrap();
        assert_eq!(
            parse_columns(&raw.columns),
            vec![
                Column::Name,
                Column::Custom("Dev Status v2".to_string()),
                Column::Tags
            ]
        );
        assert_eq!(
            project_source(raw.projects.as_ref()),
            ProjectSource::Explicit(vec!["ISMS".to_string()])
        );
    }
}
