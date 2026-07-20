# ADR 0003: Own Preview Layout in a Presentation Module

- Status: Accepted
- Date: 2026-07-20

## Context

Preview rendering must keep several calculations aligned: diff/source styling, commit-header filtering, hard wrapping, logical-to-rendered row indexes, large-file windows, scroll limits, and hunk locations. The cache lived in `ChangesState`, while `ui::changes` separately mutated it and reconstructed those mappings.

That split made layout changes risky. A difference between counting, wrapping, windowing, and hunk geometry could produce incorrect scrolling or actions attached to the wrong row.

## Decision

Introduce a deep `ui::preview` presentation module. `PreviewPresentation` owns cache identity, styled content, large-file windows, wrapped indexes, and rendered hunk geometry. Its input describes content, presentation mode, width, viewport height, wrapping, and hunk-selection state. Its output contains only the visible lines and rendered height needed by the panel renderer.

`ChangesState` owns one presentation instance because its lifetime follows the active preview, but the cache implementation and Ratatui lines are no longer part of general Changes state.

## Consequences

- Counting, styling, wrapping, windowing, and hunk positions share one owner.
- The DIFF panel renderer focuses on panel geometry, scrollbars, and controls.
- Large previews retain bounded styling work and scrolling beyond `u16` limits.
- Presentation invalidation remains tied to content generations and viewport identity.
- Git loading and application interaction state remain outside the presentation module.

## Rejected Alternatives

- Splitting only the wrapping helpers would leave cache and mapping policy distributed.
- Recomputing all styled lines each frame would simplify ownership but regress large-file performance.
- Moving preview loading into the module would combine Git/application policy with presentation concerns.
