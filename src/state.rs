//! Local UI state that should survive across runs — currently which sections
//! are collapsed. Asana's API doesn't expose the web's collapse state, so we
//! persist it ourselves in `~/.config/ninjasana/state.json`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct StateFile {
    /// nav key -> list of collapsed section keys.
    #[serde(default)]
    collapsed: HashMap<String, Vec<String>>,
}

pub struct CollapseStore {
    path: Option<PathBuf>,
    collapsed: HashMap<String, HashSet<String>>,
}

impl CollapseStore {
    pub fn load() -> Self {
        let path = state_path();
        let collapsed = path
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str::<StateFile>(&text).ok())
            .map(|file| {
                file.collapsed
                    .into_iter()
                    .map(|(nav, keys)| (nav, keys.into_iter().collect()))
                    .collect()
            })
            .unwrap_or_default();
        Self { path, collapsed }
    }

    pub fn is_collapsed(&self, nav: &str, section: &str) -> bool {
        self.collapsed.get(nav).is_some_and(|s| s.contains(section))
    }

    pub fn set(&mut self, nav: &str, section: &str, collapsed: bool) {
        let entry = self.collapsed.entry(nav.to_string()).or_default();
        if collapsed {
            entry.insert(section.to_string());
        } else {
            entry.remove(section);
        }
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
