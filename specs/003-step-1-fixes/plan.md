# Plan: 003-step-1-fixes

## Summary

Five phases, sequenced by what the input brief itself prioritizes ("order
matters — 1 and 2 are the ones a reviewer would see first") and then by
theme. Phase 1 covers items 1 and 2 together — both are "what a reviewer
reads first" fixes (README, fixture honesty) and neither touches code
behaviour, so they land first and are the lowest-risk commits on the
branch. Phase 2 covers items 3 and 4 together — both are test-suite
rebalancing across `model.rs`/`proxy.rs`/`binance.rs`, the same theme, and
item 4 explicitly redirects effort freed up by item 3, so reviewing them
side by side is more honest than splitting them. Phase 3 is item 5 alone (a
rename, small and mechanical, but touching two files' call sites so it
gets its own commit boundary). Phase 4 is item 6 alone (the largest code
change in this packet — moving ~100 lines, changing a read strategy, and
fixing `compose.yml`, so it stays isolated). Phase 5 is item 7 alone (one
comment, already resolved by the correction in `spec.md`). This branch's
own history opens with the spec-packet commit (before any phase below) and
closes with a `--no-ff` merge to `main`, per the two process changes this
packet also records.

## Phase 0: spec packet (first commit on this branch)

- Objective: Demonstrate the spec-first process correction this packet
  itself records — `spec.md`, `plan.md`, `tasks.md`, and
  `specs/003-step-1-fixes/inputs/human.md` land as the first commit on
  `003-step-1-fixes`, before any of the five implementation phases below.
- Main changes: `specs/003-step-1-fixes/spec.md`, `plan.md`, `tasks.md`
  (this packet). `inputs/human.md` was already present on the branch before
  packet-writing began.
- Verification: `git log --oneline 003-step-1-fixes` (once phases land)
  shows this commit first, ahead of every phase-N commit below.
- Done looks like: the packet is committed with no implementation changes
  mixed into the same commit.
- Commit boundary: `specs/003-step-1-fixes/spec.md`, `plan.md`, `tasks.md`.

### Phase 1: items 1 and 2 — README accuracy, fixture honesty

- Objective: Fix the two problems a reviewer sees first, neither of which
  changes running behaviour.
- Main changes:
  - `README.md`: rewrite the opening paragraph and Quick Start description
    to state step 1's actual shipped state (Binance feed connects, parses,
    logs; no second venue, no merge, no gRPC). Fix the
    `# defaults, logs, exits 0` compose comment. Re-verify the Layout
    section against the actual current file tree — per `spec.md`'s Current
    State, it may already be close to correct; confirm by reading the tree,
    don't rewrite unconditionally.
  - `src/exchange/binance.rs`: rewrite `DEPTH20_FIXTURE`'s doc comment to
    state the data is synthetic, drop the marketing URL, add the
    `TODO(<human owner>)` line naming the network-access gap. No change to
    the fixture constant's actual bytes or to the test that uses it.
  - `specs/003-step-1-fixes/revisions.md` (new): record two process
    entries — "README updates are part of finishing a step, from
    003-step-1-fixes onward" and "never invent data presented as captured;
    state synthetic-and-why, or ask for a capture."
- Verification:
  - `cargo test` — unchanged test count and results (this phase touches no
    test logic, only comments and docs).
  - `cargo build`, `cargo clippy --all-targets -- -D warnings`,
    `cargo fmt --check` — all clean.
  - Manual read-through: no remaining "step 0" language, no "exit 0" claim,
    Layout section matches `find src -name "*.rs"` output, `DEPTH20_FIXTURE`
    comment contains no marketing URL and does contain a TODO naming the
    human owner.
- Done looks like: `README.md` and the fixture comment both read honestly
  against the actual current repo state; `revisions.md` exists with two
  process entries.
- Commit boundary: `README.md`, `src/exchange/binance.rs` (comment only),
  `specs/003-step-1-fixes/revisions.md`.

### Phase 2: items 3 and 4 — rebalance test coverage

- Objective: Add real coverage for `Ord`'s load-bearing behaviour in
  `model.rs`, and cut `proxy.rs`'s redundant tests down to 2, so total test
  effort shifts toward the parser and the foundational type.
