# ADR 0001: Introduce Semantic Hit Targets Incrementally

- Status: Accepted
- Date: 2026-07-20

## Context

Hunkle historically stored rendered rectangles as individual fields on the
global `Regions` structure. Application mouse handlers then reconstructed
meaning from those rectangles, list offsets, and row heights. The repository
browser exposed this cost clearly: adding two-row pull-request cards required
application code to know their rendered height.

The same interaction also split keyboard behavior between `App` and
`app::repository_browser`, reducing locality and making additions touch input
routing, rendering, geometry, and integration tests.

## Decision

Rendering may register semantic hit targets alongside their rectangles. Input
routing asks which target occupies a point and does not infer item identity from
presentation geometry.

The repository browser is the tracer migration:

- It owns browser-specific key interpretation.
- It emits application effects for behavior outside its domain, such as closing
  or opening a branch in Graph.
- Its renderer registers targets for the overlay, list, tabs, and exact visible
  items.
- Global application code applies emitted effects and does not know browser row
  heights.

Other interactions will migrate only when changed or when the pattern removes
meaningful geometry coupling. This is not a requirement to rewrite all rectangle
handling immediately.

## Consequences

- Variable-height browser rows no longer leak into application input code.
- Browser keyboard behavior is testable through the browser module's interface.
- Global `Regions` loses browser-specific rectangle fields in favor of one
  semantic collection.
- Rendering still owns terminal geometry, while application routing still owns
  cross-domain effects.
- During incremental migration, semantic targets and legacy rectangle fields
  coexist.

## Rejected Alternatives

- Splitting every modal into a new file without changing its interface would
  create shallow modules and preserve the coupling.
- Migrating every interaction in one change would increase regression risk
  without validating the seam first.
- Keeping row-height arithmetic in `App` would continue coupling application
  behavior to presentation details.
