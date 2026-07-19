# Performance plan — hunkle

Investigation (measured on a 100k-file synthetic tree + a real 20k-file repo) found
these hot spots. This plan is the implementation checklist; each workstream is
independently testable. Numbers in parentheses are measured costs before the fix.

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
- `ChangesState` caches the `Node` tree keyed by a `files_fingerprint` stored on
  `RepositoryData` (computed in the load worker, so the check is O(1) on the UI
  thread). Toggles re-flatten only.
- Collect default-collapsed directories by walking the `Node` tree directly
  (replicating the chain-join rule) instead of a separate full flatten — removes
  the double build at startup.
- Accumulate descendant file counts in `Node` at build time; store the count on
  directory rows. `refresh_diff` reads the row count instead of scanning
  `repo.files` with `starts_with`.
- Same descendant count for worktree directory rows ("N changed files in x/").

### W2 — `git ls-files` instead of the filesystem walk (`git.rs`)
- Replace `repository_files` with `git ls-files -z --cached --others
  --exclude-standard` (single process, respects ignore rules).
- **Behavior change:** ignored files (e.g. `ignored/cache.txt`,
  `node_modules/...`) no longer appear in FILES or file search. Update the test
  at `git.rs` that asserts the old behavior.
- Keep output sorted (verify with `is_sorted`, sort only if needed).

### W3 — Change-status color map (`app/changes.rs`, `ui/changes.rs`)
- Build `HashMap<path, worst status code>` once per data change (rebuilt in
  `rebuild_worktree_rows`, i.e. on reload), replacing the per-row-per-frame
  linear scan in `explorer_file_color`.

### W4 — Styled-diff cache + hover throttle (`app/changes.rs`, `ui/changes.rs`, `main.rs`, `app.rs`)
- Cache the fully styled diff/source document in `ChangesState`, keyed by
  `(preview_generation, width, wrap)`. Per frame, slice the cached doc instead of
  re-tokenizing. Cache `display_count`, `hunk_rows`, `rendered_height` alongside.
- Docs larger than ~30k display lines fall back to the current per-frame window
  path to bound memory.
- Hunk-hover mouse moves only mark the frame dirty when the hovered hunk actually
  changes (same for action-menu hover).

### W5 — Per-frame hygiene (`ui/history.rs`, `ui/overlays.rs`, `app.rs`, `git.rs`)
- HISTORY list: construct items only for the visible slice (variable heights are
  handled with a small height-aware offset walk), mirroring `draw_graph`.
- Graph width: compute `max(commit.graph.len())` once at load, store on
  `RepositoryData`.
- Command overlay: borrow transcript lines (`Line<'a>` from `&str`) instead of
  `to_owned()` per line per frame.
- `change_counts()`: compute once per reload, cache in `ChangesState`.

### W6 — History cap + lane-clone-free graph layout (`git.rs`)
- Cap the graph feed: `git log --max-count=5000` (constant, documented). Branch
  history panel already caps at 200. Lazy pagination is future work.
- Rewrite `layout_graph` to mutate lanes in place (int/bool scratch vecs)
  instead of cloning `Vec<Option<String>>` twice per commit. Existing graph
  tests must pass unchanged.

### W7 — Idle backoff + downstream fingerprint skips (`repository_session.rs`, `app.rs`, `app/file_search.rs`)
- Status signature check backs off exponentially (800 ms → 10 s cap) while no
  change is detected; any key press or detected change resets to 800 ms.
- `FileSearch::reindex` skips work when the files fingerprint is unchanged.
- Explorer tree rebuild / change-color map already skip via W1/W3 fingerprints.

### W8 — Cap untracked-file diff reads (`git.rs`)
- `git::diff` for untracked files currently reads the whole file into memory.
  Read at most ~128 KiB for the preview; binary-detect on that sample; note
  truncation in the preview.

### W9 — Verification
- `cargo test` green (update tests that encode old behavior: ls-files ignore
  rules, row struct fields).
- `cargo clippy` clean.
- Re-run the synthetic 100k-file benchmark to confirm:
  explorer toggle < 5 ms, startup tree work < 10 ms, color lookup ~O(1)/frame.

## Out of scope (noted for later)
- Lazy pagination of the commit graph beyond the 5_000 cap.
- Splitting "status refresh" from "full reload" in the session layer (mitigated
  for now by W2 + fingerprint skips).
- Capping very large commit diffs (interacts with hunk staging; needs care).
