# Ticket 002: Visual Commit-Stack Editor

**Status:** Ready for implementation

**Blocked by:** None - can start immediately.

## What to Build

Add a visual commit-stack editor for rewriting the current branch between a chosen base and `HEAD`. Users can reorder, reword, squash, fixup, drop, and split commits; preview the resulting plan; apply it without an external interactive-rebase editor; resolve interruptions; and recover the original stack.

The completed workflow should make careful history shaping approachable without hiding that commit OIDs and branch history will be rewritten.

## Product Decisions

- Define the editable stack as the linear first-parent commits after the selected merge base through `HEAD`, displayed oldest first.
- Default the base using the same remote-default-branch resolution expected by Branch Review, while allowing explicit local or remote base selection.
- Reject stacks containing merge commits in the first implementation. Explain which commit prevents editing rather than flattening or rewriting merge topology implicitly.
- Require a clean worktree and index before entering apply. Do not autostash or silently move uncommitted changes.
- Warn when commits appear to have been published to an upstream or remote-tracking ref and require an additional explicit confirmation before rewriting them.
- Represent all edits as an in-memory plan until the user confirms Apply. Navigation, reordering, action changes, and message edits must not mutate the repository.
- Create a durable backup ref at the original `HEAD` before the first mutation. Show its name in the confirmation and result views and retain it after success until the user explicitly removes it through a future cleanup mechanism.
- Execute the plan non-interactively under Hunkle's control. Do not open Git's sequence editor or depend on an external editor.
- Model application as an exclusive, resumable repository operation with explicit planning, running, conflict, split, completed, aborted, and failed states.
- Refresh all repository facets affected by a rewrite and preserve selection using rewrite-produced old-to-new commit mappings rather than assuming OIDs remain stable.
- Keep the plan model compatible with future change baskets, but do not require change baskets for this ticket.

## Required Experience

- Open the editor from History or Graph with a sensible initial base and the current branch stack.
- Show each commit's action, subject, author, date, short OID, and changed-file/line summary. Selecting a commit exposes its patch.
- Support keyboard and mouse reordering, plus explicit Pick, Reword, Squash, Fixup, Drop, and Split actions.
- Validate the plan continuously. For example, the first commit cannot squash or fix up into a nonexistent predecessor, and a plan cannot drop every commit without an explicit destructive confirmation.
- Edit complete commit messages for Reword and allow a message to be chosen for Squash. Fixup discards the fixup commit's message by default.
- Show a final plan preview that explains the old stack, planned order/actions, target base, dirty/published checks, backup ref, and expected destructive effects before Apply is enabled.
- During application, show progress against the plan and prevent incompatible repository operations.
- If Git stops on conflicts, show the conflicted files and expose Continue, Skip when valid, and Abort. Users can return to Hunkle's existing Changes and Files tools to resolve and stage conflict results before continuing.
- If Git stops for Split, expose the selected commit's changes through the existing Changes and hunk-staging workflow. Require the user to create at least two replacement commits before continuing the remaining plan, with a clear way to abort.
- Detect an interrupted Hunkle-managed rewrite when opening the repository or restarting Hunkle. Offer Resume or Abort instead of starting an unrelated mutation.
- On success, show the rewritten commits and preserve access to the backup ref. On failure, report captured Git output and retain enough operation state to resume or abort safely.

## Safety and Recovery

- Never begin mutation without a verified clean worktree/index, valid base, validated plan, and successfully created backup ref.
- Re-check repository root, generation, base, original HEAD, cleanliness, and operation compatibility immediately before applying the plan.
- Abort must use Git's operation-aware abort path and verify that the original branch, worktree, and index are restored. If automatic restoration cannot be verified, stop and present the backup ref and recovery commands rather than claiming success.
- Failed commands may still leave sequencer or conflict state. Refresh and detect repository state after every completion, including failures.
- Quitting or switching repositories during an active rewrite must require confirmation and leave recoverable durable state.
- Do not run arbitrary shell text. Generated Git commands, sequence data, messages, and temporary paths must be passed as structured arguments/data.

## Acceptance Criteria

- [ ] A clean linear local stack can be opened, reordered, reworded, squashed, fixed up, dropped, and applied from the TUI.
- [ ] No repository mutation occurs before the final Apply confirmation.
- [ ] Plan validation prevents invalid action sequences and clearly explains corrections.
- [ ] Applying always creates and reports a backup ref pointing to the original `HEAD`.
- [ ] Dirty worktrees/indexes, detached or unborn HEAD, missing merge bases, shallow history, merge commits, and changed HEAD/base races block application with useful messages.
- [ ] Published commits trigger a stronger confirmation and are never rewritten silently.
- [ ] Conflicted rewrites can be inspected, resolved, continued, skipped when Git permits, or aborted from Hunkle.
- [ ] Split pauses on the chosen commit and uses Hunkle's file/hunk staging flow to produce at least two replacement commits before resuming.
- [ ] Hunkle detects and can resume or abort an interrupted managed rewrite after restart.
- [ ] Successful completion refreshes worktree, file inventory when needed, History, Graph, and refs, with selection restored to the corresponding rewritten commit where possible.
- [ ] Abort restores the original stack in normal conflict and split scenarios; the backup ref remains available if verification fails.
- [ ] Real-Git tests cover every action individually, mixed plans, message preservation, dirty-state rejection, published warnings, conflicts, split flow, interruption/restart, abort, stale-head rejection, and backup recovery.
- [ ] Ratatui surface tests cover plan editing, patch inspection, validation, confirmation, progress, conflict controls, split controls, completion, and failure/recovery states.

## Not in This Ticket

- Rewriting merge commits or preserving arbitrary merge topology.
- Force-pushing rewritten history.
- Automatically stashing dirty work.
- Automatically deleting backup refs.
- Change baskets as a separate planning abstraction.
