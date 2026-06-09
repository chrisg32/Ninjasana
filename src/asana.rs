//! A thin async Asana REST client built on `reqwest` + `serde`.
//!
//! Asana ships no official Rust SDK, so we call the REST API directly. Every
//! Asana response wraps its payload in a top-level `{ "data": ... }` envelope,
//! which [`DataEnvelope`] models generically. Most endpoints return minimal
//! fields unless asked, so we pass `opt_fields` on the requests that need more.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::config::Config;

/// Identifies which task list a set of sections belongs to, so the UI can
/// ignore responses that arrive after the user has navigated elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskListKey {
    MyTasks,
    Project(String),
}

/// A result handed back to the UI thread from an async Asana call.
#[derive(Debug, Clone)]
pub enum AsanaUpdate {
    /// Initial load: identity, workspace, and the member-project list.
    Bootstrap {
        user: User,
        workspace: Workspace,
        projects: Vec<Project>,
    },
    /// Section-grouped tasks for a given list.
    Tasks {
        key: TaskListKey,
        sections: Vec<Section>,
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

/// A paginated list response: `{ "data": [...], "next_page": { "offset": .. } }`.
#[derive(Debug, Deserialize)]
struct Page<T> {
    data: Vec<T>,
    #[serde(default)]
    next_page: Option<NextPage>,
}

#[derive(Debug, Deserialize)]
struct NextPage {
    offset: Option<String>,
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

/// A compact `{ "name": ... }` reference (assignee, project, …).
#[derive(Debug, Clone, Deserialize)]
pub struct Named {
    #[serde(default)]
    pub name: String,
}

/// A compact `{ "gid": ... }` reference (e.g. a project member).
#[derive(Debug, Clone, Deserialize)]
pub struct Ref {
    pub gid: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub gid: String,
    pub name: String,
    #[serde(default)]
    pub members: Vec<Ref>,
}

/// A section reference, with its position-defining gid.
#[derive(Debug, Clone, Deserialize)]
pub struct SectionRef {
    pub gid: String,
    #[serde(default)]
    pub name: String,
}

/// A task's membership in a project.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Membership {
    #[serde(default)]
    pub project: Option<Named>,
}

/// A custom field on a task. `display_value` is Asana's rendered value for any
/// field type (enum, text, number, …), so we can show it without type logic.
#[derive(Debug, Clone, Deserialize)]
pub struct CustomField {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub display_value: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    pub gid: String,
    pub name: String,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub due_on: Option<String>,
    #[serde(default)]
    pub assignee: Option<Named>,
    /// Present on My Tasks results: the section within the user's task list.
    #[serde(default)]
    pub assignee_section: Option<SectionRef>,
    #[serde(default)]
    pub memberships: Vec<Membership>,
    #[serde(default)]
    pub tags: Vec<Named>,
    #[serde(default)]
    pub custom_fields: Vec<CustomField>,
}

impl Task {
    /// Names of the projects this task belongs to.
    pub fn project_names(&self) -> Vec<String> {
        self.memberships
            .iter()
            .filter_map(|m| m.project.as_ref().map(|p| p.name.clone()))
            .filter(|n| !n.is_empty())
            .collect()
    }

    /// Names of the tags on this task.
    pub fn tag_names(&self) -> Vec<String> {
        self.tags
            .iter()
            .map(|t| t.name.clone())
            .filter(|n| !n.is_empty())
            .collect()
    }

    /// The display value of a named custom field (e.g. "Dev Status v2"), if set.
    pub fn custom_field(&self, name: &str) -> Option<String> {
        self.custom_fields
            .iter()
            .find(|f| f.name == name)
            .and_then(|f| f.display_value.clone())
            .filter(|v| !v.is_empty())
    }
}

/// A named group of tasks, in display order. `gid` is the section's id when one
/// exists (project sections, and My Tasks assignee-sections) — used to persist
/// moves. It is `None` for the synthetic "(No section)" bucket.
#[derive(Debug, Clone)]
pub struct Section {
    pub gid: Option<String>,
    pub name: String,
    pub tasks: Vec<Task>,
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

/// `{ "gid": ... }` for the user's task list.
#[derive(Debug, Clone, Deserialize)]
struct UserTaskList {
    gid: String,
}

const NO_SECTION: &str = "(No section)";

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

    /// Issue paginated GETs, following `next_page` until exhausted.
    async fn get_all<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<Vec<T>> {
        let mut out = Vec::new();
        let mut offset: Option<String> = None;
        loop {
            let mut q: Vec<(&str, &str)> = query.to_vec();
            if let Some(o) = &offset {
                q.push(("offset", o));
            }
            let page: Page<T> = self
                .http
                .get(self.url(path, &q))
                .bearer_auth(&self.config.token)
                .send()
                .await
                .context("sending request to Asana")?
                .error_for_status()
                .context("Asana returned an error status")?
                .json()
                .await
                .context("decoding the Asana response")?;
            out.extend(page.data);
            match page.next_page.and_then(|n| n.offset) {
                Some(next) => offset = Some(next),
                None => break,
            }
        }
        Ok(out)
    }

    /// Fetch the authenticated user. Doubles as a connectivity/auth check.
    pub async fn me(&self) -> Result<User> {
        self.get("users/me", &[]).await
    }

    pub async fn workspaces(&self) -> Result<Vec<Workspace>> {
        self.get("workspaces", &[("limit", "100")]).await
    }

    /// The user's favorited projects, in sidebar order — the closest public-API
    /// match to Asana's web "Projects"/"Starred" sidebar. Current user only.
    pub async fn favorite_projects(&self, workspace_gid: &str) -> Result<Vec<Project>> {
        self.get_all(
            "users/me/favorites",
            &[
                ("workspace", workspace_gid),
                ("resource_type", "project"),
                ("limit", "100"),
                ("opt_fields", "name"),
            ],
        )
        .await
    }

    /// Every (non-archived) project in the workspace, by name. Used to resolve
    /// an explicit, name-based project list from config.
    pub async fn all_projects(&self, workspace_gid: &str) -> Result<Vec<Project>> {
        self.get_all(
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

    /// Every (non-archived) project in the workspace that the user is a member
    /// of. A superset of the web sidebar — membership ≠ sidebar presence.
    pub async fn member_projects(&self, workspace_gid: &str, user_gid: &str) -> Result<Vec<Project>> {
        let projects: Vec<Project> = self
            .get_all(
                "projects",
                &[
                    ("workspace", workspace_gid),
                    ("archived", "false"),
                    ("limit", "100"),
                    ("opt_fields", "name,members"),
                ],
            )
            .await?;
        Ok(projects
            .into_iter()
            .filter(|p| p.members.iter().any(|m| m.gid == user_gid))
            .collect())
    }

    /// Section-grouped tasks for a project, in the project's section order.
    pub async fn project_sections(&self, project_gid: &str) -> Result<Vec<Section>> {
        let sections: Vec<SectionRef> = self
            .get(
                &format!("projects/{project_gid}/sections"),
                &[("opt_fields", "name"), ("limit", "100")],
            )
            .await?;

        let mut out = Vec::with_capacity(sections.len());
        for section in sections {
            let tasks: Vec<Task> = self
                .get(
                    &format!("sections/{}/tasks", section.gid),
                    &[
                        ("limit", "100"),
                        (
                            "opt_fields",
                            "name,completed,due_on,assignee.name,memberships.project.name,\
                             tags.name,custom_fields.name,custom_fields.display_value",
                        ),
                    ],
                )
                .await
                .unwrap_or_default();
            let name = if section.name.is_empty() {
                NO_SECTION.to_string()
            } else {
                section.name
            };
            out.push(Section {
                gid: Some(section.gid),
                name,
                tasks,
            });
        }
        Ok(out)
    }

    /// Section-grouped "My Tasks" for the user, in My Tasks display order.
    pub async fn my_tasks_sections(
        &self,
        workspace_gid: &str,
        user_gid: &str,
    ) -> Result<Vec<Section>> {
        let utl: UserTaskList = self
            .get(
                &format!("users/{user_gid}/user_task_list"),
                &[("workspace", workspace_gid)],
            )
            .await?;

        let tasks: Vec<Task> = self
            .get(
                &format!("user_task_lists/{}/tasks", utl.gid),
                &[
                    ("completed_since", "now"),
                    ("limit", "100"),
                    (
                        "opt_fields",
                        "name,completed,due_on,assignee.name,assignee_section.name,\
                         memberships.project.name,tags.name,custom_fields.name,\
                         custom_fields.display_value",
                    ),
                ],
            )
            .await?;

        Ok(group_by_assignee_section(tasks))
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

    /// Send a body to `path` with the given method and check the status.
    async fn write(&self, method: reqwest::Method, path: &str, body: Value) -> Result<()> {
        self.http
            .request(method, self.url(path, &[]))
            .bearer_auth(&self.config.token)
            .json(&body)
            .send()
            .await
            .context("sending request to Asana")?
            .error_for_status()
            .context("Asana returned an error status")?;
        Ok(())
    }

    /// Move `task_gid` within a project section, positioning it before
    /// `insert_before` (or at the end when `None`). Also moves the task into the
    /// section if it wasn't already there.
    pub async fn move_task_in_section(
        &self,
        section_gid: &str,
        task_gid: &str,
        insert_before: Option<&str>,
    ) -> Result<()> {
        let mut data = json!({ "task": task_gid });
        if let Some(before) = insert_before {
            data["insert_before"] = json!(before);
        }
        self.write(
            reqwest::Method::POST,
            &format!("sections/{section_gid}/addTask"),
            json!({ "data": data }),
        )
        .await
    }

    /// Move a task to a different My Tasks (assignee) section.
    pub async fn set_assignee_section(&self, task_gid: &str, section_gid: &str) -> Result<()> {
        self.write(
            reqwest::Method::PUT,
            &format!("tasks/{task_gid}"),
            json!({ "data": { "assignee_section": section_gid } }),
        )
        .await
    }
}

/// Group My Tasks by their assignee-section, preserving the order in which the
/// sections first appear (which matches the user's My Tasks ordering). Keyed by
/// section gid so renamed/duplicate names stay distinct.
fn group_by_assignee_section(tasks: Vec<Task>) -> Vec<Section> {
    // Stable insertion order of section keys.
    let mut order: Vec<String> = Vec::new();
    let mut meta: HashMap<String, (Option<String>, String)> = HashMap::new();
    let mut groups: HashMap<String, Vec<Task>> = HashMap::new();

    for task in tasks {
        let (key, gid, name) = match &task.assignee_section {
            Some(section) => {
                let name = if section.name.is_empty() {
                    NO_SECTION.to_string()
                } else {
                    section.name.clone()
                };
                (section.gid.clone(), Some(section.gid.clone()), name)
            }
            None => (NO_SECTION.to_string(), None, NO_SECTION.to_string()),
        };
        if !groups.contains_key(&key) {
            order.push(key.clone());
            meta.insert(key.clone(), (gid, name));
        }
        groups.entry(key).or_default().push(task);
    }

    order
        .into_iter()
        .map(|key| {
            let tasks = groups.remove(&key).unwrap_or_default();
            let (gid, name) = meta.remove(&key).unwrap_or((None, NO_SECTION.to_string()));
            Section { gid, name, tasks }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{group_by_assignee_section, Task};

    #[test]
    fn groups_my_tasks_by_assignee_section_in_first_seen_order() {
        // Shaped like a real /user_task_lists/{}/tasks response.
        let json = r#"[
            {"gid":"1","name":"A","assignee_section":{"gid":"s1","name":"Now"}},
            {"gid":"2","name":"B","assignee_section":{"gid":"s2","name":"Today"}},
            {"gid":"3","name":"C","assignee_section":{"gid":"s1","name":"Now"}},
            {"gid":"4","name":"D"}
        ]"#;
        let tasks: Vec<Task> = serde_json::from_str(json).unwrap();
        let sections = group_by_assignee_section(tasks);

        // Section order follows first appearance; unsectioned tasks bucket last.
        let names: Vec<&str> = sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Now", "Today", "(No section)"]);

        // Sections carry their gid (used to persist moves); synthetic one has none.
        assert_eq!(sections[0].gid.as_deref(), Some("s1"));
        assert_eq!(sections[2].gid, None);

        // Tasks land in the right group, in order.
        let now: Vec<&str> = sections[0].tasks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(now, vec!["A", "C"]);
        assert_eq!(sections[1].tasks.len(), 1);
        assert_eq!(sections[2].tasks[0].name, "D");
    }

    #[test]
    fn task_accessors_read_tags_and_custom_fields() {
        let json = r#"{
            "gid":"9","name":"T","completed":false,
            "tags":[{"name":"infra"},{"name":"urgent"}],
            "custom_fields":[
                {"name":"Dev Status v2","display_value":"2. Development"},
                {"name":"Empty","display_value":null}
            ],
            "memberships":[{"project":{"name":"Chateau"}}]
        }"#;
        let task: Task = serde_json::from_str(json).unwrap();
        assert_eq!(task.tag_names(), vec!["infra", "urgent"]);
        assert_eq!(task.project_names(), vec!["Chateau"]);
        assert_eq!(task.custom_field("Dev Status v2").as_deref(), Some("2. Development"));
        assert_eq!(task.custom_field("Empty"), None);
        assert_eq!(task.custom_field("Missing"), None);
    }
}
