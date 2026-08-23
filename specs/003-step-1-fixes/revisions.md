# Revisions — 003-step-1-fixes

Numbered log of changes to the approved spec, applied after the fact. Each
entry states what changed, why, and what it supersedes. `spec.md` is not
rewritten in place — this file is the record of drift and its justification.

## 1. README updates are part of finishing a step, not a deferred task

**What changed:** from `003-step-1-fixes` onward, updating `README.md` to
match what a step actually shipped is part of that step's own commit
boundary — not a separate, later "docs pass" task that can drift out of
sync with the code for a full step (or, as happened here, a full spec
packet) before someone notices.

**Why:** this packet exists in part because step 1 (`002-binance-feed`)
shipped a working Binance feed while `README.md` kept describing step 0's
state — "None of that logic exists yet," "exit 0" — for an entire spec
packet's worth of history. Nothing enforced that the README moved when the
code did; it was left as an implicit follow-up and fell behind. A reviewer
reading the README first (which is the normal order) would form a wrong
belief about what the repo does before ever opening `src/`.

**Standing rule:** a step (or spec packet) is not "done" until its README
sections describing behaviour match the code actually merged, checked in
the same verification pass as the rest of that step's acceptance criteria
— not assumed to be handled by a later cleanup pass.

## 2. Never invent data presented as captured

**What changed:** `src/exchange/binance.rs`'s `DEPTH20_FIXTURE` doc comment
previously claimed the fixture was "constructed to match the documented
wire shape at <https://www.binance.com/en/binance-api> depth-stream
sample" — that URL is Binance's general marketing page, not a depth-stream
reference, and the fixture data itself is visibly synthetic (bids stepping
in exact `0.00001` increments, amounts in exact `0.25` increments; real
order books never look like this). The comment has been rewritten to state
plainly that the fixture is synthetic test data, not a real capture, and
carries a `TODO` naming the gap (no live network access to Binance from
this sandbox — confirmed by a timed-out `curl` to
`stream.binance.com:9443` while writing this packet) and how to close it
(`wscat` capture from a network that can reach Binance). The fixture
constant's actual bytes are unchanged, and so is the test that exercises
it — this entry fixes a false claim about provenance, not the test's
coverage.

**Why:** presenting invented data as based on real documentation misleads
a reviewer into trusting the parser has met real Binance output when it
hasn't — irregular decimal counts, short values like `"0"`, and uneven
gaps between levels are all absent from this fixture. That gap is real and
worth knowing about; papering over it with a comment implying research
that didn't happen is worse than leaving the gap visible.

**Standing rule:** never invent data that's presented as captured or
documentation-derived. If real data isn't reachable, say so plainly in the
comment — state that the fixture is synthetic and why — or ask the project
owner to capture one, rather than writing a comment that makes invented
data look like it came from a real source. This applies to any future
fixture in this project, not just this one.

**Outstanding gap (not resolved by this packet):** this sandbox still has
no live network access to Binance (`curl -m 8 https://stream.binance.com:9443`
times out — verified 2026-08-23, same result as the prior finding recorded
in `specs/002-binance-feed/revisions.md` entry 2). `DEPTH20_FIXTURE` is
still synthetic data; a real capture has not been substituted. The `TODO`
comment names this explicitly rather than claiming it's resolved.
