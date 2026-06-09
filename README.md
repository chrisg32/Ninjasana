# Ninjasana

A **mouse-native**, layout-friendly terminal UI for [Asana](https://asana.com).

Inspired by [herdr](https://herdr.dev) — and designed to run happily *inside* a
herdr pane. Where most TUIs are keyboard-first with mouse support bolted on,
Ninjasana treats the mouse as a first-class input: click views, click tasks,
scroll lists, and (soon) drag to reorder and right-click for context menus.

## Why Rust + Ratatui?

The whole product is bespoke mouse interaction, so we want to own the screen
geometry. [Ratatui](https://ratatui.rs) is immediate-mode: every frame we
compute the layout rectangles and draw to them. That same set of rectangles is
what we hit-test clicks against — one coordinate system, no separate retained
widget tree to keep in sync. It's also the stack herdr itself is built on
(Ratatui + crossterm), which keeps the mouse/escape-sequence model identical on
both sides of the PTY boundary.

The Asana API is reached directly over REST (`reqwest` + `serde`); Asana ships
no official Rust SDK, and its responses map cleanly onto `serde` types.

## Commands

```sh
ninjasana            # full three-pane Asana view
ninjasana <task_url> # open just the detail pane for one task (URL or bare id)
ninjasana login      # store an Asana Personal Access Token (validated, kept in the OS keychain)
ninjasana logout     # remove the stored token
```

The full view mirrors how Asana looks in the browser:

- **Left** — navigation: *My Tasks* pinned on top, then the projects you're a
  member of.
- **Middle** — the selected list's tasks, grouped into collapsible **sections**
  in Asana's order, shown as a configurable table (see [Configuration](#configuration)).
  **Drag a row** to reorder it within or across sections. Tasks show an open
  circle (`○`) and **cannot be completed by clicking** — a row click only opens
  its detail. Tags and status-style custom fields are color-coded.
- **Right** — task detail, shown only once a task is selected.

## Configuration

Columns are read from `~/.config/ninjasana/config.toml` (honoring
`XDG_CONFIG_HOME`); a generic default is written on first run. Set the columns
and their order:

```toml
# Built-ins: "name", "due_date", "assignee", "projects", "tags", "completed"
# Custom fields: "custom:<Asana field name>", e.g. "custom:Dev Status v2"
columns = ["name", "due_date", "custom:Dev Status v2", "tags", "projects"]

# Which projects appear in the nav pane. Either a mode...
#   "favorites" — your favorited projects, in sidebar order (default)
#   "member"    — every project you're a member of
# ...or an explicit, ordered list of project names:
#   projects = ["ISMS", "Sprint - Maximilian", "Software Department"]
projects = "favorites"
```

Custom fields are referenced by name so nothing workspace-specific is hardcoded
in the binary. Section collapse/expand state is remembered between runs (stored
in `~/.config/ninjasana/state.json`).

> Note: Asana's public API doesn't expose the web sidebar's curated "Projects"
> list directly. `"favorites"` returns your favorited projects in sidebar order,
> `"member"` returns the full set you belong to, and an explicit name list lets
> you reproduce a curated sidebar exactly (names are matched case-insensitively,
> in the order given).

## Status

Working today:

- `login` / `logout` with a Personal Access Token stored in the macOS Keychain.
- Live data: identity + workspace + projects on startup, My Tasks and per-project
  task lists, and full task detail (status, assignee, due date, notes, permalink).
- `ninjasana <task_url>` opens straight to the detail pane.
- Mouse-native foundation: clickable nav, clickable task rows, a clickable Quit
  button, scroll-wheel support, and panic-safe terminal restore — all built on a
  `ZoneMap` that hit-tests clicks against the same rectangles we lay out.
- An offline **demo mode** when no token is set, so you can click around.

Next up: right-click context menus and a few display refinements.

## Getting started

```sh
# 1. Install Rust (rustup) if you haven't.
cargo run -- login        # connect your account, or…
cargo run                 # …just run it (demo mode without a token)

# A token can also come from the environment instead of the keychain:
cp .env.example .env      # then put an Asana PAT in .env
```

### Controls

| Input | Action |
| --- | --- |
| Click a nav entry | Switch list (My Tasks / a project) |
| Click a section header | Collapse / expand the section |
| Click a task row | Open its detail pane (never completes the task) |
| **Drag a task row** | Reorder within / across sections |
| Scroll wheel | Scroll the task list |
| ↑/↓ or `j`/`k` | Move task selection |
| Click **Quit** / `q` / `Esc` | Exit |

## License

MIT — see [LICENSE](LICENSE).
