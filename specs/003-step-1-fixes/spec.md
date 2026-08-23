---
spec_name: "Step 1 follow-up fixes"
spec_id: "003"
spec_folder: "003-step-1-fixes"
status: "draft"
created_at: "2026-08-23"
updated_at: "2026-08-23"
created_by: "one-shot-spec-packet"
creation_mode: "one-shot-minor-update"
source_inputs:
  - "inputs/human.md"
source_agents: []
goal: "Fix seven concrete, reviewer-visible problems left over from the merged step 1 (Binance feed) — a stale README, a fabricated test fixture presented as real, an untested Ord invariant, inverted test weight, a misnamed constructor, ~100 lines of CONNECT-tunnel plumbing buried in main.rs, and one match arm to verify — without touching step 1's actual feed behaviour."
purpose: "The step 1 packet (002-binance-feed) shipped working code, but a post-merge review found seven small, independent defects that would each mislead a reviewer in a different way: a README describing a state the repo no longer has, a fixture comment that overstates its own provenance, a compiler-enforced invariant (Ord via f64::total_cmp) that step 5's merge will depend on entirely but that has zero test coverage, and test effort weighted toward a six-line env-var parser instead of the actual feed parser and the foundational type. None of these change what the program does; all of them change what a reader would believe about what the program does, or how safely later steps (5, in particular) can build on what's here. This packet also records two process corrections — spec-first commit ordering and --no-ff merges — so this branch's own history demonstrates the fix rather than just describing it."
parent_request: "step-1 follow-up fixes brief, 2026-08-23 (specs/003-step-1-fixes/inputs/human.md)"
related_paths:
  - "README.md"
  - "compose.yml"
  - "src/model.rs"
  - "src/exchange/binance.rs"
  - "src/proxy.rs"
  - "src/main.rs"
  - "specs/003-step-1-fixes/revisions.md"
verification_level: "mixed"
complexity: "small"
---

# Spec: 003-step-1-fixes

## Problem

Step 1 (`specs/002-binance-feed/`) merged and works — the binary connects to
Binance, parses `depth20` snapshots, and logs the top of book. A post-merge
review of the merged state found seven independent defects, none of which
break the running program, all of which would mislead a reviewer reading the
repo or undermine a later step's ability to trust what's here:

1. `README.md` still describes step 0's state (no websocket client, "exits
   0", a Layout section missing `model.rs`, `exchange/`, `proxy.rs`) even
   though step 1 shipped and changed all three claims.
2. `src/exchange/binance.rs`'s `DEPTH20_FIXTURE` test constant is invented
   data, and its comment falsely implies it was shaped from a real Binance
   doc, citing a marketing URL that isn't a depth-stream reference at all.
   The bids step down in exact `0.00001` increments with amounts in exact
   `0.25` increments — visibly synthetic, and it means the parser has never
   actually met real Binance output (irregular decimal counts, short values
   like `"0"`, uneven gaps).
3. `src/model.rs` has one test (a `Display` round-trip). `Ord` via
   `f64::total_cmp` — the property step 5's `merge()` will depend on
   entirely for sorting bids/asks — is implemented and never exercised.
