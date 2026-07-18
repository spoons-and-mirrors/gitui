# Git Panel

A focused Rust/Ratatui interface for the two Git views that matter most during everyday work:

- A collapsible worktree tree with compact directory chains for inspecting, staging, unstaging, and committing changes.
- An all-refs commit graph showing branches, remotes, tags, authors, dates, and hashes.
- Source-aware diffs with line numbers, syntax color, and tinted additions, deletions, and hunk headers.
- Nonblocking worktree refresh when files, the index, branches, or HEAD change outside GitUI.
- Automatic OpenCode theme matching, with Catppuccin Macchiato as the fallback.

## Run

Git and a recent Rust toolchain are required.

```sh
cargo run -p gitui
cargo run -p gitui -- /path/to/repository
```

Starting outside a repository opens the directory navigator automatically.

## Keys

| Key | Action |
|---|---|
| `1`, `2`, `Tab` | Changes, Graph, or switch view |
| `j`, `k` | Move selection |
| `g`, `G` | First or last row |
| `PageUp`, `PageDown` | Scroll the selected file's diff |
| `w` | Toggle line wrapping in the Diff panel |
| `h`, `l`, `Left`, `Right` | Collapse, expand, or navigate the worktree tree |
| `Enter` | Toggle the selected directory |
| `Space` | Stage or unstage selected entry |
| `a`, `u` | Stage all or unstage all |
| `c` | Focus the commit message editor |
| `Enter`, `Ctrl+Enter` | New commit-message line, create commit |
| `r` | Refresh |
| `o` | Choose another repository |
| `s` | Open settings |
| `?` | Help |
| `q` | Quit |

In the repository explorer, press `p` or `/` to type or paste a path directly. Relative paths start from the displayed directory; `~/...` paths are supported.

## Mouse

- Click header controls to switch views, refresh, choose a repository, or open help.
- Drag the divider between Worktree and Diff to resize either panel.
- Click a directory to expand or collapse it. Click a file's right-aligned checkbox or right-click its row to stage or unstage it.
- Use the wheel over Worktree, Diff, or Graph to scroll that surface.
- Click the Worktree `Stage all` checkbox to stage everything; click it again when checked to unstage everything.
- Click the commit editor inside Worktree, type a message, and press `Ctrl+Enter` to commit.
- Click the repository path field to type, or click a directory/repository entry to navigate or open it.

## Settings

Settings are saved to `$XDG_CONFIG_HOME/gitui/config`, or `~/.config/gitui/config` when `XDG_CONFIG_HOME` is unset. On Windows, GitUI uses `%APPDATA%\gitui\config`. Auto-fetch can periodically run `git fetch --all --prune` for the active repository without blocking the interface; its interval is configurable from 1 to 1440 minutes. The last manually selected Worktree width is stored as an exact terminal-column count.

## Theme

GitUI uses the active OpenCode TUI theme when OpenCode is installed. It follows OpenCode's `tui.json`/`tui.jsonc` selection first, then `~/.local/state/opencode/kv.json`, and supports all bundled OpenCode themes plus user and project themes under `opencode/themes/*.json`. If no usable theme is found, GitUI uses Catppuccin Macchiato.

## Scope

This first version stays deliberately small. It uses the installed Git executable so ordering, configuration, worktrees, refs, and repository formats behave like Git itself. The graph uses terminal-native Unicode rather than terminal-specific image protocols.
