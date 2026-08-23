# Tasks: 003-step-1-fixes

## Task Writing Rules

- Each task should describe a real unit of progress.
- Each task should name the expected files or areas touched.
- Each task should include explicit verification.
- Prefer behavior-level verification over mock-only checks.

## How to Work This List

Work in order: the spec packet commit (Phase 0) first, then phases 1-5 in
order, each ending with its own commit — six commits total, matching
`plan.md`'s commit boundaries. Before committing a phase, its verification
steps must all pass; if one doesn't, fix it inside the phase before moving
on. Merge this branch back to `main` with `git merge --no-ff`, per the
process change this packet records.

**Standing invariant — reaffirm at every phase's close:** no change to what
the binary connects to, parses, or logs. If any phase's diff would change
that, stop and flag it — this packet is cleanup only.

---

## Phase 0: spec packet (first commit)

### 0.1 Commit the packet before any implementation
- Files or areas: `specs/003-step-1-fixes/spec.md`, `plan.md`, `tasks.md`
- Change: none beyond what's already written in this packet.
- Verification: `git status` shows these three files staged, no
  implementation files (`README.md`, `src/**`, `compose.yml`) in the same
  commit.
- Done when: the commit lands and is the first commit on
  `003-step-1-fixes` (confirm with `git log --oneline 003-step-1-fixes`
  once later phases exist, or `git log --oneline main..HEAD` right now).

---

## Phase 1: items 1 and 2 — README accuracy, fixture honesty

### 1.1 Rewrite README's opening and Quick Start to describe step 1's shipped state
- Files or areas: `README.md`
- Change: Replace "None of that logic exists yet. This is step 0..." and
  "Both parse arguments, build a `Config`, log one `starting` line to
  stderr, and exit 0" with accurate text: the Binance websocket client
  connects, parses `depth20` snapshots into the internal `Book`, and logs
  one line per update to stderr; no second venue, no merge logic, no gRPC
  server yet.
- Verification: manual read-through — no "step 0" language, no "exit 0"
  claim remains anywhere in `README.md`.
- Done when: the opening paragraph and Quick Start section both describe
  what `cargo run -- --pair ethbtc` actually does today.

### 1.2 Fix the compose comment
- Files or areas: `README.md` (Docker section, the `docker compose up
  --build # defaults, logs, exits 0` line)
- Change: Replace the comment with one describing the actual behaviour —
  connects to Binance, logs book updates, runs until the connection closes
  or errors (does not exit 0 promptly).
- Verification: manual read-through of the Docker section.
- Done when: no remaining claim that the container "exits 0" after logging
  once.

### 1.3 Re-verify the Layout section against the actual file tree
- Files or areas: `README.md` (Layout section)
- Change: Run `find src -name "*.rs" | sort` and compare against the
  Layout tree already in `README.md`. Per `spec.md`'s Current State, the
  tree may already be accurate (it was last touched by commit `3b28713`,
  after the human brief's complaint was likely written) — if so, make no
  change and say so in the phase's commit message; if it's missing or
  misdescribes any file, fix it.
- Verification: every `.rs` file under `src/` (including `exchange/`) has
  a corresponding, accurately-described line in the Layout tree.
- Done when: the Layout section and `find src -name "*.rs"` agree, and the
  commit message states whether this task changed anything.

### 1.4 Rewrite the `DEPTH20_FIXTURE` comment honestly
- Files or areas: `src/exchange/binance.rs` (doc comment above
  `DEPTH20_FIXTURE`, lines ~80-85)
- Change: Replace the comment claiming the data is "real-shaped...
  constructed to match the documented wire shape at
  <https://www.binance.com/en/binance-api>" with one that states plainly:
  the fixture is synthetic test data, not a real capture; the marketing
  URL is removed; a `// TODO(<human owner's name>): replace with a real
  captured wss://stream.binance.com:9443/ws/ethbtc@depth20@100ms payload —
  this sandbox has no live network access to capture one.` line is added.
  The fixture constant's actual string content is unchanged — do not
  invent replacement data.
- Verification: `cargo test --lib exchange::binance::` — the existing
  fixture test still passes unchanged (proves the constant itself wasn't
  touched).
- Done when: the comment contains no marketing URL, states the data is
  synthetic, and carries the TODO naming the human owner.

### 1.5 Create `specs/003-step-1-fixes/revisions.md` with the two process entries from this phase
- Files or areas: `specs/003-step-1-fixes/revisions.md` (new)
- Change: Record two entries: (1) "README updates are part of finishing a
  step, not a deferred task — from 003-step-1-fixes onward" (item 1's
  process rule); (2) "Never invent data presented as captured — if real
  data isn't reachable, state the gap and why, or ask for a capture, per
  the DEPTH20_FIXTURE fix in this packet" (item 2's process rule). Follow
  the format of `specs/002-binance-feed/revisions.md` (numbered entries,
  each stating what changed and why).