4. Test weight is inverted: `src/proxy.rs` (a six-line env-var string
   splitter, the author's own network workaround) has 6 tests; three of
   them (`rejects_a_missing_port`, `rejects_a_non_numeric_port`,
   `rejects_an_empty_string`) all assert the same "unparseable input
   returns `None`" property and should be one test per this project's
   testing convention. `src/exchange/binance.rs` (the actual feed parser)
   has 3; `src/model.rs` (the foundational type) has 1.
5. `Amount::from_str_price` (and `Price::from_str_price`, named the same
   way) undercuts the newtype's own justification — the whole point of
   `Price`/`Amount` being distinct types is that they can't be confused,
   and the constructor name embeds "price" on both.
6. `src/main.rs` is 203 lines; roughly 100 of them are the HTTP `CONNECT`
   tunnel (`connect_through_proxy`, `read_http_response_headers`), burying
   the read loop that step 1 is actually about. The header reader also
   reads one byte at a time (a 40-byte response costs 40 syscalls), and
   `main.rs` special-cases a broken `"://:"` proxy value that only exists
   because `compose.yml` sets `HTTP_PROXY`/`HTTPS_PROXY` to that broken
   templated string when `PROXY_HOST`/`PROXY_PORT` are unset.
7. `Message::Frame(_)` is matched in `main.rs`'s read loop with a claim (in
   the human brief) that this variant can't arrive through `.next()` on a
   normal stream. **Verified false while writing this spec** — see
   "Correction to the input brief" below: `tungstenite::Message` has no
   `#[non_exhaustive]` attribute, so the match must handle all six variants
   including `Frame` to compile; removing the arm breaks the build. This
   item is now "confirm and document," not "remove."

None of the seven touch step 1's actual feed behaviour — no change to what
gets connected to, parsed, or logged.

## Correction to the input brief

Item 7 in `inputs/human.md` reads: "`Message::Frame(_)` can't arrive through
`next()` on a normal stream... Drop the arm if the match still compiles
exhaustively without it, and tell me if it doesn't." Reading
`tungstenite` 0.30's `Message` enum directly
(`~/.cargo/registry/.../tungstenite-0.30.0/src/protocol/message.rs`, lines
155-174) confirms the enum has no `#[non_exhaustive]` attribute. The doc
comment on `Frame` does say "you're not going to get this value while
reading the message" — the *practical* claim is correct, `.next()` will
never actually produce `Message::Frame` — but the *type-level* claim is not:
the match is exhaustive today only because the `Frame(_)` arm is present.
Removing it does not compile. Item 7's task is therefore: keep the arm,
and add a one-line comment stating it is structurally required (not
reachable, per tungstenite's own doc comment, but required for exhaustive
matching) — so a future reader doesn't "clean it up" based on the same
mistaken premise.

## Goal

After this packet:

- `README.md` accurately describes step 1's shipped state: one Binance feed
  connecting, parsing, and logging the top of book; no second venue, no
  merge, no gRPC server. Layout section lists `model.rs`, `exchange/`,
  `proxy.rs`. The `# defaults, logs, exits 0` compose comment is corrected
  to describe the actual run-until-closed behaviour.
- `src/exchange/binance.rs`'s fixture comment honestly states the data is
  synthetic (not a real capture) and carries a TODO naming the human owner
  and the reason a real capture isn't in this commit (no live network
  access in this sandbox) — no new fabricated data is introduced.
- `src/model.rs` gains 2-3 tests pinning `Ord`'s behaviour: a collection of
  `Price`s sorts ascending by value; ordering is total and consistent for
  equal values.
- `src/proxy.rs` is cut from 6 tests to 2: one well-formed value with a
  scheme, one that rejects unparseable input.
- `Price::from_str_price`/`Amount::from_str_price` are renamed to
  `Price::parse`/`Amount::parse` (or `from_decimal_str` if a call site makes
  `parse` ambiguous against a trait method already in scope — checked, not
  assumed) with every call site updated.
- `connect_through_proxy` and `read_http_response_headers` move from
  `src/main.rs` into `src/proxy.rs`, next to `parse_proxy_addr`;
  `read_http_response_headers` reads via `BufReader::read_until` instead of
  one byte at a time; the `"://:"` special case is deleted from the code and
  `compose.yml` is fixed so it never sets `HTTP_PROXY`/`HTTPS_PROXY` at all
  when `PROXY_HOST`/`PROXY_PORT` are unset.
- `Message::Frame(_)` stays, with a comment explaining why it's structurally
  required despite being practically unreachable.
- `specs/003-step-1-fixes/revisions.md` exists and records both process
  corrections (spec-first commit ordering; `--no-ff` merges) as well as the
  item-7 correction to the input brief.
- This branch's own commit history has the spec packet as its first commit,
  and merges back to `main` with `git merge --no-ff`.

