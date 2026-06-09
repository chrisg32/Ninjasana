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

## Status

Early scaffold. Working today:

- Terminal setup with mouse capture + panic-safe restore.
- An async event bus merging keyboard, mouse, ticks, and Asana API results.
- A clickable **zone** hit-testing system (`ZoneMap`).
- A three-pane layout (header / sidebar / task list / status bar) with clickable
  views, clickable task rows, a clickable Quit button, and scroll-wheel support.
- A thin async Asana client that authenticates via `/users/me` on startup.

The task list is demo data until the real Asana fetches are wired in.

## Getting started

```sh
# 1. Install Rust (rustup) if you haven't.
# 2. (Optional) connect a real account:
cp .env.example .env
#    then put an Asana Personal Access Token in .env

# 3. Run it.
cargo run
```

Without a token, Ninjasana runs in offline **demo mode** so you can click around.

### Controls

| Input | Action |
| --- | --- |
| Click a view | Switch the sidebar view |
| Click a task | Select it |
| Scroll wheel | Scroll the task list |
| ↑/↓ or `j`/`k` | Move selection |
| Click **Quit** / `q` / `Esc` | Exit |

## License

MIT — see [LICENSE](LICENSE).
