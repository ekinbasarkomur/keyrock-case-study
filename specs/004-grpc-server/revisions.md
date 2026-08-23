# Revisions — 004-grpc-server

Numbered log of changes to the approved spec, applied after the fact. Each
entry states what changed, why, and what it supersedes. `spec.md` is not
rewritten in place — this file is the record of drift and its justification.

## 1. Personal infrastructure details in a committed file — narrowed rule

**What changed (original entry):** a sentence in `README.md`'s proxy
paragraph naming the specific instance type, cloud provider, and region the
project owner's own dev-time proxy runs on (`t3.nano` EC2 in
`eu-central-1`) was cut down to the one clause that's actually actionable
for a reader in that context: any HTTP `CONNECT` proxy works, provided its
`SSL_ports`/`Safe_ports` ACL allows port 9443.

**What changed (this revision):** the same fact — `t3.nano`, Squid,
`eu-central-1` — was reintroduced, deliberately, in a new "Deployment
notes" section, alongside why that region was picked (proximity to
Binance's endpoint, not capacity — it forwards one websocket) and what a
real deployment would do differently (NAT gateway/VPC endpoint instead of
a standalone proxy instance, a real health check instead of relying on
`select!`'s exit-on-failure as the only signal). This is not a reversal —
it's the same fact placed where it earns its place.

**Why the same fact was wrong in one spot and right in the other:** the
proxy paragraph's surrounding context is "here's a capability this service
has, and how to configure it" — a personal instance detail there reads as
"here's how the author personally worked around a limitation," which is a
different, worse story about the same code. A "Deployment notes" section's
context is "here's how this actually runs and what a real deployment would
change" — the same detail there reads as operational awareness: knowing
what to size, where, and why. Same sentence, different frame, different
signal to the reader.

**Standing rule, narrowed rather than contradicted:** the line was never
"nothing about the author's setup, ever" — it's whether the detail informs
the reader about the *system* or only about the *author's account*. Region
choice and instance sizing for a documented deployment path are the
former — a reader deciding how to deploy this themselves can use them.
Which of the author's other, unrelated projects share a Docker network, or
a homelab-specific path only the author's machine has, are the latter —
they inform nothing about this system for a reader who doesn't have that
machine. Apply this distinction going forward rather than a blanket "no
personal detail, anywhere."

## 2. A committed file never cites a file that isn't committed

**What changed:** `tests/grpc.rs`'s module doc comment cited a rules file
under this project's local, gitignored agent guidance directory (a reader
of the pushed repository can't open it) alongside
`specs/002-binance-feed/revisions.md` (committed, fine — kept). The same
pattern existed in `specs/004-grpc-server/tasks.md` and in
`specs/002-binance-feed/revisions.md`'s own entry 3. All four unreachable
citations were dropped or rewritten to describe the fact without pointing
at a path the reader can't follow; the reachable citations stayed.

**Why:** this exact failure mode already happened once, in step 0 — a
citation correct and helpful to an agent working inside this repo (which
does have the local guidance directory on disk) but a dead end for anyone
reading the pushed repository on GitHub, where that directory was never
uploaded. It recurred here across three more files, which means the rule
needs to be explicit and standing, not something caught ad hoc per spec.

**Standing rule, binding from this entry forward:** before a spec packet,
README section, or code comment cites another file as its source of
detail, confirm that file is actually committed (not `.gitignore`d) — if it
isn't, either drop the citation or state the fact directly instead of
pointing at a path the reader can't open. Paired with entry 1 above, both
exist for the same underlying reason: a public repository should be
legible and actionable to a stranger reading it cold, with nothing that
only resolves for the person who wrote it.
