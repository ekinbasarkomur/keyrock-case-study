# Revisions — 004-grpc-server

Numbered log of changes to the approved spec, applied after the fact. Each
entry states what changed, why, and what it supersedes. `spec.md` is not
rewritten in place — this file is the record of drift and its justification.

## 1. Never commit personal infrastructure details — standing rule

**What changed:** a sentence in `README.md`'s proxy paragraph naming the
specific instance type, cloud provider, and region the project owner's own
dev-time proxy runs on (`t3.nano` EC2 in `eu-central-1`) was cut down to the
one clause that's actually actionable for a reader: any HTTP `CONNECT`
proxy works, provided its `SSL_ports`/`Safe_ports` ACL allows port 9443.

**Why:** this is the third time the project owner's own infrastructure ended
up in a committed file — a homelab-specific network note in `compose.yml`
(removed during step 0's cleanup pass), the raw brief under
`specs/*/inputs/` (kept gitignored precisely because it's working input, not
publishable), and now this. The pattern each time is the same: a true,
harmless-seeming detail about *how the author personally runs this* creeps
into a file meant to describe *how the software works*. The ACL requirement
is a fact about the feature — useful to anyone standing up their own proxy.
The instance type, provider, and region are facts about the author's
account — useful to no reader, and they quietly reframe proxy support from
"a capability this service has" to "a workaround the author needed," which
is a different, worse story for the same code.

**Standing rule, binding from this entry forward:** nothing about the
project owner's own machine, network, cloud accounts, or other projects
goes into a file that gets committed. If a detail only makes sense to the
author, it doesn't belong in a repository they've invited someone else to
read.
