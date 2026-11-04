# Long capture — open questions and verification notes

Working notes for the manual long-screenshot engine (`model/scroll_sheet`
+ `platform/scroll_capture` + `ui/scroll_bar`). Not user documentation:
everything here is an open design question, a tradeoff worth re-reading
before touching the matcher, or a measurement nobody has made yet.
**Delete an entry when it is resolved** — the reasoning that outlives the
fix belongs in a code comment next to the logic, which is where the rest
of this subsystem's war stories already live.

Current session invariants (covered by unit tests in
`model/scroll_sheet/tests.rs` and `platform/scroll_capture/session.rs`):

- a refused frame writes *nothing* (one position base, `stable_profile`);
- the ten-minute limit is a true idle timeout refreshed on every accepted
  fold and on resume after a pause;
- `SheetLimits::max_height` is scaled by the output's physical-to-logical
  ratio (bounded by a 512 MiB sheet memory ceiling) and hitting it
  surfaces `Phase::MaxLength` in the panel HUD (or delivers a localized
  partial notice when chrome-free) instead of silently truncating;
- the preview panel publishes `(well_rows, sample_width)` atomically from
  its output scale (`PreviewGeometry`), keeping both sharpness and scroll
  rate invariant across 1×, fractional, 2× and 3× displays;
- control and terminal events ride an unbounded channel while preview
  bitmaps self-throttle on queue depth, and `Preview::report` records
  phase changes only after delivery succeeds.

## Open design question: what the preview strip *is*

The panel is 254 logical px wide, so fitting the selection's full width
means shrinking it. One strip row covers about `254 / logical_width`
logical px — so a 1 200 logical-px selection advances at ~21 % of the
page's speed and a 1 000 logical-px one at ~25 %, independent of display
scale. That is geometry, not a bug: a thumbnail cannot move at the speed
of the thing it is a thumbnail of. Options if full-width thumbnailing ever
feels sluggish:

- **keep the whole-width thumbnail** (current) — you see the layout, the
  strip moves at a consistent fraction of the scroll rate;
- **crop at 1:1** (`k = 1`, sample width = `254 × scale`, centred) —
  pixel-identical to the page, so it moves *exactly* with the scroll, but
  only a slice of the width is visible;
- **widen the panel** into the free side strip (`plan_scroll_panel` already
  knows how much room there is) — halves the shrink, still not 1:1.

## Tradeoffs already documented in code

Re-read these before changing behaviour; the reasons are next to the logic:

- integer-only offsets — sub-pixel smooth scrolling can drop/duplicate one
  seam row per step (module header, `model/scroll_sheet.rs`)
- fixed elements in the *middle* of the frame are not detected, only
  edge-anchored bands
- collapsing toolbars keep their tallest extent; rows they expose are lost,
  not misplaced
- one contiguous canvas, no tiling — which is why the 512 MiB memory
  ceiling and height cap exist
- a refusal never writes: the cost is the band nothing could bridge, the
  alternative (re-anchoring on the last seen frame) corrupted a viewport of
  correct rows

## Not verified yet

- **screencopy roundtrip cost.** `MANUAL_POLL = 28 ms`
  (`platform/scroll_capture/session.rs`) sleeps only the *remainder*, so
  the real rate is `max(capture, 28 ms)`. If a frame takes 40 ms+, the
  loop is capture-bound and the step sizes are bigger than the fixtures
  assume. The loop logs `frame #N → … (NNms)` — read that first.
- **Texture churn.** One preview bitmap per accepted frame (~35/s): a new
  gpui texture per frame, the previous one evicted via `window.drop_image`.
  Never measured on a weak GPU.
- **Horizontal sessions.** `transpose_rgba` (`model/scroll_sheet.rs`)
  copies the whole frame once per push, cache-hostile, unmeasured.
- **Matcher cost.** 2.8 ms → 4.4 ms per 1200×700 frame in release after
  the evidence weighting (~16 % of a core at 35 fps); 94–124 ms in a debug
  build. A debug run makes every cadence decision here meaningless.
- **Real content.** Most matcher evidence comes from synthetic and
  font-rasterized fixtures. Real pages add scroll-triggered animation,
  lazy images and video.

## Fixtures worth knowing

`model/scroll_sheet/tests.rs` and `scene_tests.rs` (run with `--nocapture`
when tuning):

- `a_smooth_flick_of_a_real_page_folds_every_frame` — eased-out flick,
  fractional offsets, every frame must fold
- `a_sparse_page_stays_exact_whatever_the_matcher_refuses` — mostly-blank
  page; the case where flat rows used to outvote real ones
- `a_fast_flick_beyond_the_window_refuses_instead_of_shifting` — steps past
  the overlap budget must refuse, not shift
- `a_jump_beyond_the_window_refuses_and_keeps_the_canvas_byte_identical` /
  `a_stalled_session_resumes_exactly_after_one_nudge_back` — the refusal
  and recovery contracts
- `scroll_and_check_prefix` — the harness they share; the invariant it
  enforces (the sheet is always an exact prefix of the page) is the one to
  keep green

Numbers that justified the current shape, for the next person who wants to
change a threshold: one refused frame used to corrupt 475 of 1 864 canvas
rows, a fast-flick series left 41 % of the canvas wrong, and a sparse page
accepted `dy = -474` for a 272-row scroll.
