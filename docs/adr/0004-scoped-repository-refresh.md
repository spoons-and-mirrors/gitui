# ADR 0004: Refresh Repository Data by Facet

- Status: Accepted
- Date: 2026-07-20

## Context

Every successful operation previously rebuilt all repository data. Staging one file therefore reran status and diff statistics, enumerated every tracked, untracked, and ignored file, loaded branch history, laid out up to 5,000 Graph commits, listed refs, and checked host capability.

The aggregate `RepositoryData` is useful to consumers, but its loading policy gave cheap mutations the cost and invalidation surface of a full repository open.

## Decision

Keep `RepositoryData` as the application snapshot while loading updates as independent WORKTREE, FILES inventory, history, Graph, and refs facets. `RepositorySession` requests a `RefreshScope` based on the completed operation and atomically applies the returned facets to the current snapshot.

Use these policies:

- Staging mutations and editor completion refresh WORKTREE.
- File operations refresh WORKTREE and FILES inventory.
- Fetch refreshes history, Graph, and refs.
- Commit refreshes WORKTREE, history, Graph, and refs.
- Manual refreshes, arbitrary Git commands, and externally detected changes refresh everything because their effects are not known locally.
- Repository open still loads every facet concurrently.

Queued refreshes union their scopes so no requested facet is lost while another load is active. Existing generation and root checks continue to reject stale results.

## Consequences

- Routine staging avoids filesystem inventory and history/Graph queries.
- File operations avoid history and Graph queries.
- Fetch avoids worktree diff statistics and full file enumeration.
- Existing selection restoration and fingerprint-based tree rebuilding remain unchanged.
- A new operation must declare the facets it can invalidate.
- Unknown operations retain the safe full-refresh path.

## Rejected Alternatives

- Keeping full reloads was simple but increasingly expensive as repository data grew.
- Splitting `RepositoryData` across all consumers would create broad churn without improving refresh policy.
- Inferring changed facets from Git command text would be fragile; arbitrary commands use a full refresh instead.
