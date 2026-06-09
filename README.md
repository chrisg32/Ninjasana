# Ninjasana

A **mouse-native**, layout-friendly terminal UI for [Asana](https://asana.com).

Inspired by [herdr](https://herdr.dev) — and designed to run happily *inside* a
herdr pane. Where most TUIs are keyboard-first with mouse support bolted on,
Ninjasana treats the mouse as a first-class input: click views and tasks, scroll
lists, collapse sections, drag rows to reorder them, and drag the column
dividers to resize them.

```
╭───────────────────────────────────────────────────────────────────────────╮
│ Ninjasana  ·  Asana in your terminal   Your Name                    [ Quit ]│
╰───────────────────────────────────────────────────────────────────────────╯
╭ Navigation ──╮╭ My Tasks ───────────────────────────────────╮╭ Task ───────╮
│★ My Tasks    ││  Name           │Due Date  │Dev Status│Tags  ││ Deploy …    │
│# Engineering ││▾ Now (1)                                     ││ ○ incomplete│
│# Design       │ ○ Deploy SignalR…│2026-06-12│Development│      ││ Assignee: … │
│# Roadmap      │▾ Today (3)                                    ││ Due: …      │
│              ││ ○ Read-only SQL …│—         │Development│infra ││ Notes …     │
╰──────────────╯╰──────────────────────────────────────────────╯╰─────────────╯
 click: open · drag row: reorder · drag │: resize · click section: collapse · q: quit
```

## Contents

- [What it does](#what-it-does)
- [Requirements](#requirements)
- [Install](#install)
- [Log in](#log-in)
- [Usage](#usage)
- [Controls](#controls)
- [Configuration](#configuration)
- [Where your data lives](#where-your-data-lives)
- [Running inside herdr](#running-inside-herdr)
- [Development](#development)
- [Why Rust + Ratatui](#why-rust--ratatui)
- [License](#license)

## What it does

The full view mirrors how Asana looks in the browser, in three panes:

- **Left — Navigation.** *My Tasks* pinned on top, then your projects (see
  [Configuration](#configuration) for how the project list is chosen).
- **Middle — Tasks.** The selected list's tasks, grouped into collapsible
  **sections** in Asana's order, shown as a configurable, resizable table.
  Tasks show an open circle (`○`); a row click only opens its detail, so you can
  never accidentally complete a task. Tags and status-style custom fields are
  color-coded.
- **Right — Detail.** The selected task, mirroring Asana's web task pane: a
  **Mark complete** button (with optional confirmation) and a **Copy link**
  button, the title, an optional **description**, your configured **fields**
  (click to edit — a picklist for enum fields, inline text entry for text
  fields), the **subtasks** (click to open one), a scrollable
  **Comments / All activity** conversation with two tabs, and a **composer** to
  post a new comment.

You can also open a single task straight to the detail pane from its URL, which
is handy as a quick `$EDITOR`-style "open this task" command.

## Requirements

- **Rust** (stable). Install via [rustup](https://rustup.rs).
- An **Asana account** and a Personal Access Token.
- For secure token storage via `ninjasana login`, an OS keychain:
  **macOS Keychain**, **Windows Credential Manager**, or the **Linux Secret
  Service** (e.g. GNOME Keyring / KWallet). On headless boxes or in CI, skip
  `login` and use the `ASANA_ACCESS_TOKEN` environment variable instead
  (see [Log in](#log-in)).

## Install

Clone and build a release binary:

```sh
git clone https://github.com/chrisg32/Ninjasana.git
cd Ninjasana
cargo build --release
# binary at ./target/release/ninjasana
```

Or install it onto your `PATH` with Cargo:

```sh
cargo install --path .
# now `ninjasana` is available anywhere
```

## Log in

Ninjasana authenticates with an Asana **Personal Access Token (PAT)**.

1. Create a token at **https://app.asana.com/0/my-apps** → *Create new token*.
2. Run:

   ```sh
   ninjasana login
   ```

   Paste the token when prompted. Ninjasana validates it against the Asana API
   and stores it securely in your **OS keychain** (macOS Keychain, Windows
   Credential Manager, or the Linux Secret Service) under the service name
   `ninjasana`.

To sign out and remove the stored token:

```sh
ninjasana logout
```

### Environment variable (CI / headless / dev)

Instead of `login`, you can supply the token via the environment. This takes
precedence over the keychain and works on any platform:

```sh
export ASANA_ACCESS_TOKEN=your_token_here
ninjasana
```

A local `.env` file is also loaded automatically if present:

```sh
cp .env.example .env   # then edit .env
```

Without any token, Ninjasana runs in offline **demo mode** so you can explore
the interface.

## Usage

```sh
ninjasana                 # full three-pane Asana view
ninjasana <task_url>      # open just the detail pane for one task
ninjasana login           # store an Asana Personal Access Token
ninjasana logout          # remove the stored token
ninjasana --help          # full help
```

`<task_url>` accepts a full Asana task URL (e.g.
`https://app.asana.com/0/<project>/<task>` or the newer
`.../task/<task>` form) or a bare numeric task id.

## Controls

| Input | Action |
| --- | --- |
| Click a nav entry | Switch list (My Tasks / a project) |
| Click a section header | Collapse / expand the section (remembered between runs) |
| Click a task row | Open its detail pane (never completes the task) |
| **Drag a task row** | Reorder within / across sections |
| **Drag a column divider** (`│`) | Resize that column |
| Scroll wheel | Scroll whichever region is under the cursor (task list, description/fields, or conversation) |
| ↑ / ↓ or `j` / `k` | Move task selection |
| Click **Mark complete** | Complete the task (asks first if `confirm_complete`) |
| Click **Copy Link** | Copy the task's URL to the clipboard |
| Click **Comments** / **All activity** | Switch the conversation tab |
| Click a **field** | Edit it — picklist for enum fields, text entry for text fields |
| Click a **subtask** | Open that subtask in the detail pane |
| Click the **composer** and type | Add a comment (Enter sends, Esc cancels) |
| Click **Quit** / `q` / `Esc` | Exit |

In the confirmation dialog: `y` / Enter confirms, `n` / Esc cancels. In a
picklist: click an option, or Esc to dismiss.

## Configuration

Configuration lives at `~/.config/ninjasana/config.toml` (honoring
`XDG_CONFIG_HOME`). A generic default is written automatically on first run, so
you can just edit it.

```toml
# Columns shown in the task table, in order. Built-in columns:
#   "name", "due_date", "assignee", "projects", "tags", "completed"
# Custom fields use a "custom:" prefix with the exact Asana field name:
#   "custom:Dev Status v2"
columns = ["name", "due_date", "assignee", "projects", "tags"]

# Which projects appear in the navigation pane. Either a mode...
#   "favorites" — your favorited projects, in sidebar order (default)
#   "member"    — every project you're a member of
# ...or an explicit, ordered list of project names to show exactly those:
#   projects = ["ISMS", "Sprint - Maximilian", "Software Department"]
projects = "favorites"

# Show the top header bar and bottom status bar. Applies in every mode — turn
# them off (e.g. when opening a single task with `ninjasana <task_url>`) to use
# the full height of the terminal.
show_header = true
show_footer = true

# The task detail pane (right side).
[detail]
show_description = true    # show the task description?
confirm_complete = true    # confirm before marking a task complete?
# Fields listed under the description, in order — same tokens as `columns`:
fields = ["assignee", "due_date", "custom:Dev Status v2"]
```

### Columns

Each entry in `columns` is either a built-in (`name`, `due_date`, `assignee`,
`projects`, `tags`, `completed`) or a custom field referenced by its exact Asana
name with a `custom:` prefix. Custom fields are referenced by name so nothing
workspace-specific is hardcoded in the binary. The `name` column flexes to fill
remaining width; the others have fixed widths you can adjust by dragging their
dividers.

### Projects

Asana's public API does **not** expose the web sidebar's curated "Projects"
list, so `projects` offers three options:

- `"favorites"` — your favorited projects, returned in sidebar order. The
  closest single-call match to the web sidebar.
- `"member"` — every (non-archived) project you're a member of. A superset of
  the sidebar.
- An explicit **list of project names** — reproduces a curated sidebar exactly.
  Names are matched case-insensitively against all projects in your workspace
  and shown in the order you list them.

### Detail pane

The `[detail]` table controls the right-hand task pane:

- `show_description` — whether the task description is shown.
- `confirm_complete` — whether **Mark complete** asks before completing.
- `fields` — the fields listed under the description, in order, using the same
  tokens as `columns` (built-ins and `custom:<field>`). The title is always
  shown; subtasks and the Comments / All activity tabs always follow. Enum and
  text custom fields are editable in place (other field types are read-only for
  now).

## Where your data lives

| What | Where |
| --- | --- |
| Asana token | OS keychain (service `ninjasana`), or `ASANA_ACCESS_TOKEN` |
| Configuration | `~/.config/ninjasana/config.toml` |
| UI state (section collapse, column widths) | `~/.config/ninjasana/state.json` |

Your token is never written to the config or state files. Section collapse
state and resized column widths persist across runs because Asana's API does not
expose them.

## Running inside herdr

[herdr](https://herdr.dev) embeds any terminal program in a PTY pane, so
Ninjasana runs inside it like any other mouse-aware TUI — clicks, drags, scroll,
and the alternate screen all behave. Matching herdr's own stack (Ratatui +
crossterm) keeps the mouse/escape-sequence handling consistent on both sides of
the PTY boundary.

## Development

```sh
cargo build          # debug build
cargo run            # run it
cargo test           # unit tests
cargo clippy         # lints
cargo run -- login   # pass args through cargo with `--`
```

The code is small and layered:

- `cli.rs` — argument parsing and task-URL handling.
- `commands.rs` — the non-TUI `login` / `logout` commands.
- `config.rs` / `credentials.rs` — token resolution and Keychain storage.
- `settings.rs` — the config file (columns, project source).
- `state.rs` — persisted UI state (collapse, column widths).
- `asana.rs` — the async Asana REST client (`reqwest` + `serde`).
- `event.rs` — the async event bus (keyboard, mouse, ticks, API results).
- `app.rs` — application state, the event loop, and mouse hit-testing.
- `ui.rs` — Ratatui rendering; every clickable element registers its rectangle.

## Why Rust + Ratatui

The product is bespoke mouse interaction, so we want to own the screen geometry.
[Ratatui](https://ratatui.rs) is immediate-mode: every frame we compute the
layout rectangles and draw to them, and that same set of rectangles is what we
hit-test clicks, drags, and resizes against — one coordinate system, no separate
retained widget tree to keep in sync. It's also the stack herdr is built on.

The Asana API is reached directly over REST (`reqwest` + `serde`); Asana ships
no official Rust SDK, and its responses map cleanly onto `serde` types.

## License

MIT — see [LICENSE](LICENSE).