## Scope

**In** — all seven items above, plus the two process changes, plus the
`revisions.md` file.

**Out — explicitly deferred, do not do these here:**

- The 25-minute live ping/pong verification against Binance (human owner's
  own follow-up).
- Verifying the CONNECT tunnel end-to-end against a real proxy (human
  owner's own follow-up; this sandbox cannot reach one).
- Capturing the real `depth20` fixture (human owner's own follow-up; this
  sandbox cannot reach `stream.binance.com`).
- Any change to step 1's actual connect/parse/log behaviour, the pair
  configuration mechanism, or `Config`.
- Any step-2-onward work (Bitstamp feed, merge, gRPC server).

## Current State

Verified by reading the files directly (2026-08-23, on this branch, off
`main` at `3b28713`):

- `README.md` (lines 8-12, 33-35, 45-71, 122) still states "None of that
  logic exists yet. This is step 0..." and "Both parse arguments, build a
  `Config`, log one `starting` line to stderr, and exit 0" — both false
  since step 1 merged. The Layout tree (lines 45-71) is actually already
  fairly close (it does list `model.rs`, `proxy.rs`, `exchange/` — the
  human brief's claim that Layout "never gained" these appears to predate
  the most recent README commit, `3b28713`, "README: show Layout as a tree,
  bring file list up to date"). The compose comment `# defaults, logs,
  exits 0` at line 122 is still wrong — the binary now runs until the
  websocket connection closes, it does not log-and-exit-0.
- `src/exchange/binance.rs` lines 80-85: `DEPTH20_FIXTURE`'s doc comment
  cites `<https://www.binance.com/en/binance-api>` and calls the data
  "real-shaped... constructed to match the documented wire shape" — the URL
  is Binance's general API marketing page, not a depth-stream doc, and the
  data steps in exact synthetic increments (`0.00001` per bid,
  `0.25000000` per amount).
- `src/model.rs`: one test, `price_round_trips_through_display` (line
  106). No test touches `Ord`/`PartialOrd`/`Eq`.
- `src/proxy.rs`: 6 tests (lines 30-67) —
  `parses_scheme_host_and_port`, `parses_without_a_scheme`,
  `ignores_a_trailing_path`, `rejects_a_missing_port`,
  `rejects_a_non_numeric_port`, `rejects_an_empty_string`. The last three
  all assert `parse_proxy_addr(...) == None` for a different malformed
  shape.
- `src/model.rs` lines 32, 40: `Price::from_str_price` and
  `Amount::from_str_price` — both constructors named after `Price`
  regardless of which type they're on.
- `src/main.rs` is 204 lines. `connect_through_proxy` (lines 156-181) and
  `read_http_response_headers` (lines 186-203) live here, not in
  `src/proxy.rs`. `read_http_response_headers` reads one byte at a time
  into a `Vec<u8>` via `stream.read(&mut byte)` in a loop (line 190-197).
  `proxy_addr()` (lines 131-149) special-cases
  `raw.trim_end_matches('/').ends_with("://:")` at line 139 to treat that
  exact string as "no proxy" — this exists only because `compose.yml` lines
  25-26 build `HTTP_PROXY`/`HTTPS_PROXY` from
  `${PROXY_HOST:-}:${PROXY_PORT:-}`, which is the literal string `http://:`
  when both are unset.
- `src/main.rs` lines 87-119: the `match message` block handles `Text`,
  `Ping | Pong`, `Close`, `Binary`, and `Frame(_)` explicitly — six of six
  `tungstenite::Message` variants (see "Correction to the input brief"
  above for why `Frame` cannot simply be dropped).
- `cargo test` currently reports 12 unit tests (`config::tests` 2,
  `exchange::binance::tests` 3, `model::tests` 1, `proxy::tests` 6) plus 6
  integration tests in `tests/cli.rs` — 18 total.
- `specs/003-step-1-fixes/revisions.md` does not exist yet.
- `specs/002-binance-feed/`'s packet commit (`83d01f5 Add spec packet for
  002-binance-feed`) landed after `b07e818` (the proxy feature commit) and
  before the two most recent commits — i.e. after some implementation had
  already landed, not before all of it. `git log --oneline` on `main` shows
  no merge commit for the `002-binance-feed` branch boundary (fast-forward).

## Proposed Design

### Item 1 — README accuracy pass

Rewrite the opening paragraph and Quick Start description to state: the
websocket client to Binance exists and streams `depth20@100ms`; the parser
converts to the internal `Book` type and logs one line per update; no second
venue, no merge, no gRPC server yet. Fix the `# defaults, logs, exits 0`
compose comment to describe the actual behaviour (connects, runs until the
connection closes or errors). Re-verify the Layout section against the
actual file tree at time of edit (per Current State above, it may already
be close to correct — confirm, don't assume it needs a rewrite). Add a line
to `specs/003-step-1-fixes/revisions.md` establishing, as a standing rule
from this packet onward, that a README update is part of finishing a step,
not a deferred task.

### Item 2 — fixture honesty, no fabrication

Do not invent a new fixture. Rewrite the `DEPTH20_FIXTURE` doc comment to:
state plainly the data is synthetic, not a real capture; drop the marketing
URL; add `// TODO(<human owner>): replace with a real captured
wss://stream.binance.com:9443/ws/ethbtc@depth20@100ms payload — this
sandbox has no live network access to capture one.` The fixture constant
itself, and the test that exercises it, are otherwise unchanged — this
keeps `cargo test` green while making the gap visible instead of papered
over. Record the same "never invent data presented as captured" rule in
`revisions.md` as a process entry, mirroring item 1's process entry.

### Item 3 — `Ord` coverage for `Price`

Add 2-3 tests to `src/model.rs`'s `mod tests`, each pinning a specific,
nameable failure mode:

- A `Vec<Price>` built from out-of-order string literals, sorted with
  `.sort()`, ends up ascending by value — catches a flipped comparator
  (e.g. `other.cmp(self)` instead of `self.cmp(other)`) or a `PartialOrd`
  impl that silently disagrees with `Ord`.
- Two `Price`s parsed from the same string compare `Ordering::Equal` and
  are interchangeable in a `BTreeSet`/sorted context — catches a
  `total_cmp` misuse that breaks reflexivity for equal values (the
  documented gap `total_cmp` exists to close, e.g. around `NaN` or signed
  zero, though this domain never produces either from a decimal string).
- (Optional third, only if a distinct failure mode is nameable): the same
  three-test treatment applied to `Amount`, if `Amount`'s `Ord` isn't
  already covered by symmetry with `Price`'s — add only if a real gap
  exists, per the "name the bug or don't add the test" rule.

### Item 4 — collapse `proxy.rs`'s three redundant "rejects" tests

Keep `parses_scheme_host_and_port` (well-formed value with a scheme) and
one rejection test covering the "returns `None` for unparseable input"
property — the existing `rejects_a_missing_port`,
`rejects_a_non_numeric_port`, and `rejects_an_empty_string` all assert
exactly this property for three different malformed shapes, and per the
project's testing convention (also applied in `specs/002-binance-feed/`)
they collapse into one. `parses_without_a_scheme` and
`ignores_a_trailing_path` are two more of `proxy.rs`'s existing 6 tests, not
mentioned in the human brief's item 4 — the brief's target is 2 tests
total, so these two also need to be dropped or folded, since keeping them
would land at 3-4, not 2. Resolve by folding the "no scheme" and
"trailing path" cases into the one well-formed-parse test as sub-assertions
if they'd otherwise be lost coverage worth keeping — or drop them if the
single well-formed case already exercises the same code path — decided at
implementation time by reading which lines of `parse_proxy_addr` each test
actually exercises uniquely; the binding constraint from the brief is the
final count (2), not which specific two of the six survive verbatim.

### Item 5 — rename `from_str_price` to `parse`

`Price::from_str_price` → `Price::parse`; `Amount::from_str_price` →
`Amount::parse` (fall back to `from_decimal_str` only if `parse` collides
with an already-in-scope trait method at a call site — check with
`cargo build` before committing to the name, don't assume). Update every
call site: `src/model.rs`'s own tests, `src/exchange/binance.rs`'s
`parse_levels` and its tests.

### Item 6 — move proxy plumbing, fix the buffered read, fix compose.yml

Move `connect_through_proxy` and `read_http_response_headers` from
`src/main.rs` into `src/proxy.rs`, next to `parse_proxy_addr`. Change
`read_http_response_headers` to read via `std::io::BufRead::read_until`
(wrapping the stream in a `tokio::io::BufReader`, reading line-by-line up to
the blank-line terminator) instead of the current byte-at-a-time
`stream.read(&mut byte)` loop — still not a full HTTP parser, just not
40 syscalls for a 40-byte response. `src/main.rs` calls one function (e.g.
`proxy::connect_through_proxy(...)`) and gets back a connected stream.
Delete the `"://:"` special case from `proxy_addr()` in `src/main.rs`
entirely. Fix `compose.yml` so `HTTP_PROXY`/`HTTPS_PROXY` are not set at all
when `PROXY_HOST`/`PROXY_PORT` are absent from `.env` — e.g. guard with
compose's variable-substitution conditionals, or move the proxy env vars
into a separate `.env`-only override file that's simply absent by default,
whichever keeps `docker compose up --build` working with no `.env` present
at all (must re-verify, since this is exactly the "defaults with no
environment" case `Config::from_env()` already guarantees at the Rust
level — `compose.yml` should offer the same guarantee). Keep the README's
existing framing of the proxy as a legitimate optional feature.

### Item 7 — document, don't remove, the `Frame` arm

Per "Correction to the input brief" above: keep the `Message::Frame(_)` arm
in `src/main.rs`'s match. Add a one-line comment: `// Structurally required
for the match to be exhaustive — tungstenite::Message has no
#[non_exhaustive], even though .next() never actually produces this
variant (see the Frame doc comment in tungstenite's own source).`

## Acceptance Criteria

- `cargo build` succeeds.
- `cargo test` is green; report the exact test count per module after the
  rebalance (target: `proxy.rs` 2, `exchange::binance` 3, `model.rs` 3,
  `config` 2 unchanged — confirm the actual final numbers when done, since
  item 4's exact survivors are decided at implementation time).
- `cargo clippy --all-targets -- -D warnings` is clean.
- `cargo fmt --check` is clean.
- `docker compose build` succeeds.
- `git log --graph` on `main`, after this branch merges, shows a merge
  commit for `003-step-1-fixes` (not a fast-forward).
- Every test added in items 3/4 has a named bug or regression it catches,
  stated in this packet or in the commit that adds it; if no such bug can
  be named, the test is not added.
- `specs/003-step-1-fixes/revisions.md` exists and records: the README
  process rule (item 1), the "never fabricate data presented as captured"
  rule (item 2), the spec-first commit-order rule, the `--no-ff` merge
  rule, and the item-7 correction to the input brief.

## Invariants and Critical Don'ts

- No change to what the binary connects to, parses, or logs — this packet
  is cleanup, not new behaviour.
- Item 2 must not introduce a new fabricated fixture; the honest options
  are "leave the existing synthetic data with an honest comment and a TODO"
  or "use a real capture if one becomes available" — never "invent a more
  convincing-looking one."
- Item 4's final `proxy.rs` test count is 2, not 3 or 4 — don't stop at
  removing only the three redundant "rejects" tests if that still leaves
  more than 2 total.
- Item 6 must not change the proxy's opt-in behavior or its README framing
  — only where the code lives and how the header read is buffered.
- Item 7: the `Frame` arm stays. Do not remove it based on the original
  brief's premise — this spec's correction supersedes that premise.
- The spec packet for this folder must be the first commit on this branch,
  and this branch merges to `main` with `git merge --no-ff` — both are
  acceptance criteria for the process, not just code.

## Risks and Tradeoffs

- **Item 4's exact test survivors are a judgment call**, not fully
  prescribed by the brief (which names a target count, 2, but the brief's
  own item 4 only explicitly discusses 3 of the 6 existing tests as
  redundant). Resolved in Proposed Design by deciding at implementation
  time which of the remaining tests are redundant with each other, keeping
  the binding constraint (final count = 2) rather than guessing which two
  survive without reading the code.
- **Item 6's compose.yml fix must be re-verified against a clean
  environment** (no `.env` file at all) since that's the exact case the
  brief says is currently broken — a fix that only works when `.env` exists
  but is empty would not actually close the gap.
- **Item 2 leaves a known gap** (synthetic fixture, TODO) rather than
  resolving it — this is the deliberate, correct choice per the human
  brief's own instruction ("I'd rather have a visible gap than a
  plausible-looking invention"), not an incomplete task.

## Testing Strategy

Per `.claude/rules/testing.md` and the convention already applied in
`specs/002-binance-feed/revisions.md` entry — for every test added, name
the specific bug or regression it catches; if none can be named, don't add
it. Applied here:

- Item 3's `Price` ordering tests each state, in Proposed Design above,
  exactly which comparator bug they'd catch (a flipped `cmp`, a
  `PartialOrd`/`Ord` disagreement, a `total_cmp` reflexivity break).
- Item 4's collapse removes tests, it doesn't add any — the "name the bug"
  rule instead applies in reverse: each surviving test must still catch a
  distinct failure mode, or it also gets folded/dropped.
- No test is added for items 1, 2, 5, 6, or 7 — they're a docs fix, a
  comment/TODO fix, a rename (already covered by existing call-site tests,
  which must still pass after the rename), a code-motion plus one
  buffering change (verified by `cargo test` continuing to pass and, per
  Out of Scope, a live proxy is not available to test end-to-end in this
  sandbox), and a comment addition, respectively.

## Rollback Plan

Each of the seven items is independently revertible: items 1, 2, 3, 4, 5,
7 touch a single file each (`README.md`+`compose.yml`,
`src/exchange/binance.rs`, `src/model.rs`, `src/proxy.rs`, `src/model.rs`
+`src/exchange/binance.rs`, `src/main.rs`); item 6 touches
`src/main.rs`+`src/proxy.rs`+`compose.yml`. `git revert` on any single
item's commit restores the pre-fix state for that item without affecting
the others, provided items land as separate commits per `plan.md`'s phase
boundaries.

## Open Questions

None blocking. Item 4's exact surviving-test choice and item 6's
`from_decimal_str` fallback naming are both resolved at implementation time
per explicit, bounded rules stated in Proposed Design (final count / check
before naming) rather than deferred as open questions — the brief is
otherwise prescriptive on every point. Item 7's premise was checked (not
assumed) while writing this spec and is corrected above, not left open.

## Process Changes (recorded from this packet onward)

Two corrections to how this project's spec packets are handled, effective
starting with this branch, both also recorded in
`specs/003-step-1-fixes/revisions.md`:

1. **Spec-first commit ordering.** `002-binance-feed`'s spec packet
   (`specs/002-binance-feed/`) was committed as `83d01f5`, after
   `b07e818` (a real implementation commit) had already landed — the
   opposite of "write the spec, then implement against it." From this
   branch onward, the spec packet (`spec.md`, `plan.md`, `tasks.md`) is the
   first commit on a new spec branch, before any implementation commit.
   This branch's own history demonstrates the fix.
2. **`--no-ff` merges.** `002-binance-feed` was fast-forwarded into `main`,
   leaving no merge commit marking the branch boundary in `git log`. This
   branch, and every spec branch from here on, merges back with
   `git merge --no-ff` so the branch boundary stays visible in
   `git log --graph`.