- Main changes:
  - `src/model.rs`: add 2-3 tests per `spec.md`'s Proposed Design (item 3)
    — ascending sort of a `Vec<Price>`, equal-value reflexivity/consistency.
    Add the `Amount` equivalent only if it's not already covered by
    symmetry and a distinct bug is nameable.
  - `src/proxy.rs`: read which lines of `parse_proxy_addr` each of the 6
    existing tests uniquely exercises; collapse to exactly 2 —
    `parses_scheme_host_and_port` (or an equivalent well-formed-with-scheme
    case) and one rejection test covering the "unparseable input returns
    `None`" property, folding or dropping `parses_without_a_scheme`,
    `ignores_a_trailing_path`, and the other two "rejects_*" tests per
    `spec.md`'s item 4 design.
- Verification:
  - `cargo test` — green; report actual counts (`model.rs`, `proxy.rs`)
    against the target (`model.rs` 3, `proxy.rs` 2).
  - `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`
    clean.
  - For every test added or kept, confirm (by writing it into the commit
    message or a code comment) the specific bug it catches, per
    `spec.md`'s Testing Strategy — no test survives on "seems useful."
- Done looks like: `model.rs` has 2-3 new tests each pinning a named
  ordering property; `proxy.rs` has exactly 2 tests; no test in either file
  exists without a stated reason.
- Commit boundary: `src/model.rs`, `src/proxy.rs`.

### Phase 3: item 5 — rename `from_str_price` to `parse`

- Objective: Make the constructor names match the newtype boundary they're
  supposed to enforce.
- Main changes: `src/model.rs` — `Price::from_str_price` → `Price::parse`,
  `Amount::from_str_price` → `Amount::parse` (verify via `cargo build`
  whether `parse` collides with an in-scope trait method at any call site
  before committing to the name; fall back to `from_decimal_str` only if
  it does). Update every call site: `src/model.rs`'s own tests,
  `src/exchange/binance.rs`'s `parse_levels` and its tests.
- Verification:
  - `cargo build` succeeds with the new name at every call site (a
    leftover `from_str_price` reference fails the build, which is the
    actual proof every call site was updated — not a `grep`).
  - `cargo test` green, same test count as end of Phase 2 (a rename adds
    no tests).
  - `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`
    clean.
- Done looks like: `from_str_price` no longer appears anywhere in `src/`;
  `Price::parse`/`Amount::parse` are the only constructors.
- Commit boundary: `src/model.rs`, `src/exchange/binance.rs`.

### Phase 4: item 6 — move proxy plumbing, buffer the header read, fix compose.yml

- Objective: Get the CONNECT-tunnel plumbing out of `main.rs`'s way, stop
  reading the proxy's response one byte at a time, and stop `compose.yml`
  from emitting a broken proxy value the code has to special-case.
- Main changes:
  - `src/proxy.rs`: add `connect_through_proxy` and
    `read_http_response_headers`, moved verbatim in logic (not
    reimplemented) from `src/main.rs`, next to `parse_proxy_addr`.
    `read_http_response_headers` changes from a byte-at-a-time
    `stream.read(&mut byte)` loop to wrapping the stream in a
    `tokio::io::BufReader` and reading via `read_until` up to the
    blank-line terminator.
  - `src/main.rs`: delete the two moved functions; call
    `proxy::connect_through_proxy(...)` instead. Delete the `"://:"`
    special case in `proxy_addr()` entirely — it becomes dead code once
    `compose.yml` no longer produces that value.
  - `compose.yml`: change `HTTP_PROXY`/`HTTPS_PROXY` so they are not set at
    all when `PROXY_HOST`/`PROXY_PORT` are absent from `.env`, rather than
    resolving to `http://:`. Verify against a clean environment (no `.env`
    file present at all — the exact case the current bug requires) that
    `docker compose up --build`/`docker compose run` still start cleanly
    with no proxy configured.
  - `README.md`: no framing change — the proxy is still documented as an
    optional feature; only its implementation location changes, which the
    README doesn't currently describe at the function level.
- Verification:
  - `cargo build`, `cargo test` (green, same count as Phase 3's close —
    this is code motion plus a strategy change, not new test surface;
    if a new test is warranted for the buffered read, name the bug it
    catches before adding it, per the project's testing convention — a
    likely candidate is "response split across multiple TCP segments,"
    but only add it if it's genuinely exercisable without a live proxy;
    otherwise this remains an inspection-verified change, consistent with
    Testing Strategy in spec.md, which does not require a new test here).
  - `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`
    clean.
  - `grep -n "://:"` across `src/` and `compose.yml` returns nothing.
  - With no `.env` file present, `docker compose config` (or
    `docker compose up --build` followed by an immediate stop) shows
    `HTTP_PROXY`/`HTTPS_PROXY` are absent from the resolved environment,
    not present-and-broken.
  - `docker compose build` succeeds.
