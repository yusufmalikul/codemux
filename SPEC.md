# Spec: Session Manager for Claude Code

> Working name: **codemux** (repo `codemux`). This document also calls it **"the app"**.
> Do not use the word "Claude" in the app name (it is an Anthropic trademark). Use "for Claude Code" in the description.

Status: draft v0.2 · 2026-10-13 (added "New terminal")

---

## 1. Problem

- In one day there can be 5–10 Claude Code sessions, each in a tmux tab (inside Ghostty).
- After macOS restarts, all tmux tabs are gone.
- The next day it is hard to remember which sessions were open and what they were about.

The session data is not actually lost. Claude Code saves every session in
`~/.claude/projects/<folder>/<session-id>.jsonl`, and a session can be opened again with
`claude --resume <session-id>`. Only **the terminals and their tab layout** are lost.

## 2. Goals

A desktop app with **one window**: a sidebar with the session list on the left (like Claude/ChatGPT),
and the Claude Code terminal on the right.

1. All sessions show in the sidebar: the active ones and the old ones.
2. Clear indicators: which one is working, which one is waiting for input, which one is old.
3. The Claude Code terminal runs **inside the app**.
4. After an OS restart, the sessions that were open show up again, and can be continued with one click.
5. Runs on **macOS, Windows, and Linux** from one codebase.
6. Open source.

## 3. Non-goals (v1)

- Not a replacement for a general terminal. The "New terminal" tab (section 11.5) does hold a normal shell,
  but its only purpose is to start Claude Code. No advanced terminal features (split, profiles, etc.).
- Does not change Claude Code session files or send them anywhere. Deleting happens only when the user asks
  (section 11.6).
- Does not call the Claude/Anthropic API directly.
- Does not support sessions inside WSL (see section 14).
- No sync between computers.

## 4. Technical decisions

| Topic | Decision | Reason |
|---|---|---|
| Language | Rust | One codebase for 3 OSes, fast, safe |
| UI framework | **Iced 0.14** (version pinned) | Can build low-level widgets (the terminal) on the GPU, has IME support, proven by COSMIC Terminal |
| Terminal emulator | **`alacritty_terminal` 0.26** | Mature, maintained by the Alacritty team, Apache-2.0 |
| PTY | `alacritty_terminal::tty` (ConPTY on Windows) | Already in the same package as the emulator |
| Terminal widget | **Our own crate** `crates/terminal`, started from the `iced_term` 0.8 code (MIT) | The terminal is the core of the app. We own the code instead of depending on a small project |
| Not Tauri | — | Request from the project owner |
| Look | Our own clean theme, consistent on 3 OSes, using the system font | The terminal takes ~80% of the screen and is never "native" in any app anyway |

## 5. Users and scenarios

**User:** a developer who runs many Claude Code sessions every day.

**Main scenarios:**

