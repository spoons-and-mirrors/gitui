# Performance plan - hunkle

Investigation (measured on a 100k-file synthetic tree + a real 20k-file repo) found
these hot spots. This plan is the implementation checklist; each workstream is
independently testable. Numbers in parentheses are measured costs before the fix.

## Implementation status

W1-W8 are implemented. Verification completed with all 70 tests passing and
Clippy clean with warnings denied. A release-mode benchmark against the implemented
tree measured a 50.1 ms one-time construction and 16.1 us average cached flatten
for a default-collapsed 100k-file tree, down from the 45-90 ms full rebuild on
every toggle.

## Confirmed baseline costs

| Hot spot | Measured |
|---|---|
| `build_file_tree` full rebuild, 100k files | ~45–90 ms per toggle, ~130 ms at startup (built twice) |
| `repository_files` filesystem walk | ~35 ms / 20k files, runs on every reload, ignores `.gitignore` |
| `explorer_file_color` linear scan | ~0.8 ms/frame (50 rows × 5k changes), grows linearly |
| Directory-row file count (`starts_with` scan) | ~0.4 ms per selection change at 100k files |
| `git log` (all refs, no cap) + `layout_graph` lane clones | unbounded; 2 String-vec clones per commit |
| `git status` signature check | every 800 ms forever, 10–40 ms per run |
| Diff pane: full-diff scans + per-token allocations | every frame; wrap mode restyles the whole document per frame |

## Workstreams

### W1 — Two-phase explorer tree + descendant counts (`tree.rs`, `app/changes.rs`)
- Split `build_file_tree` into `build_node_tree(files) -> Node` (expensive, cached)
  and `flatten_file_tree(&Node, collapsed) -> rows` (cheap, per toggle).
- `ChangesState` caches the file tree for both Git and local workspaces, keyed by
  a `files_fingerprint` stored on
  `RepositoryData` (computed in the load worker, so the check is O(1) on the UI
  thread). Toggles re-flatten only.
- Collect default-collapsed directories by walking the `Node` tree directly
  (replicating the chain-join rule) instead of a separate full flatten — removes
  the double build at startup.
- Accumulate descendant file counts in `Node` at build time; store the count on
  directory rows. `refresh_diff` reads the row count instead of scanning
  `repo.files` with `starts_with`.
- Same descendant count for worktree directory rows ("N changed files in x/").
  Counts are section-specific so staged and unstaged directory previews remain
  accurate. Cache the staged/unstaged worktree trees independently of collapse
  toggles.

### W2 — `git ls-files` instead of the filesystem walk (`git.rs`)
- For Git repositories only, replace `repository_files` with `git ls-files -z
  --cached --others --exclude-standard` (single process, respects ignore
  rules). Keep the filesystem walker for local workspaces, including directories
  nested inside an enclosing Git repository.
- Exclude tracked files deleted from disk so FILES keeps its current contract of
  listing previewable filesystem entries.
- **Behavior change:** ignored files (e.g. `ignored/cache.txt`,
  `node_modules/...`) no longer appear in FILES or file search. Update the test
  at `git.rs` that asserts the old behavior.
- Keep output sorted (verify with `is_sorted`, sort only if needed).

### W3 — Change-status color map (`app/changes.rs`, `ui/changes.rs`)
- Build `HashMap<path, worst status code>` once per changes fingerprint,
  replacing the per-row-per-frame linear scan in `explorer_file_color`.
  Directory toggles must not rebuild the map.

### W4 — Styled-diff cache + hover throttle (`app/changes.rs`, `ui/changes.rs`, `main.rs`, `app.rs`)
- Cache the fully styled diff/source document in `ChangesState`, keyed by
  `(preview_content_generation, path, kind, width)`. Request generation alone is
  unsafe because loading text and the accepted async result share a request ID.
  Per frame, slice the cached doc instead of re-tokenizing. Cache
  `display_count`, `hunk_rows`, and `rendered_height` alongside.
- Docs larger than ~30k display lines stay on a bounded, viewport-oriented path;
  wrapped rendering must not style or retain the complete oversized document.
- Hunk-hover mouse moves only mark the frame dirty when the hovered hunk actually
  changes (same for action-menu hover).

### W5 — Per-frame hygiene (`ui/history.rs`, `ui/overlays.rs`, `app.rs`, `git.rs`)
- HISTORY list: construct items only for the visible slice (variable heights are
  handled with a small height-aware offset walk), mirroring `draw_graph`.
- Graph width: compute `max(commit.graph.len())` once at load, store on
  `RepositoryData`.
- Command overlay: borrow transcript lines (`Line<'a>` from `&str`) instead of
  `to_owned()` per line per frame.
- Cache repository-derived graph width and staged/unstaged counts on
  `RepositoryData`, so all consumers share one calculation.

### W6 — History cap + lane-clone-free graph layout (`git.rs`)
- Request 5,001 graph commits, retain 5,000, and surface truncation in the graph
  UI. Branch history already caps at 200. Lazy pagination is future work.
- Intern OIDs to integer lane IDs and avoid cloning lane `String`s per commit.
  Add exact linear/branch/merge/cutoff fixtures before changing the layout.

### W7 — Idle backoff + downstream fingerprint skips (`repository_session.rs`, `app.rs`, `app/file_search.rs`)
- Status signature check backs off exponentially (800 ms → 10 s cap) while no
  change is detected; any key press or detected change resets to 800 ms.
- `FileSearch::reindex` skips work when the files fingerprint is unchanged.
- Explorer tree rebuild and change-color map skip via separate files and changes
  fingerprints. Local workspace reloads receive the same files fingerprint.

### W8 — Cap untracked-file diff reads (`git.rs`)
- `git::diff` for untracked files currently reads the whole file into memory.
  Read at most ~128 KiB plus one sentinel byte for the preview; binary-detect on
  that sample, cap displayed lines, and clearly note truncation.

### W9 — Verification
- [x] `cargo test`: 70 tests pass, including wrapped-gutter, large wrapped-preview, sparse-checkout,
  ignore-rule, graph-layout, polling, and row tests.
- [x] `cargo clippy --all-targets -- -D warnings`: clean.
- [x] Release-mode 100k-file benchmark: one 50.1 ms tree construction per files
  fingerprint; default-collapsed cached flatten averages 16.1 us per toggle.
- [x] Explorer status colors use an O(1) hash-map lookup per visible row.

## Out of scope (noted for later)
- Lazy pagination of the commit graph beyond the 5_000 cap.
- Splitting "status refresh" from "full reload" in the session layer (mitigated
  for now by W2 + fingerprint skips).
- Capping very large commit diffs (interacts with hunk staging; needs care).