- Done looks like: `src/main.rs` no longer contains any CONNECT-tunnel
  logic beyond the one call to `proxy::connect_through_proxy`;
  `read_http_response_headers` no longer reads byte-at-a-time; running with
  no `.env` at all produces no proxy env vars, not a broken template
  string.
- Commit boundary: `src/main.rs`, `src/proxy.rs`, `compose.yml`.

### Phase 5: item 7 — document the `Frame` arm

- Objective: Record, in code, why `Message::Frame(_)` stays — closing the
  loop on the correction already made in `spec.md`.
- Main changes: `src/main.rs` — add a one-line comment above the `Frame(_)`
  arm per `spec.md`'s Proposed Design (item 7). No behaviour change; the
  arm itself is untouched.
- Verification:
  - `cargo build` — still compiles (confirms the match remains exhaustive,
    the same check performed while writing `spec.md`).
  - `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`
    clean.
- Done looks like: the comment is present; no other line in `src/main.rs`'s
  match arm changed.
- Commit boundary: `src/main.rs`.

## Cross-Cutting Considerations

- **No behavioural change anywhere in this packet.** Every phase above is
  either docs, comments, test rebalancing, a rename, or code motion — the
  binary's connect/parse/log behaviour at the end of Phase 5 must be
  identical to its behaviour at the start of Phase 1. This is the single
  invariant every phase's verification should reaffirm.
- **Test count bookkeeping.** Track the running total across phases: start
  of Phase 1 = 18 (12 unit + 6 integration). Phase 2 changes unit count by
  (+2 or +3 model, -4 proxy) = net -1 or -2. Phases 3-5 add no tests.
  Report the actual final count in the packet's Acceptance Criteria
  check, not the projected one.
- **Item 4's exact test survivors are decided during Phase 2**, by reading
  which lines each existing test uniquely covers — not guessed in advance.
  The binding constraint is the final count (2), stated in `spec.md`.
- **Item 6 is the only phase touching `compose.yml`** and the only phase
  needing a clean-environment (`.env`-absent) re-verification — treat that
  verification step as load-bearing, not optional, since it's the exact
  case the original bug required to reproduce.

## Verification Gates

Before this branch is considered ready to hand off and merge:

- `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check` all clean at the tip of the branch.
- `docker compose build` succeeds.
- Test counts per module reported against the target in `spec.md`'s
  Acceptance Criteria (`proxy.rs` 2, `model.rs` 3, `exchange::binance` 3,
  `config` 2 unchanged).
- Every test added in Phase 2 has a named bug or regression it catches,
  reported alongside the phase.
- `specs/003-step-1-fixes/revisions.md` exists with all required entries
  (README rule, fixture-honesty rule, spec-first commit order, `--no-ff`
  merges, item-7 correction).
- `git log --oneline 003-step-1-fixes` shows the spec-packet commit
  (Phase 0) first, ahead of all five implementation-phase commits.
- Merging to `main` uses `git merge --no-ff 003-step-1-fixes`; `git log
  --graph` on `main` afterward shows a merge commit for this branch.

## Expected Drift Triggers

If any of the following becomes true while implementing, update `spec.md`
before continuing rather than improvising past it:

- Phase 1's Layout-section check finds the README's Layout tree is already
  fully accurate (per `spec.md`'s Current State note that it may already
  be close) — if so, that sub-item is a no-op, and the commit message
  should say so explicitly rather than silently touching nothing.
- Phase 2's line-by-line read of `parse_proxy_addr`'s test coverage reveals
  a genuinely distinct case among the 6 existing tests that the target
  count of 2 would lose real coverage on — that's a case for flagging back
  to the human owner (per `spec.md`'s Risks), not silently keeping a third
  test and missing the stated target.
- Phase 3's `cargo build` check finds `parse` collides with an in-scope
  trait method — switch to `from_decimal_str` per `spec.md`'s fallback,
  and note which call site triggered it.
- Phase 4's clean-environment `compose.yml` check finds `docker compose`
  variable-substitution syntax doesn't support "unset entirely when a
  variable is absent" the way assumed — that's a design gap in the fix
  approach, not something to work around silently; report it.