- Verification: file exists, both entries are present and each is
  self-contained (readable without needing this task list open alongside
  it).
- Done when: both entries are recorded.

### 1.6 Full green check for Phase 1
- Files or areas: none — verification-only.
- Change: none.
- Verification:
  - `cargo build`, `cargo test` (same count as before this phase — no test
    logic changed), `cargo clippy --all-targets -- -D warnings`,
    `cargo fmt --check` — all clean.
  - `docker compose build` succeeds (README/comment-only phase, but
    confirm nothing else broke).
- Done when: all checks pass.

**Commit boundary:** `README.md`, `src/exchange/binance.rs`,
`specs/003-step-1-fixes/revisions.md`.

---

## Phase 2: items 3 and 4 — rebalance test coverage

### 2.1 Add `Price` ordering tests to `src/model.rs`
- Files or areas: `src/model.rs` (`mod tests`)
- Change: Add tests per `spec.md`'s Proposed Design item 3:
  1. `prices_sort_ascending_by_value` (or similarly named) — build a
     `Vec<Price>` from out-of-order decimal strings, `.sort()` it, assert
     the result is ascending by parsed value. Catches: a flipped
     comparator (`other.cmp(self)`) or a `PartialOrd`/`Ord` disagreement.
  2. `equal_prices_are_reflexively_equal_in_order` (or similarly named) —
     two `Price`s parsed from the same string compare `Ordering::Equal`,
     and neither `a < b` nor `b < a` holds. Catches: a `total_cmp`
     misuse that breaks reflexivity for equal values.
  3. (Only if a distinct bug is nameable) the same treatment for `Amount`
     — add only if `Amount`'s `Ord` isn't already exercised by symmetry
     with `Price`'s tests and a real gap exists.
