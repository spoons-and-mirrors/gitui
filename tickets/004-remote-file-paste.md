# Ticket 004: Remote File Paste

**Status:** Blocked by upstream Herdr support

**Blocked by:** Herdr must expose a secure client-side plugin invocation and file-transfer channel for the exact client attached to the originating pane. Current Herdr plugins run on the remote server and cannot read the local WSL/Windows clipboard or open a client-owned upload stream.

## What to Build

Let a user copy files or folders in Windows Explorer, focus Hunkle running through `herdr --remote` from WSL, choose a destination in Files, and press `Ctrl+V` to transfer the copied tree into the remote workspace.

Hunkle owns the user-facing feature: destination selection, acquisition state, conflict resolution, safe import, progress, refresh, and final selection. A Herdr plugin should own Windows clipboard discovery and transfer preparation once Herdr provides the minimal generic client-side plugin and upload primitives.

## Product Decisions

- Scope the first version to Windows Explorer file clipboard entries accessed from a Herdr client running inside WSL.
- Treat this as a Hunkle feature with a narrow Herdr dependency. Clipboard-specific behavior should live in a Herdr plugin rather than becoming a Hunkle-specific Herdr core feature.
- Extend Herdr core only with reusable primitives: invoke a plugin on the exact originating local client and provide a secure, bounded file-transfer channel to the remote server.
- Keep ordinary terminal text paste unchanged. File paste begins only from an explicit `Ctrl+V` while Files is visible and focused.
- Capture the workspace and destination before requesting clipboard contents. Late results from another workspace, repository generation, pane, or request must be rejected.
- Clicking blank space in Files explicitly selects the workspace root. Selecting a directory targets that directory; selecting a file targets its parent directory.
- Show indeterminate progress while Herdr acquires and transfers clipboard data, then measured progress while Hunkle imports staged entries.
- Support one active transfer at a time.
- Resolve collisions before writing. Offer Replace, Keep both, Skip, and Cancel, with an Apply to all option.
- Preserve relative paths, file bytes, hierarchy, empty directories, and executable bits when available. Do not promise Windows ACLs, alternate data streams, ownership, xattrs, hard links, or timestamps in the first version.
- Release Herdr's staged transfer lease after success, cancellation, or failure. Herdr TTL cleanup is a fallback, not the normal lifecycle.

## Required Herdr Boundary

- Route a request from a remote pane to the exact attached local client that originated it.
- Invoke an enabled client-side plugin entrypoint inside WSL.
- Let the plugin read Explorer `CF_HDROP` through a static noninteractive PowerShell command and map returned paths through direct `wslpath` calls.
- Stream files over a separate authenticated SSH/data channel. Never send file bytes through the pane PTY or terminal paste framing.
- Use a single-use upload capability that is never placed in argv, environment variables, logs, or rendered terminal output.
- Stage a bounded, immutable transfer outside the workspace and return only a versioned manifest, opaque lease id, staging root, ordered relative entries, total bytes, and expiry.
- Bind requests to the originating pane/client, enforce deadlines and quotas, reject replay, clean partial data, and make release idempotent.

## Hunkle Experience

- Blank Files space and the Files root header expose a semantic root destination rather than relying on inferred geometry.
- The selected destination remains visually clear before and throughout the transfer.
- `Ctrl+V` starts acquisition without blocking input or rendering.
- A progress overlay identifies the destination and current acquisition/import phase and supports cancellation.
- Conflicts are presented together after complete preflight rather than discovered during writes.
- Successful completion refreshes the full file inventory and selects the first imported top-level entry.
- Failure messages distinguish unavailable Herdr support, no file clipboard content, rejected activation, transfer failure, unsafe manifest, collision cancellation, import failure, and stale workspace context.

## Safety and Scope

- Hunkle, not Herdr, decides the final workspace destination and performs the import.
- Validate both the staged manifest and filesystem before writes. Reject absolute paths, traversal, `.git`, malformed UTF-8, duplicates, file/directory prefix contradictions, symlinks, and special files.
- Enforce limits for entry count, aggregate bytes, individual file size, path length, and nesting depth on both sides of the boundary.
- Resolve the destination beneath the active workspace without following symlinks. Reserve output paths atomically and avoid partial overwrite.
- Complete conflict planning before mutation. Track newly created entries and roll back only entries created by the failed import.
- Always invalidate and refresh Files after any import attempt that may have written data, including partial failure.

## Acceptance Criteria

- [ ] Herdr exposes a documented, versioned client-side plugin invocation and staged file-transfer contract usable from a remote pane.
- [ ] A Herdr plugin reads copied files and folders from Windows Explorer while the client runs in WSL, without requiring a native Windows Herdr client.
- [ ] Hunkle pastes into the selected directory, a selected file's parent, or the explicitly selected workspace root.
- [ ] Clipboard acquisition and network transfer do not block Hunkle and show an indeterminate state; import shows measured progress.
- [ ] Replace, Keep both, Skip, Cancel, and Apply to all produce deterministic preflight plans before writes.
- [ ] Files and nested folders arrive with correct names, hierarchy, contents, empty directories, and supported executable bits.
- [ ] Unsafe manifests, traversal, symlinks, special files, excessive transfers, replayed capabilities, and stale workspace results are rejected without writing outside the destination.
- [ ] Success, failure, and cancellation release the Herdr lease and refresh Files correctly.
- [ ] Focused protocol tests cover client/pane pinning, activation, timeout, disconnect, replay, release, quotas, and archive/parser adversarial cases.
- [ ] Hunkle tests cover root/directory/file-parent destinations, semantic hit targets, stale completion rejection, every conflict action, rollback, progress, cancellation, and refresh.
- [ ] A manual end-to-end test copies mixed files and nested folders from Windows Explorer through WSL `herdr --remote` into Hunkle on the VM.

## Not in This Ticket

- Copying remote files back to Windows.
- Native Windows Herdr remote support.
- macOS or Linux desktop clipboard discovery.
- Terminal drag-and-drop or textual path paste.
- Clipboard synchronization unrelated to an explicit Hunkle paste.
- Preserving Windows ACLs, alternate data streams, ownership, xattrs, hard links, or special files.
