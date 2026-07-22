# Ticket 003: Terminal PNG Preview

**Status:** Ready for implementation

**Blocked by:** None - begin with the WezTerm/Herdr tracer-bullet check described below.

## What to Build

Render a PNG selected in Files inside the existing preview pane instead of treating its bytes as source text. Use Kitty graphics when the terminal path confirms support so that the user sees the image's real pixels, scaled to the pane. Fall back to a true-color Unicode half-block reconstruction when native graphics are unavailable.

The verified target path is Hunkle through Herdr to WezTerm. Herdr already captures pane Kitty graphics and re-emits clipped, positioned Kitty images when its experimental graphics support is enabled, and WezTerm implements the Kitty image protocol. Before building the full preview, prove that exact installed path with a minimal Kitty PNG rendered inside a Herdr pane.

## Product Decisions

- Scope the first version to PNG files selected in Files. Preserve current text, diff, and rendered-Markdown behavior everywhere else.
- Use native Kitty graphics only after a positive capability response or explicit supported configuration. Do not infer support from `TERM=xterm-256color`, which is also the value visible inside Herdr.
- Use a Ratatui-compatible image library that provides Kitty and Unicode half-block backends if it supports the repository's Ratatui version and deterministic backend selection in tests. Do not implement a new terminal graphics protocol unless that integration proves unsuitable.
- Preserve image aspect ratio, account for terminal cell geometry, scale down to the preview body, and center the result without covering the header, borders, or adjacent panes.
- Treat image output as stateful overlay content. Replace or clear native placements when the selection, preview geometry, pane, interaction, workspace, suspend state, or application lifecycle changes.
- Decode PNGs away from the render loop. Bound source size and decoded dimensions so malformed or very large images cannot exhaust memory or stall navigation.
- Change preview loading to return typed text, image, or error content instead of forcing all file data into a string. Preserve the current generation and active-workspace checks so stale background results cannot replace the current selection.
- Cache decoded image content and regenerate presentation only when the image or preview geometry changes.
- Keep scrolling and Markdown/source controls out of image mode. Continue to show the selected path and read-only state in the preview header.
- Show an informative in-pane error for an unreadable, corrupt, unsupported, or excessive PNG. Never expose raw binary data or unsupported protocol bytes as text.

## Required Experience

- Selecting a valid PNG in Files replaces the text preview with an image preview.
- Through a compatible Herdr-to-WezTerm session, the preview uses the original decoded image pixels rather than character art.
- With Kitty graphics unavailable or disabled, the same file produces a recognizable true-color half-block reconstruction.
- Resizing Hunkle rescales and repositions the preview while preserving aspect ratio.
- Moving quickly between PNGs and text files always shows the current selection and leaves no stale image behind.
- Switching panes, interactions, or workspaces and exiting Hunkle removes all image placements cleanly.
- Loading and decoding remain asynchronous and do not interrupt navigation or input.

## Compatibility and Safety

- Herdr's Kitty graphics support is experimental and disabled by default. Native-pixel acceptance requires `[experimental] kitty_graphics = true` in Herdr and a newly attached client using WezTerm.
- WezTerm supports the Kitty image operations needed for static PNG transmission and placement, but its broader Kitty compatibility tracker remains open. The tracer-bullet and final manual acceptance test are therefore mandatory.
- Windows Terminal supports Sixel but not the Kitty output Herdr currently emits. It must receive the Unicode fallback unless Herdr gains a separate Sixel host backend.
- Capability detection must fail closed. An absent, malformed, or timed-out response selects the Unicode backend.
- Native graphics cleanup must also run on error and shutdown paths so the shell is restored without graphical artifacts.
- Keep decoded image memory bounded and release cached images when their workspace or selection is no longer active.

## Acceptance Criteria

- [ ] A minimal Kitty PNG displays as native pixels in the exact installed Hunkle-adjacent environment: a Herdr pane attached through WezTerm with experimental Kitty graphics enabled.
- [ ] Selecting a valid PNG in Files displays it in the preview body with its aspect ratio preserved.
- [ ] A positively detected Kitty path uses native image pixels; an unsupported, disabled, or inconclusive path uses the Unicode fallback without showing raw escape data.
- [ ] Wide, tall, tiny, transparent, and indexed-color PNG fixtures render within the pane without distortion or overflow.
- [ ] Resizing the terminal updates image size and placement without leaving stale pixels.
- [ ] Switching between two images, from image to text, away from Files, between workspaces, and out of Hunkle clears obsolete placements.
- [ ] A late asynchronous result cannot replace a newer selection or a different workspace's preview.
- [ ] Corrupt, unreadable, and excessive PNGs show useful errors and leave Hunkle responsive.
- [ ] Existing text, diff, source, Markdown, wrapping, scrolling, and large-preview behavior remain unchanged for non-image content.
- [ ] Full-application Ratatui surface tests force the deterministic Unicode backend and cover selection, scaling across pane shapes, resize, replacement, cleanup, errors, and stale-result rejection.
- [ ] Focused tests cover protocol selection and native-placement cleanup where the Ratatui test backend cannot observe host graphics.
- [ ] Manual WezTerm/Herdr acceptance covers native rendering, resize/repaint, rapid selection changes, suspend/resume if supported, and clean shutdown.
- [ ] A manual run with Herdr Kitty graphics disabled confirms that fallback output is readable and no protocol artifacts appear.

## Not in This Ticket

- JPEG, GIF, WebP, AVIF, SVG, video, or other media formats.
- Animation or playback.
- Image editing, cropping, exporting, or mutation.
- Rendering images from historical commits or binary diffs in Changes or Graph.
- Adding Sixel output to Herdr or native image support to Windows Terminal through Herdr's current Kitty-only path.
- Replacing Hunkle's existing source, diff, or rendered-Markdown presentation.
