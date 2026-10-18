# Long capture — open defects and open questions

Working notes for the manual long-screenshot engine (`model/scroll_sheet.rs`
+ `platform/scroll_capture.rs` + `ui/scroll_bar.rs`). Not user
documentation: everything here is a defect that is still open, a tradeoff
worth re-reading before touching the matcher, or a measurement nobody has
made yet. **Delete an entry when it is fixed** — the reasoning that
outlives the fix belongs in a code comment next to the logic, which is
where the rest of this subsystem's war stories already live.

State as of `5ab0167`: a refused frame writes *nothing* (one position
base, `stable_profile`), both costs are weighted by per-row evidence, the
loop samples every 28 ms (sleeping only the remainder), the admissible
window is the overlap budget rather than 0.6 of the viewport, and the HUD
distinguishes "no overlap" (scroll slower) from "can't line up" (the area
is unstable). Verified by fixtures only — nothing in this file has been
run against a real compositor yet.

## Open defects

### 1. The ten-minute timeout is wall clock, and its message is wrong

`MANUAL_DEADLINE = 600 s` (`platform/scroll_capture.rs:222`) is measured
from session start (`:574`), not from the last accepted frame, so an
*actively scrolling* user is cut off at ten minutes. The failure string
says the opposite: "Nothing new was captured for ten minutes"
(`:588`). Fix: refresh the deadline on every accepted fold (a real idle
timeout), and make the string truthful. Note the string is inline English,
not a `locales/ui.yml` key — see defect 6.

### 2. The height cap is counted in physical rows, and hitting it is silent

`SheetLimits::max_height = 20_000` (`model/scroll_sheet.rs:144`) is
**physical** rows, so a 2× display gets ~10 000 logical px of page and a 3×
display ~6 700 — a long article reaches it. Memory at that point is
`20 000 × captured_width × 4` (192 MB for a 2 400 px-wide selection).
Hitting it is worse than the limit: `Fold::Refused(TooTall)` goes straight
to `deliver()` (`platform/scroll_capture.rs:392`) with only a log line,
so the user is handed a truncated long image that looks complete. Fix:
express the cap in logical px (or raise it and lower the capture width),
and surface the truncation in the panel instead of the log.

### 3. "Can't advance" does not say what to do

A refusal streak that outruns the search window looks terminal: the sheet
stops growing while the page keeps scrolling. It is not terminal — the
recovery is one nudge *back* inside the window, after which the skipped
page rows are still ahead and get captured normally
(`a_stalled_session_resumes_exactly_after_one_nudge_back` proves the
canvas stays byte-identical during the stall and reconstructs exactly
afterwards). The chip only says "Can't advance"
(`locales/ui.yml:231`), so the cure is
undiscoverable. Fix: say it in the chip ("scroll back a little to
resume"). Alternative, unbuilt: auto re-link with a visible splice.

### 4. HiDPI: the preview's sample width ignores the scale factor

`PREVIEW_DISPLAY_W = 254` (`model/scroll_sheet.rs:1697`) is logical and
`PREVIEW_SAMPLE_W = 480` (`:1703`) is physical, and
`preview_scale_for`/`preview_fit_rows` (`:1708`, `:1721`) mix the two.
On a 2× screen the strip's bitmap is therefore upsampled (soft text), and
the strip's motion *relative to the page* changes with the display scale,
which makes "does the preview follow my scroll" unanswerable across
machines. Fix: have the panel publish its sample width the way it already
publishes its well height (`preview_rows`), derived from the window scale
factor, and keep every consumer on that one number.

### 5. A dropped event desynchronises the HUD permanently

The engine's channel is `bounded(64)` (`platform/scroll_capture.rs:186`)
and both publishers record state *before* `try_send`:
`Preview::report` sets `self.phase` (`:548`) and `Preview::push` sets
`self.shown` (`:486`). If the send fails, the UI never learns about that
change and — because both are deduplicated — will not be told again until
the next *different* state. The bitmap case self-heals on the next
accepted frame; the phase case can leave a wrong chip indefinitely. Fix:
record the state only after a successful send (retry next tick), or make
the channel unbounded.

### 6. Session failure strings bypass the locale file

`fail_with_partial` callers pass inline English
(`platform/scroll_capture.rs:585`, `:634`, `:663`). Everything the
*panel* says goes through `locales/ui.yml`; these reach the user through
the "stopped early" notification, so they are user-visible text that
cannot be translated. Fix: move them to `scroll.fail.*` keys.

## Open design question: what the preview strip *is*

The panel is 254 logical px wide, so fitting the selection's full width
means shrinking it. One strip row covers about `254 / (pw · k)` logical px
(`k = ceil(capture_width / PREVIEW_SAMPLE_W)`, `pw = ceil(capture_width /
k)`) — call it `254 / capture_width`. On a 1× display the page itself
moves 1 logical px per captured row, so a 1 200 px-wide selection advances
at ~21 % of the page's speed and a 1 000 px-wide one at ~25 %; on a 2×
display both roughly double (defect 4 again). That is geometry, not a
bug: a thumbnail cannot move at the speed of the thing it is a thumbnail
of. Options:

- **keep the whole-width thumbnail** — you see the layout, the strip moves
  at a fraction of the scroll rate;
- **crop at 1:1** (`k = 1`, sample width = 254 × scale factor, centred) —
  pixel-identical to the page, so it moves *exactly* with the scroll and
  text stays sharp, but only a slice of the width is visible;
- **widen the panel** into the free side strip (`plan_scroll_panel` already
  knows how much room there is) — halves the shrink, still not 1:1.

Whichever wins, defect 4 has to be fixed in the same pass.

## Tradeoffs already documented in code

Re-read these before changing behaviour; the reasons are next to the logic:

- integer-only offsets — sub-pixel smooth scrolling can drop/duplicate one
  seam row per step (module header, `model/scroll_sheet.rs`)
- fixed elements in the *middle* of the frame are not detected, only
  edge-anchored bands
- collapsing toolbars keep their tallest extent; rows they expose are lost,
  not misplaced
- one contiguous canvas, no tiling — which is why the height cap exists
- a refusal never writes: the cost is the band nothing could bridge, the
  alternative (re-anchoring on the last seen frame) corrupted a viewport of
  correct rows

## Not verified yet

- **screencopy roundtrip cost.** Every cadence number above assumes the
  capture is cheap; `MANUAL_POLL = 28 ms`
  (`platform/scroll_capture.rs:219`) sleeps only the *remainder*, so the
  real rate is `max(capture, 28 ms)`. If a frame takes 40 ms+, the loop is
  capture-bound and the step sizes are bigger than the fixtures assume.
  The manual loop now logs `frame #N → … (NNms)` — read that first.
- **Texture churn.** One 750 KB preview bitmap per accepted frame (~35/s):
  a new gpui texture per frame, the previous one evicted via
  `window.drop_image`. Never measured on a weak GPU.
- **Horizontal sessions.** `transpose_rgba` (`model/scroll_sheet.rs:162`)
  copies the whole frame once per push, cache-hostile, unmeasured.
- **Matcher cost.** 2.8 ms → 4.4 ms per 1200×700 frame in release after
  the evidence weighting (~16 % of a core at 35 fps); 94–124 ms in a debug
  build. A debug run makes every cadence decision here meaningless.
- **Real content.** All matcher evidence comes from synthetic fixtures:
  per-pixel text ink, sparse paragraphs, fractional sub-pixel offsets. Real
  pages add scroll-triggered animation, lazy images, sticky headers and
  video, which no fixture models.

## Fixtures worth knowing

`model/scroll_sheet.rs` tests (run them with `--nocapture` when tuning):

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
