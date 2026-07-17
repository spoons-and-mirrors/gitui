# Git Panel

A focused Rust/Ratatui interface for the two Git views that matter most during everyday work:

- A directory-tree worktree panel for inspecting, staging, unstaging, and committing changes.
- An all-refs commit graph showing branches, remotes, tags, authors, dates, and hashes.

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
| `Space` | Stage or unstage selected entry |
| `a`, `u` | Stage all or unstage all |
| `c` | Enter a commit message; `Enter` commits |
| `r` | Refresh |
| `o` | Choose another repository |
| `?` | Help |
| `q` | Quit |

In the repository explorer, press `p` or `/` to type or paste a path directly. Relative paths start from the displayed directory; `~/...` paths are supported.

## Mouse

- Click header controls to switch views, refresh, choose a repository, or open help.
- Drag the divider between Worktree and Diff to resize either panel.
- Click rows to select them; click a file's right-aligned `S`/`W` badge or right-click its row to stage or unstage it.
- Use the wheel over Worktree, Diff, or Graph to scroll that surface.
- Click the Worktree `Stage all` checkbox to stage everything; click it again when checked to unstage everything.
- Click the blue-purple commit editor inside Worktree, type a message, and press `Enter` to commit.
- Click the repository path field to type, or click a directory/repository entry to navigate or open it.

## Scope

This first version stays deliberately small. It uses the installed Git executable so ordering, configuration, worktrees, refs, and repository formats behave like Git itself. The graph uses terminal-native Unicode rather than terminal-specific image protocols.
