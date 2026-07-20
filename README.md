# hunkle

- A collapsible worktree tree with per-file added/deleted line counts for inspecting, staging, unstaging, and committing changes.
- A switchable repository file tree that includes tracked, untracked, and Git-ignored content, with read-only, syntax-colored previews.
- Local workspaces for browsing, searching, and previewing directories that are not Git repositories.
- A resizable current-branch history shelf with HEAD, branch, remote, and tag decorations; selecting a commit shows its patch.
- A repository Actions menu for committing, pushing, fetching, pulling with rebase, and running non-interactive Git commands with captured output.
- An all-refs commit graph showing branches, remotes, tags, authors, dates, and hashes.
- A filterable repository browser for local and remote branches plus open GitHub pull requests and issues.
- Source-aware diffs with line numbers, syntax color, and tinted additions, deletions, and hunk headers.
- Nonblocking worktree refresh when files, the index, branches, or HEAD change outside hunkle.
- Automatic OpenCode theme matching, with Catppuccin Macchiato as the fallback.

## Run

A recent Rust toolchain is required. Git is required for repository status, history, staging, and repository actions. GitHub CLI (`gh`) is optional and supplies pull requests and issues in the repository browser when installed and authenticated. GitHub data is prefetched and cached in memory for 15 minutes.

```sh
cargo run -p hunkle
cargo run -p hunkle -- /path/to/repository
```

hunkle opens exactly the current or requested directory. When that directory is a Git repository root, Git status and history are available. Any other directory opens as a local file workspace with recursive file browsing, fuzzy search, and previews; it never climbs into an enclosing repository.

## Keys

| Key | Action |
|---|---|
| `1`, `2`, `Tab` | Changes, Graph, or switch view |
| `j`, `k` | Move selection; scroll oversized hunks by 10 rows |
| `Home`, `G` | First or last row |
| `PageUp`, `PageDown` | Scroll the selected file's diff |
| `w` | Toggle line wrapping in the Diff panel |
| `e`, `E` | Open the selected file in your editor, or configure the editor |
| `f` | Switch the left pane between Worktree and Files |
| `F3` | Fuzzy-search repository files from anywhere |
| `F2` | Rename the selected file or folder in Files |
| `Ctrl+Delete` | Delete the selected file or folder after confirmation |
| `h`, `l`, `Left`, `Right` | Navigate the tree; Right enters/stages in hunk mode and Left exits it |
| `Enter` | Toggle the selected directory |
| `Space` | Stage or unstage the selected entry, or stage the selected hunk |
| `Right`, `l` in hunk mode | Stage the selected hunk |
| `a`, `u` | Stage all or unstage all |
| `c` | Focus the commit message editor |
| `Enter`, `Ctrl+Enter` | New commit-message line, create commit |
| `Left`, `Right`, `Home`, `End` | Move within the commit message |
| `Ctrl+A` | Select the complete commit message |
| `Ctrl+Backspace`, `Alt+Backspace` | Delete the previous commit-message word |
| `r` | Refresh |
| `o` | Open Explorer |
| `b` | Browse branches, pull requests, and issues |
| `s` | Open settings |
| `x` | Open repository Actions |
| `g` | Open Git command |
| `?` | Help |
| `q` | Quit |

In Explorer, start typing a folder name, press `p` to search from an empty field, or `/` to start an absolute path. Search accepts fuzzy directory names, relative paths, absolute paths, and `~/...`; `Tab` accepts the best completion and `Enter` opens a repository or navigates into a directory. hunkle indexes directories under your home folder and common workspace mounts in the background.

## Mouse

