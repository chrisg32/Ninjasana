//! Local UI state that should survive across runs: which sections are collapsed
//! and any manually-resized column widths. Asana's API doesn't expose either,
//! so we persist them ourselves in `~/.config/ninjasana/state.json`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct StateFile {
    /// nav key -> list of collapsed section keys.
    #[serde(default)]
    collapsed: HashMap<String, Vec<String>>,
    /// column key -> resized width.
    #[serde(default)]
    column_widths: HashMap<String, usize>,
}

pub struct UiState {
    path: Option<PathBuf>,
    collapsed: HashMap<String, HashSet<String>>,
    widths: HashMap<String, usize>,
}

impl UiState {
    pub fn load() -> Self {
        let path = state_path();
        let file: StateFile = path
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let collapsed = file
            .collapsed
            .into_iter()
            .map(|(nav, keys)| (nav, keys.into_iter().collect()))
            .collect();
        Self {
            path,
            collapsed,
            widths: file.column_widths,
        }
    }

    // ---- section collapse ---------------------------------------------

    pub fn is_collapsed(&self, nav: &str, section: &str) -> bool {
        self.collapsed.get(nav).is_some_and(|s| s.contains(section))
    }

    pub fn set_collapsed(&mut self, nav: &str, section: &str, collapsed: bool) {
        let entry = self.collapsed.entry(nav.to_string()).or_default();
        if collapsed {
            entry.insert(section.to_string());
        } else {
            entry.remove(section);
        }
        self.save();
    }

    // ---- column widths ------------------------------------------------

    pub fn column_width(&self, column: &str) -> Option<usize> {
        self.widths.get(column).copied()
    }

    pub fn set_column_width(&mut self, column: &str, width: usize) {
        self.widths.insert(column.to_string(), width);
        self.save();
    }

    fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let file = StateFile {
            collapsed: self
                .collapsed
                .iter()
                .map(|(nav, keys)| (nav.clone(), keys.iter().cloned().collect()))
                .collect(),
            column_widths: self.widths.clone(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&file) {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(path, json);
        }
    }
}

fn state_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("ninjasana").join("state.json"))
}
