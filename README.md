<h1 align="center">
  <img src="crates/app/assets/icon.png" alt="Codemux icon" width="128"><br>
  Codemux
</h1>

<p align="center">
  A desktop session manager for Claude Code.<br>
  All your sessions in one window, and they come back after a restart.
</p>

![Codemux showing sessions grouped into workspaces in the sidebar, with Claude Code running in a tab](docs/screenshot.png)

> ⚠️ **Note:** Codemux is a personal tool. I build it for my own daily use, so it follows my own needs and habits.
> I do not plan to support other setups or take feature requests.
> If you want to use it, please fork it and change it to fit your needs with your own AI coding agent (such as Claude Code).

One window: a sidebar with all your Claude Code sessions on the left, and the session's terminal on the right.

## Why

Claude Code saves every session in `~/.claude/projects/`, and you can reopen one with `claude --resume <id>`.
But after a restart, your terminal tabs are gone, and it is hard to remember which sessions were open.

Codemux keeps that list for you:

- Shows every session in a sidebar, grouped by time, and in workspaces you create.
- Shows which sessions are working, which are waiting for input, and which are old.
- Runs Claude Code in a real terminal inside the app.
- After a restart, the sessions that were open come back, and you can resume each with one click.
- Works with several Claude accounts. When one account hits its limit, continue the same session with another.

Codemux never sends your session files anywhere and never calls the Claude API.

## Build and run

You need [Rust](https://rustup.rs) (stable) and the `claude` CLI on your `PATH`.

```sh
cargo run --release -p codemux
```

On macOS, install it as an app, so you can open it from Spotlight (Cmd+Space, type "Codemux"), Finder, or Launchpad:

```sh
./scripts/install-macos.sh
```

On Linux, install it for your user, so it shows in the app menu:

```sh
./scripts/install-linux.sh
```

On Windows, install it from PowerShell, so it shows in the Start menu:

```powershell
.\scripts\install-windows.ps1
```

Run the script again after you pull changes, to update the app.

Run the tests:

```sh
cargo test
```

### Linux and Windows

I only use macOS, so Linux and Windows are not tested. Build Codemux from source
with the install script above. You may find small problems there.

Notes:

- **Shortcuts:** macOS uses ⌘ (for example ⌘N). Linux and Windows use Ctrl+Shift
  (for example Ctrl+Shift+N, and Ctrl+Shift+C / Ctrl+Shift+V to copy and paste),
  so plain Ctrl keys still go to the terminal.
- **Linux:** you need a desktop with Vulkan or OpenGL drivers, and `libxkbcommon`
  (most desktops have both). Shells and `claude` start through your login shell (`$SHELL`).
- **Windows:** new terminals open PowerShell. Claude sessions start with `cmd.exe /C claude`,
  so `claude` must be on your `PATH` (the native installer and npm both do this).

## Project layout

| Crate | What it does |
|---|---|
| `crates/core` | Reads session files, tracks status, stores app state. No UI. |
| `crates/terminal` | Terminal widget for [Iced](https://iced.rs), built on `alacritty_terminal`. |
| `crates/app` | The desktop app (sidebar, tabs, settings). |

See [SPEC.md](SPEC.md) for the full design.

## License

MIT. See [LICENSE](LICENSE) and [NOTICE](NOTICE). The terminal crate includes code from
[iced_term](https://github.com/Harzu/iced_term) (MIT).