- Click header controls to switch views, refresh, open Explorer, or open help.
- Drag the divider between Worktree and Diff to resize either panel.
- Drag the History section header vertically to resize the current-branch commit shelf.
- Click `x ACTIONS` above History to push, fetch, pull with rebase, or run a custom Git command.
- Click or scroll History to inspect a commit's patch; click a Worktree file to return to its current diff.
- Click a directory to expand or collapse it. Click a file's right-aligned checkbox or right-click its row to stage or unstage it.
- Click `WORKTREE` or `FILES` in the left header to switch modes; clicking a repository file previews its contents.
- Click `+` in the Files header to create a file or folder. Drag a Files entry onto a folder or the Files header to move it.
- The wheel pans Worktree and Files as viewports without changing the selected file; click a visible row to select it.
- Use the wheel over Diff or Graph to scroll that surface.
- Drag the one-column Diff scrollbar or click its track to move quickly through large patches.
- Click the Worktree `Stage all` checkbox to stage everything; click it again when checked to unstage everything.
- Click the commit editor inside Worktree, type a message, and press `Ctrl+Enter` to commit.
- Click the Explorer path field to type, or click a directory/repository entry to navigate or open it.
- Drag across visible text to select it and automatically copy it to the clipboard. In Files, hold `Shift` while dragging to select text instead of moving an entry. Selection stays within the panel where the drag starts.

## Settings

Settings are saved to `$XDG_CONFIG_HOME/hunkle/config`, or `~/.config/hunkle/config` when `XDG_CONFIG_HOME` is unset. On Windows, hunkle uses `%APPDATA%\hunkle\config`. Existing settings are loaded from the old `gitui` location when no hunkle config exists. The first `e` press asks for an editor command such as `nvim`, `micro`, or `code --wait`; hunkle saves it, suspends the TUI, and runs the editor interactively. Press `E` to change it later. Auto-fetch can periodically run `git fetch --all --prune` for the active repository without blocking the interface; its interval is configurable from 1 to 1440 minutes. The last manually selected Worktree width and History height are stored as exact terminal-cell counts.

## Theme

hunkle uses the active OpenCode TUI theme when OpenCode is installed. It follows OpenCode's `tui.json`/`tui.jsonc` selection first, then `~/.local/state/opencode/kv.json`, and supports all bundled OpenCode themes plus user and project themes under `opencode/themes/*.json`. If no usable theme is found, hunkle uses Catppuccin Macchiato.

## Architecture

The binary stays deliberately direct, with modules split by the behavior they own:

| Module | Responsibility |
|---|---|
| `main` | Terminal setup, cleanup, and event loop |
| `app` | Global input routing, workspace state, Git mutations, settings, and notices |
| `app::actions` | Repository Actions, command input, and captured results |
| `app::changes` | Changes-screen selection, navigation, and displayed content |
| `app::explorer` | Workspace discovery, navigation, and fuzzy search |
| `app::repository_browser` | Branch, pull-request, and issue interaction plus cached remote data |
| `repository_session` | Active workspace lifecycle, background operations, and scoped refreshes |
| `git` | Installed-Git commands, parsing, refreshable repository facets, and local workspaces |
| `ui::preview` | Stateful preview styling, wrapping, viewport windows, and hunk geometry |
| `selection` | Screen-cell selection, text extraction, and clipboard fallback |
| `tree` | Pure worktree and file-tree projection |
| `ui` | Rendering shell, header, and view dispatch |
| `ui::changes` | Worktree, Files, Diff, and commit workspace |
| `ui::history` | Current-branch history and all-refs graph |
| `ui::overlays` | Explorer, repository browser, settings, and help overlays |
| `ui::text` | Deterministic source and diff presentation |
| `theme` | Theme discovery, resolution, and palette data |

Keep Git command details in `git`, operation scheduling in `repository_session`, interaction decisions in `app`, and visual formatting in `ui`. Add another module only when it can own a cohesive behavior behind a smaller interface than the implementation it hides.

Custom commands run as Git arguments from the active repository with prompts and editors disabled. They do not invoke a shell, so pipes, redirects, and other shell syntax are never interpreted.

## Scope

This first version stays deliberately small. It uses the installed Git executable so ordering, configuration, worktrees, refs, and repository formats behave like Git itself. The graph uses terminal-native Unicode rather than terminal-specific image protocols.
