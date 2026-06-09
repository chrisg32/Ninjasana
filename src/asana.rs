//! A thin async Asana REST client built on `reqwest` + `serde`.
//!
//! Asana ships no official Rust SDK, so we call the REST API directly. Every
//! Asana response wraps its payload in a top-level `{ "data": ... }` envelope,
//! which [`DataEnvelope`] models generically. Most endpoints return minimal
//! fields unless asked, so we pass `opt_fields` on the requests that need more.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::config::Config;

/// Identifies which task list a set of tasks belongs to, so the UI can ignore
/// responses that arrive after the user has navigated elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskListKey {
    MyTasks,
    Project(String),
}

/// A result handed back to the UI thread from an async Asana call.
#[derive(Debug, Clone)]
pub enum AsanaUpdate {
    /// Initial load: identity, workspace, and the project list.
    Bootstrap {
        user: User,
        workspace: Workspace,
        projects: Vec<Project>,
    },
    /// Tasks for a given list (My Tasks or a project).
    Tasks {
        key: TaskListKey,
        tasks: Vec<Task>,
    },
    /// Full detail for a single task.
    Detail(TaskDetail),
    /// Something went wrong; carries a human-readable message.
    Error(String),
}

/// Generic `{ "data": T }` envelope used by every Asana endpoint.
#[derive(Debug, Deserialize)]
struct DataEnvelope<T> {
    data: T,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub gid: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub gid: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub gid: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    pub gid: String,
    pub name: String,
    #[serde(default)]
    pub completed: bool,
}

/// A nested `{ "name": ... }` object (e.g. an assignee).
#[derive(Debug, Clone, Deserialize)]
pub struct Named {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskDetail {
    #[allow(dead_code)]
    pub gid: String,
    pub name: String,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub notes: String,
    pub assignee: Option<Named>,
    pub due_on: Option<String>,
    pub permalink_url: Option<String>,
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    config: Config,
}

impl Client {
    pub fn new(config: Config) -> Self {
        Self {
            http: reqwest::Client::new(),
            config,
        }
    }

    /// Build a full URL, appending `query` as a `?k=v&…` string. The values we
    /// pass (gids, field lists, literals) are all query-safe, so no escaping is
    /// needed.
    fn url(&self, path: &str, query: &[(&str, &str)]) -> String {
        let mut url = format!(
            "{}/{}",
            self.config.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        if !query.is_empty() {
            let pairs: Vec<String> = query.iter().map(|(k, v)| format!("{k}={v}")).collect();
            url.push('?');
            url.push_str(&pairs.join("&"));
        }
        url
    }

    /// Issue a GET and unwrap the `{ "data": ... }` envelope.
    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        let envelope: DataEnvelope<T> = self
            .http
            .get(self.url(path, query))
            .bearer_auth(&self.config.token)
            .send()
            .await
            .context("sending request to Asana")?
            .error_for_status()
            .context("Asana returned an error status")?
            .json()
            .await
            .context("decoding the Asana response")?;
        Ok(envelope.data)
    }

    /// Fetch the authenticated user. Doubles as a connectivity/auth check.
    pub async fn me(&self) -> Result<User> {
        self.get("users/me", &[]).await
    }

    pub async fn workspaces(&self) -> Result<Vec<Workspace>> {
        self.get("workspaces", &[("limit", "100")]).await
    }

    pub async fn projects(&self, workspace_gid: &str) -> Result<Vec<Project>> {
        self.get(
            "projects",
            &[
                ("workspace", workspace_gid),
                ("archived", "false"),
                ("limit", "100"),
                ("opt_fields", "name"),
            ],
        )
        .await
    }

    pub async fn tasks_in_project(&self, project_gid: &str) -> Result<Vec<Task>> {
        self.get(
            &format!("projects/{project_gid}/tasks"),
            &[("limit", "100"), ("opt_fields", "name,completed")],
        )
        .await
    }

    /// Incomplete tasks assigned to the user in the given workspace.
    pub async fn my_tasks(&self, workspace_gid: &str, user_gid: &str) -> Result<Vec<Task>> {
        self.get(
            "tasks",
            &[
                ("assignee", user_gid),
                ("workspace", workspace_gid),
                ("completed_since", "now"),
                ("limit", "100"),
                ("opt_fields", "name,completed"),
            ],
        )
        .await
    }

    pub async fn task(&self, gid: &str) -> Result<TaskDetail> {
        self.get(
            &format!("tasks/{gid}"),
            &[(
                "opt_fields",
                "name,completed,notes,due_on,assignee.name,permalink_url",
            )],
        )
        .await
    }
}
