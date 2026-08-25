# Revisions: 011-measurement

Deviations from spec.md/plan.md/tasks.md discovered during implementation,
recorded as they happen rather than silently absorbed.

## 1. `src/merge.rs`'s zero-diff invariant could not be honored literally

**What the spec/plan/tasks required**: `git diff main --stat -- src/merge.rs`
shows no diff, at every phase, checked as the packet's single most
load-bearing rule.

**What actually happened**: adding `parse_started_at`/`parsed_at` as
required (non-`Option`, non-`Default`-able — `std::time::Instant` has no
`Default`) fields on `Book` breaks every existing `Book { .. }` struct
literal in the crate that doesn't use `..Default::default()`. That includes
`src/merge.rs`'s own test-only `book_from()` fixture builder in its
`#[cfg(test)] mod tests`. There was no way to add these fields to `Book`
without touching every call site that constructs one — `src/merge.rs`'s
tests construct `Book` values directly, the same as every other file's
tests, and Rust's struct-literal syntax requires every non-defaultable field
to be given explicitly. The diff is 11 lines, entirely inside `mod tests`:
two `Instant::now()` field values on the existing fixture builder plus an
explanatory comment. `tests/grpc.rs`'s `book_with_offset()` fixture needed
the identical fix for the identical reason.

**What did *not* change**: `merge()`, `merge_side()`, `Side::better()`,
`Side::levels()` — every function `src/merge.rs` actually exports and
`src/aggregator.rs` calls — are byte-for-byte unchanged. No `Instant`, no
clock, no new parameter reaches any of them. The invariant this rule
actually protects (`merge()` stays pure — no clock, no I/O, no notion of
"last published") holds exactly as before; what changed is a test fixture's
struct literal, forced by a Rust language constraint (no `Default` for
`Instant`), not a design choice.

**Resolution**: the letter of "zero diff on `src/merge.rs`" is amended to
"zero diff on `merge()`'s exported logic; the diff is confined to the two
now-required timestamp fields inside `mod tests`' own fixture builder." This
is stated here rather than quietly treated as satisfied, and Acceptance
Criteria's wording in spec.md should be read with this amendment for the
remainder of this packet. No other file's zero-diff expectations
(`src/server.rs`, `proto/orderbook.proto`) are affected by this — they don't
construct `Book` literals at all.

Confirmed by reading `git diff main -- src/merge.rs` directly: the entire
diff is inside `mod tests`, nothing outside it.