- Verification: `cargo test --lib model::` — new tests pass.
- Done when: `src/model.rs` has 2-3 new tests, each with a stated bug it
  catches (in the test's doc comment or name).

### 2.2 Collapse `src/proxy.rs`'s test suite to 2
- Files or areas: `src/proxy.rs` (`mod tests`)
- Change: Read which lines of `parse_proxy_addr` each of the 6 existing
  tests (`parses_scheme_host_and_port`, `parses_without_a_scheme`,
  `ignores_a_trailing_path`, `rejects_a_missing_port`,
  `rejects_a_non_numeric_port`, `rejects_an_empty_string`) uniquely
  exercises. Collapse to exactly 2: one well-formed-value-with-scheme
  parse test, one rejects-unparseable-input test. Fold any genuinely
  distinct case worth keeping into one of the two survivors as an
  additional assertion rather than a separate test function; drop
  anything that's redundant with another survivor.
- Verification: `cargo test --lib proxy::` — exactly 2 tests, both pass.
- Done when: `src/proxy.rs` has exactly 2 tests.

### 2.3 Full green check and count report for Phase 2
- Files or areas: none — verification-only.
- Change: none.
- Verification:
  - `cargo test` — green; report actual per-module counts:
    `model::tests`, `proxy::tests`, `exchange::binance::tests`,
    `config::tests`.
  - `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`
    clean.
- Done when: the actual counts are reported (target: `model.rs` 3,
  `proxy.rs` 2 — confirm actual against target and note any deviation with
  its reason).

**Commit boundary:** `src/model.rs`, `src/proxy.rs`.

---

## Phase 3: item 5 — rename `from_str_price` to `parse`

### 3.1 Rename both constructors and update every call site
- Files or areas: `src/model.rs`, `src/exchange/binance.rs`
- Change: `Price::from_str_price` → `Price::parse`;
  `Amount::from_str_price` → `Amount::parse`. Update call sites:
  `src/model.rs`'s own tests (task 2.1's new tests too, if written before
  this rename lands — otherwise write them with the new name directly),
  `src/exchange/binance.rs`'s `parse_levels` function and its tests. Check
  with `cargo build` whether `parse` collides with an in-scope trait
  method (e.g. `FromStr::parse`) at any call site; if it does, use
  `from_decimal_str` instead and note which call site forced the fallback.
- Verification: `cargo build` succeeds — a missed call site fails the
  build, which is the real proof every reference was updated.
- Done when: `grep -rn "from_str_price" src/` returns nothing.

### 3.2 Full green check for Phase 3
- Files or areas: none — verification-only.
- Change: none.
- Verification:
  - `cargo test` — green, same total count as Phase 2's close (a rename
    adds no tests).
  - `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`
    clean.
- Done when: all checks pass.

**Commit boundary:** `src/model.rs`, `src/exchange/binance.rs`.

---

## Phase 4: item 6 — move proxy plumbing, buffer the header read, fix compose.yml

### 4.1 Move `connect_through_proxy` and `read_http_response_headers` into `src/proxy.rs`
- Files or areas: `src/proxy.rs`, `src/main.rs`
- Change: Move both functions from `src/main.rs` into `src/proxy.rs`, next
  to `parse_proxy_addr`. `src/main.rs` deletes them and calls
  `proxy::connect_through_proxy(...)` instead. No logic change in this
  task beyond the move itself (the buffering change is task 4.2).
- Verification: `cargo build` succeeds; `src/main.rs`'s CONNECT-tunnel
  section is now a single function call, not ~100 lines of plumbing.
- Done when: both functions live in `src/proxy.rs`; `src/main.rs` no
  longer defines them.

### 4.2 Replace the byte-at-a-time header read with `BufReader::read_until`
- Files or areas: `src/proxy.rs` (`read_http_response_headers`)
- Change: Wrap the `TcpStream` in a `tokio::io::BufReader`, read up to the
  blank-line (`\r\n\r\n`) terminator via `read_until` (called per line, or
  accumulated into a buffer checked for the terminator) instead of the
  current `stream.read(&mut byte)` one-byte loop. Still not a full HTTP
  parser — same header-line-oriented approach, fewer syscalls.
- Verification: `cargo build`, `cargo clippy --all-targets -- -D
  warnings` clean. No live-proxy test available in this sandbox (see
  `spec.md` Out of Scope) — verified by inspection and by the type
  signature still returning the same `String` of header text the caller
  already parses.
- Done when: `read_http_response_headers` no longer calls
  `stream.read(&mut byte)` in a loop; a single `grep -n "read(&mut byte"
  src/proxy.rs` (or equivalent) returns nothing.

### 4.3 Delete the `"://:"` special case from `src/main.rs`
- Files or areas: `src/main.rs` (`proxy_addr()`)
- Change: Remove the
  `if raw.trim_end_matches('/').ends_with("://:") { return None; }` block
  and its comment entirely.
