# ADR 0002: Centralize Repository Operation Transitions

- Status: Accepted
- Date: 2026-07-20

## Context

`RepositorySession` schedules repository loads, status checks, fetches, commits, commands, mutations, and file operations. Their active state was represented by six independent booleans, while each start method repeated a different compatibility check. Completion paths separately reset the matching flag.

Those rules intentionally permit some overlap. For example, commit and fetch may run together, an open may supersede another load, and a status result is rejected through root and baseline checks. A single global busy flag would therefore change behavior.

## Decision

`RepositorySession` owns one `OperationState` that defines whether an operation may start, marks successful transitions, records completion, and answers running-state queries.

The initial transition table preserves existing concurrency semantics exactly. Repository execution remains on the existing worker channels; this decision centralizes scheduling policy without introducing a generic runtime abstraction.

File operations share the mutation state because both require exclusive access under the same policy.

## Consequences

- Adding an operation requires an explicit scheduling decision in one place.
- Start and completion paths cannot drift through unrelated boolean checks.
- Existing concurrency exceptions are visible and regression-tested.
- Load generations and status baselines continue to reject stale results independently of operation state.
- Thread creation remains concrete until tests or additional execution backends justify a runtime adapter.

## Rejected Alternatives

- A single busy flag would unnecessarily serialize operations that currently overlap safely.
- A generic thread-pool or runtime interface would add mechanism without yet improving scheduling policy.
- Correcting existing concurrency asymmetries in the same refactor would mix behavioral changes with structural work.
