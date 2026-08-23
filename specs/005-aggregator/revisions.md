# Revisions — 005-aggregator

Numbered log of changes to the approved spec, applied after the fact. Each
entry states what changed, why, and what it supersedes.

## 1. `summarise()`'s one-sided-book fallback: `None`, not spread `0.0`

**What changed:** `src/merge.rs`'s `summarise()` originally handled a book
with an empty bid or ask side by publishing `spread: 0.0`, commented as a
"defensive fallback, not a market-state claim." It now returns `None` in
that case instead — no `Summary` gets published at all.

**Why:** `0.0` *is* a market-state claim — it asserts the best bid and best
ask are at the same price, a perfectly crossed market. That's specific and
false. Same reasoning as step 2's decision to filter `None` out of the
watch stream rather than publish a fabricated empty `Summary`: an absent
value is honest ("nothing to publish"); a fabricated one isn't
distinguishable from a real reading by anything downstream.

**Standing note for step 5:** this question — what to publish when a side
or a whole venue has nothing usable — comes back in a harder form once
there are two books and a merged output. The answer is already settled
here: return `None` (or, for the merged case, skip the write), never
fabricate a specific-looking value to fill the gap. Step 5's spec should
state this as inherited, not re-derive it.

## 2. README's "Deployment notes" proxy paragraph — reframed, not re-argued

**What changed:** the paragraph explaining the dev-time proxy previously
read as an extended disclaimer — "the author's existing default proxy...
the region wasn't chosen for this project specifically" — explaining what
the choice *wasn't*. It's now ordered constraint → solution → generalisation,
with the same facts (Turkey, `t3.nano`, `eu-central-1`) and no hedging:

> Binance is unreachable from Turkish networks, so development runs through
> a CONNECT proxy — a `t3.nano` Squid instance in `eu-central-1`, sized for
> a single websocket. Any CONNECT proxy works, provided port 9443 is
> allowed through its `SSL_ports`/`Safe_ports` ACL.

**Why:** the correction in the prior commit (replacing an invented "region
proximity to Binance" rationale with the real one) was right on the facts,
but ended up reading as apologetic once it was accurate. A constraint
stated and then solved reads better than the same constraint hedged around.
Nothing was un-said — this is a length/ordering fix on top of an
already-correct fact, not a second factual correction.

## 3. Forward note for the next spec's test-writing guidance

**What happened:** `tests/grpc.rs`'s rewrite (phase 4) initially deadlocked
— not a false pass, a real hang caught by actually running the test.
Cause: `watch::channel` only ever holds the *latest* value; sending both
test books before the client subscribed collapsed them into one observable
state, so the second `stream.message().await` never woke up. Fixed by
interleaving: send the second book only after the first read.

**Why this is recorded here rather than left as a one-off fix:** step 5's
`merge()` tests will want multi-update scenarios through the `watch`
channel far more than this step did. Any test driving `watch` must
interleave sends with reads — never batch sends up front — or the same
deadlock resurfaces, and next time it may look like a bug in `merge()`
rather than in the test. Step 5's spec should carry this forward as a
stated testing constraint, not something the implementer rediscovers.