- Verification: `cargo build` succeeds (this task depends on task 4.4
  landing in the same commit — compose.yml must stop producing `"://:"`
  before this becomes safe to test end-to-end; both are part of this
  phase's single commit).
- Done when: `grep -n '"://:"' src/main.rs` returns nothing.

### 4.4 Fix `compose.yml` to not set `HTTP_PROXY`/`HTTPS_PROXY` when unset
- Files or areas: `compose.yml`
- Change: Change the `environment:` block so `HTTP_PROXY`/`HTTPS_PROXY`
  are not set at all when `PROXY_HOST`/`PROXY_PORT` are absent from
  `.env`, instead of resolving to the literal `http://:`.
- Verification:
  - With no `.env` file present in the working tree (temporarily move it
    aside if one exists locally, restore after), run
    `docker compose config` and confirm `HTTP_PROXY`/`HTTPS_PROXY` are
    absent from the resolved config, not present with an empty/broken
    value.
  - `docker compose build` succeeds.
- Done when: a clean environment (no `.env`) produces no
  `HTTP_PROXY`/`HTTPS_PROXY` in the resolved compose config.

### 4.5 Full green check for Phase 4
- Files or areas: none — verification-only.
- Change: none.
- Verification:
  - `cargo build`, `cargo test` (green, same count as Phase 3's close
    unless a new test was genuinely warranted per `plan.md`'s Phase 4
    note — report which), `cargo clippy --all-targets -- -D warnings`,
    `cargo fmt --check` — all clean.
  - `docker compose build` succeeds.
  - `grep -rn "://:"` across `src/` and `compose.yml` returns nothing.
- Done when: all checks pass and the grep confirms no trace of the
  special case remains.

**Commit boundary:** `src/main.rs`, `src/proxy.rs`, `compose.yml`.

---

## Phase 5: item 7 — document the `Frame` arm

### 5.1 Add the explanatory comment above the `Frame(_)` arm
- Files or areas: `src/main.rs` (the `match message` block)
- Change: Add, directly above the `Message::Frame(_) => { ... }` arm:
  `// Structurally required for the match to be exhaustive —
  tungstenite::Message has no #[non_exhaustive], even though .next()
  never actually produces this variant (see the Frame doc comment in
  tungstenite's own source).` No other change to the arm or the match.
- Verification: `cargo build` succeeds (confirms the match is still
  exhaustive with the arm present — the same check performed while
  writing `spec.md`'s correction).
- Done when: the comment is present and accurate; `git diff` for this task
  shows only the added comment line(s), no logic change.

### 5.2 Full green check for Phase 5
- Files or areas: none — verification-only.
- Change: none.
- Verification:
  - `cargo build`, `cargo test` (same count as Phase 4's close),
    `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` — all
    clean.
- Done when: all checks pass.

**Commit boundary:** `src/main.rs`.

---

## Final Verification

Before closing the packet and merging:

- `cargo build` — succeeds.
- `cargo test` — green; report the final per-module counts (`model.rs`,
  `proxy.rs`, `exchange::binance`, `config`, `tests/cli.rs`) against the
  target in `spec.md`'s Acceptance Criteria.
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean.
- `docker compose build` — succeeds.
- With no `.env` present, `docker compose config` shows no
  `HTTP_PROXY`/`HTTPS_PROXY` set.
- `grep -rn "from_str_price\|://:" src/ compose.yml` — returns nothing.
- Every test added in Phase 2 has a named bug it catches, reported
  alongside this final check.
- `specs/003-step-1-fixes/revisions.md` exists with all five required
  entries (README rule, fixture-honesty rule, spec-first commit order,
  `--no-ff` merges, item-7 correction to the input brief).
- `git log --oneline 003-step-1-fixes` (or `main..003-step-1-fixes`) shows
  the spec-packet commit first, then six phase commits in order.
- Merge to `main` with `git merge --no-ff 003-step-1-fixes`; confirm
  `git log --graph` on `main` afterward shows the merge commit.