1. **Morning after a restart.** Open the app. The sidebar shows "Previously open" (yesterday's 7 sessions)
   and the rest of the history. Click a session → a terminal appears and runs `claude --resume <id>`.
2. **Start a new session.** Press ⌘N / Ctrl+Shift+N → pick a project folder → the new session runs in a new tab.
2b. **New terminal.** Press ⌘T / Ctrl+Shift+T → a normal shell opens. The user `cd`s to any folder and
   types `claude` themselves. The app detects it, and the tab becomes a session in the sidebar on its own.
   Once Claude Code writes an `ai-title`, the tab title changes too.
3. **Switch sessions.** Click another session in the sidebar. The previous session keeps running in the background.
4. **Know which one needs attention.** A session waiting for permission or an answer gets a yellow mark,
   and (optionally) shows an OS notification.
5. **Find an old session.** Type in the search box → matches title, folder, branch, and last prompt.

## 6. UI

```
┌──────────────────────┬─────────────────────────────────────────┐
│ 🔍 Search sessions... │  fix webhook retry          qismo-api   │
│ [+ Session][>_ Term]  │  fix/webhook · 2 hours ago   [■ Stop]   │
│                      ├─────────────────────────────────────────┤
│ ACTIVE                │                                         │
│ ● fix webhook retry   │        (Claude Code terminal)           │
│   qismo-api · 2m      │                                         │
│ ◐ review PR #812      │                                         │
│   sdk-api · waiting   │                                         │
│                      │                                         │
│ PREVIOUSLY OPEN       │                                         │
│ ○ macOS GUI app       │                                         │
│                      │                                         │
│ TODAY / YESTERDAY /   │                                         │
│ 7 DAYS / OLDER        │                                         │
└──────────────────────┴─────────────────────────────────────────┘
```

### 6.1 Sidebar

- Groups, from top to bottom:
  1. **Pinned**
  2. **Active**: has a terminal running in the app (including a "New terminal" tab, even if it has not
     started Claude yet), or detected running outside the app
  3. **Previously open**: was open when the app closed or the OS restarted
  4. **Today**, **Yesterday**, **Last 7 days**, **Last 30 days**, **Older**
     (based on the last activity time)
- Each row contains:
  - Status indicator (section 7)
  - Title (section 8.3)
  - Small line: project folder name · branch · relative time ("2m", "08:10", "yesterday")
- Right-click menu: Open, Pin/Unpin, Rename (only a local label in the app), Archive,
  Copy session ID, Open folder, Stop, Delete (section 11.6).
- Archived sessions are hidden, but can be shown with a filter.

### 6.2 Right area

- Header: title, folder, branch, last time, **Stop** button.
- Terminal: fills the area, and resizes with the window.
- If the session is not running yet: show a summary (title, last prompt, folder) and a
  **Continue** button (= `claude --resume`).

### 6.3 Shortcuts

| Action | macOS | Windows/Linux |
|---|---|---|
| New session (pick folder) | ⌘N | Ctrl+Shift+N |
| New terminal (normal shell) | ⌘T | Ctrl+Shift+T |
| Close tab | ⌘W | Ctrl+Shift+W |
| Search | ⌘K | Ctrl+K |
| Go to session 1–9 in the Active group | ⌘1–⌘9 | Ctrl+1–9 |
| Next/previous session | ⌘⇧] / ⌘⇧[ | Ctrl+Tab / Ctrl+Shift+Tab |
| Hide/show sidebar | ⌘\ | Ctrl+\ |

On Windows/Linux, app shortcuts use Ctrl+Shift, because Ctrl+N, Ctrl+T, and Ctrl+W are used by the shell and Claude Code.

App shortcuts must not hide shortcuts that Claude Code uses in the terminal
(for example Ctrl+C, Ctrl+R, Shift+Tab, Esc). If there is a clash, the terminal wins.

### 6.4 Theme

- Follows the OS light/dark mode.
- UI font: system font (SF Pro / Segoe UI / the Linux distro default font).
- Terminal font: configurable, default is the system monospace font.

## 7. Session status

| Icon | Status | Meaning | Data source |
|---|---|---|---|
| ● green | **Working** | Process runs in the app, and Claude is processing | Hook (section 9), fallback is new PTY output in the last 2 seconds |
| ◐ yellow | **Waiting for input** | Process runs, Claude waits for permission or an answer | `Notification` / `Stop` hook |
| ◉ blue | **Active outside the app** | Session runs in another terminal | The `.jsonl` file changed in the last 2 minutes **and** there is a `claude` process with the same `cwd` |
| ▸ | **Shell** | A "New terminal" tab that has not started Claude yet | Shell child process (section 11.5) |
| ○ gray | **Not running** | Old session, can be continued | Default |
| ✕ red | **Exited with error** | The process in the app stopped with a non-zero code | PTY exit code |

Rules:
- Status from the hook comes first. If the hook is not installed, use the fallback (PTY output and file changes).
- A session that is "active outside the app" **must not** be opened again in the app until the outside process stops,
  so one session never runs twice. The Continue button is replaced by a warning.

## 8. Data source: Claude Code session files

### 8.1 Location

| OS | Path |
|---|---|
| macOS / Linux | `~/.claude/projects/` |
| Windows | `%USERPROFILE%\.claude\projects\` |

If the `CLAUDE_CONFIG_DIR` variable is set, use that folder.

### 8.2 Fields used

Each line of a `.jsonl` file is one JSON object with a `type` field. The ones we use:

| `type` | Field | For |
|---|---|---|
| `user` / `assistant` | `sessionId`, `cwd`, `gitBranch`, `timestamp` | Folder, branch, time |
| `user` (not `isMeta`, not `isSidechain`) | `message` | First prompt (fallback title) |
| `ai-title` | `aiTitle` | Automatic title |
| `agent-name` | `agentName` | Name given by the user (`claude -n`) |
| `last-prompt` | `lastPrompt` | Small text in the sidebar and in search |
| `continued-in` | `continuedInSessionId` | Joins a continued session to its new ID |

### 8.3 Title order

1. Local name in the app (if the user renamed it)
2. `agentName`
3. Last `aiTitle`
4. The user's first prompt (cut to 80 characters)
5. "(untitled)"

### 8.4 Parser rules

- **Read-only**, with one exception: deleting a session when the user asks (section 11.6).
  The app never changes the content of a session file.
- This format is **not an official API** and can change. So:
  - The parser lives in one module (`core::sessions::parse`), with tests from sample files (fixtures).
  - Broken lines or unknown `type` values are skipped, not errors.
  - Missing fields do not crash the app.
- For big files, read **step by step**: keep the last position (offset) per file, and read
  only the new lines when the file changes.
- Skip session files that belong to subagents (sidechain).
- The index is cached (section 10), so the app opens fast.

### 8.5 Watching for changes

- The `notify` crate (FSEvents / ReadDirectoryChangesW / inotify).
- Events are grouped every 250 ms (debounce), so we do not process too often.

## 9. Claude Code hook (optional, for accurate status)

- The app **offers** to install the hook. It never installs it silently.
- Before installing, show the changes that will be written to `~/.claude/settings.json`, then ask for approval.
- Events: `SessionStart` → link the tab to the session, `UserPromptSubmit` → working, `Notification` → waiting for input, `Stop` → waiting for input/done.
- Every PTY the app creates gets the env `CODEMUX_TAB_ID=<id>`. The `claude` process in that tab inherits it,
  so the hook can write `{tab_id, session_id, cwd}`. This is the most accurate way for "New terminal" tabs,
  and it also handles `/clear` or `/resume` inside Claude, which change the session ID.
- The hook writes a small file to the app data folder: `status/<session-id>.json` containing `{state, ts}`.
- The hook must be very light (no network), and safe when the app is not running.
- There is a "Remove hook" button that puts `settings.json` back as it was.

## 10. App data

Stored in the per-OS data folder (`directories` crate):
- macOS: `~/Library/Application Support/<app>/`
- Windows: `%APPDATA%\<app>\`
- Linux: `~/.local/share/<app>/`

| File | Content |
|---|---|
| `state.json` | List of open sessions (tab order), selected session, window size, sidebar width |
| `sessions.json` | Per session: pin, archive, local name, notes |
| `index-cache.json` | Cached index result (safe to delete at any time) |
| `status/` | Status files from the hook |

- Save **atomically** (write to a temp file, then rename), so files do not break if the app dies suddenly.
- Every file has a `version` field, for format migration in later versions.

## 11. Session lifecycle

### 11.1 Resuming a session

1. The user clicks a session with status ○.
2. The app creates a PTY sized to the terminal area.
3. The command runs **through the user's login shell**, so `PATH` is the same as in a normal terminal:
   - macOS/Linux: `$SHELL -l -c 'exec claude --resume <id>'`, with `cwd` = the session folder
   - Windows: `claude.exe --resume <id>` (found through `PATH`), with `cwd` = the session folder
4. Environment:
   - Set `TERM=xterm-256color` and `COLORTERM=truecolor`
   - **Remove** `CLAUDE_CODE_*` variables inherited from the parent process.
     (From testing: an inherited session marker variable makes Claude Code turn off transcript saving.)
5. If the `cwd` folder no longer exists: show a message, and do not run anything.

### 11.2 New session

1. The user picks a folder.
2. The app creates a new UUID, then runs `claude --session-id <uuid>`.
   This way the app knows the session ID right away without guessing.
3. The session shows up in the Active group right away.

### 11.3 Closing and stopping

- **Stop:** send Ctrl+C twice. If the process has not stopped within 3 seconds, kill it. The session stays in the history.
- **Closing the app:** all running sessions are saved in `state.json` as "previously open",
  then their processes are stopped the same way.
- **OS restart or app crash:** `state.json` is saved on every change, so the list of open sessions stays safe.

### 11.4 After a restart

- The app reads `state.json`, then shows those sessions in the **Previously open** group.
- Sessions are **not** started automatically. A session starts only when clicked (project owner's decision).
- Optional: the app can be set to start on login.
- "New terminal" tabs that only hold a shell (no Claude) are not restored.
  Tabs that were running Claude are restored as sessions in "Previously open".

### 11.5 New terminal (automatic detection)

1. The user presses **New terminal**. The app opens the user's login shell (`$SHELL -l`, or `powershell.exe` on Windows)
   in the folder of the selected tab, or in the home folder.
2. While there is no Claude yet, the tab shows in the Active group as "Terminal · <folder>". The folder label follows `cd`
   (read from the shell process `cwd`).
3. Claude detection (fallback, without the hook), checked every ~1.5 seconds:
   - Look for a child process of the shell that looks like Claude Code: name `claude`/`claude.exe`,
     `argv[0]` = `claude`, path `.../claude/versions/...`, or the argument `@anthropic-ai/claude-code`.
   - Take that process's `cwd` and start time.
   - The session = the newest `.jsonl` file in `projects/<cwd folder>/` that changed after the process started,
     and is not used by another tab. Checked again every time, so `/clear` and `/resume` are detected too.
   - Weakness: two tabs that start Claude in the same folder at almost the same time can be swapped.
     The hook (section 9) removes this problem.
4. Before Claude writes anything, the tab shows as "claude · <folder>". Once there is an `ai-title`, the tab title
   uses it (title order from section 8.3).
5. When `claude` exits, the tab goes back to being a shell. The session stays in the history. Running `claude` again
   in the same tab links the tab to the new session.
6. When the shell exits (`exit`), the tab closes.

### 11.6 Deleting a session

1. The user selects a session, then presses **Delete**. The app asks for confirmation: "Delete permanently" or "Cancel".
2. What gets deleted: `<projects>/<folder>/<id>.jsonl` and the folder `<projects>/<folder>/<id>/` (subagent transcripts).
3. Safety checks:
   - The path must be exactly `<projects>/<folder>/<id>.jsonl` after resolving (no `..` or symlink that points outside).
     Anything else is refused.
   - A session running in an app tab cannot be deleted. The tab must be closed first.
   - (Not yet) Also refuse if the session is detected running outside the app (section 7).
4. After deleting, the session is gone from the sidebar and from "Previously open". It cannot be resumed anymore.

## 12. Terminal widget (`crates/terminal`)

Started from the `iced_term` 0.8 code (MIT). Attribution is in `NOTICE` and in the header of copied files.

### 12.1 Requirements from testing Claude Code v2.1.296

Claude Code turns on these modes: `1000`, `1002`, `1003`, `1006`, `1049`, `1004`, `2004`, `2031`.
Clicks in SGR format were shown to move the cursor in the input box.

| # | Requirement | State in iced_term 0.8 | What to build |
|---|---|---|---|
| T1 | Left click reported (SGR and legacy format) | ✅ Exists | Keep + test |
| T2 | Drag reported (`1002`) | ✅ Exists | Keep + test |
| T3 | **Scroll reported** as button 64/65 when mouse mode is on | ❌ Turned into ↑/↓ keys | **Required.** Report scroll. Use ↑/↓ only when mouse mode is off and alt screen + alternate scroll are on |
| T4 | **Shift + drag** (Option + drag on macOS) forces terminal selection | ❌ Missing | Add |
| T5 | Hover reported (`1003`) | ❌ Only while dragging | Report motion with no button. Send only when the cell changes |
| T6 | Right and middle click reported | ❌ Not used | Add |
| T7 | Focus events (`1004`): `ESC [ I` / `ESC [ O` | ❌ Missing | Add |
| T8 | IME (preedit + commit) | ❌ Missing | Connect to the Iced 0.14 input method. Show preedit at the cursor position |
| T9 | Bracketed paste (`2004`) | ✅ Exists, with protection | Keep + test |
| T10 | Selection: single / double (word) / triple (line) | ✅ Exists | Keep |
| T11 | Copy / paste | ✅ Exists | ⌘C/⌘V on macOS, Ctrl+Shift+C/V on Windows/Linux |
| T12 | Theme change notification (`2031`) | ❌ | Send a report when the OS light/dark mode changes |
| T13 | Links (OSC 8 and plain URLs) | ⚠️ Partly | ⌘/Ctrl + click opens the link |
| T14 | Truecolor, bold/italic/underline, wide characters (emoji, CJK) | ✅ From alacritty_terminal | Visual test |
| T15 | Resize follows the area size | ✅ | Send the new size to the PTY (debounce) |

### 12.2 Many terminals at once

- Every running session has one `Terminal` (PTY + emulator state) that stays alive even when it is not shown.
- Only the terminal on screen is drawn.
- Background terminals keep reading PTY output, so the buffer does not fill up and the Claude process is not blocked.

### 12.3 Performance targets

- 60 fps while Claude writes heavy output, on a Retina screen at ~200×60 cells.
- Keyboard input appears on screen in < 16 ms (one frame).
- 10 terminals running at once: app memory < 300 MB (not counting the `claude` processes).

## 13. Architecture

```
app/                         (repo)
├─ crates/
│  ├─ core/                  no UI, no Iced
│  │  ├─ sessions/           .jsonl parser, index, cache
│  │  ├─ watcher/            notify + debounce
│  │  ├─ status/             combines hook + PTY + outside processes
│  │  ├─ store/              state.json, sessions.json (atomic)
│  │  └─ launch/             builds the command + env per OS
│  ├─ terminal/              Iced widget + alacritty_terminal (section 12)
│  └─ app/                   Iced UI: sidebar, header, tabs, settings
├─ tests/e2e/                end-to-end tests (section 15)
├─ AGENTS.md                 guide for AI agents (pinned versions, rules)
├─ NOTICE                    attribution for iced_term and others
└─ .github/workflows/        CI + release
```

- Iced (Elm) pattern: `State` → `view` → `Message` → `update`.
- Heavy work (indexing, reading files, PTY) runs in a separate thread or task. Results are sent back as a `Message`.
- `core` does not depend on `terminal` or `app`, so it can be tested on its own and used by another UI later.

## 14. OS support

| | macOS | Windows | Linux |
|---|---|---|---|
| PTY | Unix PTY | ConPTY (Windows 10 1809+) | Unix PTY |
| Session folder | `~/.claude` | `%USERPROFILE%\.claude` | `~/.claude` |
| Command | through `$SHELL -l -c` | `claude.exe` directly | through `$SHELL -l -c` |
| Release package | `.dmg` (signed + notarized) | `.msi` (signed if a certificate is available) | AppImage + `.deb` |
| Start on login | LaunchAgent | Registry Run key | `.desktop` in autostart |

**WSL (not in v1):** sessions created inside WSL are stored on the Linux filesystem, not on Windows.
Supporting this needs its own design (reading `\\wsl$\...` and running through `wsl.exe`).

## 15. Testing

Follows the project rule: every bug starts with a failing test (e2e when possible), then gets fixed.

| Level | Content |
|---|---|
| Unit: `core` | Parser with `.jsonl` fixtures (cleaned of personal data), title order, time grouping, atomic save |
| Unit: `terminal` | For T1–T12: input event → the **exact bytes** sent to the PTY. Example: left click at (8, 24) with SGR → `ESC[<0;9;25M` |
| E2E: terminal | A small test program (`tests/e2e/fake-tui`) that turns on the same modes as Claude Code, then logs all input it receives. The test runs it in a real PTY, sends events through the widget, then checks the log. Deterministic, no network |
| E2E: app | Run the app with a fake `~/.claude` folder full of fixtures, then check the sidebar, resume (with a fake `claude`), and restore after restart |
| Manual (before release) | Mouse test script against the real `claude` (like `mousetest.py`), on 3 OSes |

CI runs all tests on macOS, Windows, and Linux.

## 16. Privacy and security

- All data stays on the user's computer. **No network, no telemetry.**
- Session files are only read, never changed. They are deleted only when the user asks, after confirmation.
- The hook is installed only with the user's approval, after its changes are shown.
- Test fixtures must not contain real data (code, secrets, emails).
- The README explains all of the above clearly.

## 17. License and attribution

- Proposed app license: **MIT OR Apache-2.0** (common in Rust projects).
- `iced_term`: MIT → must include its copyright and license text in `NOTICE`.
- `alacritty_terminal`: Apache-2.0 → compatible.
- **Do not** copy code from Zed (its terminal part is GPL licensed).

## 18. Milestones

| Milestone | Content | Done when |
|---|---|---|
| **M0: Terminal spike** | One Iced window + one terminal running `claude` | Claude Code works normally: click moves the cursor, scroll and paste work |
| **M1: Full terminal** | T1–T15 + byte tests + `fake-tui` e2e | All tests pass on 3 OSes |
| **M2: Index + sidebar** | Parser, watcher, grouped sidebar, search, resume | All local sessions show in < 1 second (with cache) |
| **M3: Many tabs + restore** | Many terminals, `state.json`, "Previously open" group, **New terminal + Claude detection** (11.5) | Restart the app → the session list comes back; `claude` typed in a New terminal shows up in the sidebar with its title |
| **M4: Status** | Fallback status + optional hook + outside process detection | Indicators match the real state |
| **M5: Release** | Packages for 3 OSes, signing, README, CONTRIBUTING | v0.1.0 released on GitHub |

### 18.1 Implementation status (2026-10-13): minimal version for testing

Done:
- Workspace `crates/core`, `crates/terminal` (from iced_term 0.8, now on alacritty_terminal 0.26), `crates/app`.
- Sidebar: Active, Previously open, history by time, search box.
- Delete session (11.6) with confirmation.
- New session (pick folder → `claude --session-id`), New terminal + Claude detection, Continue (`claude --resume`).
- Fallback status: ● working (output < 2 seconds ago), ◐ waiting, ▸ shell.
- `state.json` is saved atomically on every change.
- **T3 done**: scroll is reported as button 64/65 when mouse mode is on (SGR and legacy format),
  arrow keys only when mouse mode is off + alt screen + alternate scroll. There are 3 e2e tests with a real PTY.
- CPU fixes from profiling: theme colors are parsed once (not per cell per frame); the font is measured once
  (not per PTY event); back-to-back `Wakeup` events are merged; hidden tabs do not copy their screen;
  the history sidebar uses `lazy` (rebuilt only when its data changes).
  The grid is no longer copied: it is drawn straight from the terminal state (under a lock) only when a redraw is needed.
  Sync cost with a full scrollback went down from ~5.5 ms to < 0.001 ms per event.
- `core` unit tests: parser, incremental index, title order, real process detection, session matching, atomic save.

Not yet (put off on purpose):
- `notify` watcher (still polling every 1.5 seconds for now, and the index already reads step by step).
- Hook, detection of sessions outside the app, red error status, right-click menu, pin/archive/rename.
- Terminal fixes T4–T8, T12. Stop button with Ctrl+C (right now "Close" kills the PTY).
- `index-cache.json`, `fake-tui` e2e, CI, release packages.

**Ideas for v2:** per-session notes, automatic summaries, OS notifications, WSL support, split view (2 terminals side by side), cost stats from `cost-state`.

## 19. Open questions

1. App name and repo name?
2. License: the repo uses MIT right now (`LICENSE` file). Keep MIT, or MIT OR Apache-2.0?
3. OS notification when a session waits for input: v1 or v2?
4. Closing the app: stop all `claude` processes, or ask first if one is working?
5. Do we need a feature to export session content (for example to Markdown)?
