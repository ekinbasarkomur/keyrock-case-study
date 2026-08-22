# Tasks: 001-step-0-foundation

## Task Writing Rules

- Each task should describe a real unit of progress.
- Each task should name the expected files or areas touched.
- Each task should include explicit verification.
- Prefer behavior-level verification over mock-only checks.

## How to Work This List

Work phase by phase, in order. Each phase ends with its own commit — do not
batch multiple phases into one commit. Before committing a phase, its
Verification steps must all pass; if one doesn't, fix it inside the phase
before moving on, don't defer it to a later phase's cleanup. See `plan.md`
Cross-Cutting Considerations for why commit-per-phase is deliberate here (four
commits, not spec.md's originally-described single commit).

---

## Phase 1: Dependency set + proto + build pipeline

Nothing in `src/` changes in this phase. The existing `hello`/`doctor`
scaffold and its 7 tests must still pass untouched at the end of it — that's
what proves a failure here is the new dependency/proto/build.rs work, not a
regression riding along with it.

### 1.1 Add the full step-0 dependency set via `cargo add`
- Files or areas: `Cargo.toml`, `Cargo.lock`
- Change: Run, in order, from the crate root:
  ```sh
  cargo add tokio --features full
  cargo add tokio-tungstenite --features rustls-tls-webpki-roots
  cargo add futures-util serde_json tracing tracing-subscriber
  cargo add serde --features derive
  cargo add clap --features derive
  cargo add tonic tonic-prost prost
  cargo add --build tonic-prost-build
  ```
  Do not hand-edit version numbers in `Cargo.toml` before or after — every
  version comes from the resolver. `clap`/`tracing`/`tracing-subscriber`
  already exist as dependencies; re-running `cargo add` for them is expected
  to be a no-op or a minor bump, not a new entry.
- Verification:
  - Read `Cargo.toml` back after running the full list and confirm every
    crate named above appears exactly once, with `tonic-prost-build` under
    `[build-dependencies]`, not `[dependencies]`.
  - `git diff Cargo.lock` shows the lockfile changed (proves `cargo add`
    actually resolved and locked versions, not just edited the manifest).
- Done when:
  - `Cargo.toml` lists all four original dependencies plus every new one from
    the Tooling table in `CLAUDE.md` (minus `hdrhistogram`, which is out of
    scope for step 0 per `spec.md`).

### 1.2 Add `proto/orderbook.proto`
- Files or areas: `proto/orderbook.proto` (new)
- Change: Create the file containing exactly the protobuf schema quoted
  verbatim in `CLAUDE.md`'s "gRPC contract" section (`syntax = "proto3"`,
  `package orderbook`, `OrderbookAggregator` service with the single
  `BookSummary` RPC, `Empty`, `Summary`, `Level` messages). Copy it
  byte-for-byte — this file is never hand-edited again after this task lands.
- Verification:
  - `diff` the new file's message/service definitions against the schema
    block in `CLAUDE.md` by eye; there must be zero deviation (no renamed
    field, no added field, no type change).
- Done when:
  - `proto/orderbook.proto` exists and matches the brief's schema exactly.

### 1.3 Add `build.rs` and compile the proto
- Files or areas: `build.rs` (new, crate root — not `src/`)
- Change: Add
  ```rust
  fn main() -> Result<(), Box<dyn std::error::Error>> {
      tonic_prost_build::compile_protos("proto/orderbook.proto")?;
      Ok(())
  }
  ```
  Before trusting this shape, check `tonic-prost-build`'s docs.rs page for
  whatever version `cargo add` resolved in task 1.1 — the API may differ from
  what's shown here or in training data (the 0.14 build-split changed the
  older `tonic-build` shape). Adjust the function name/signature to match
  what's actually published for the resolved version, and note in the commit
  message if it differed from the snippet above.
- Verification:
  - `cargo build` succeeds. If it fails with a `protoc` "not found" style
    error, do not add `protobuf-compiler` here — record that the host is
    missing `protoc` and defer the system-dependency fix to Phase 3's builder
    image; report which case applied.
  - `find target -name orderbook.rs` returns at least one file.
  - That file's contents (`Read` or `grep`) contain both `Summary` and
    `Level`.
- Done when:
  - `cargo build` is green and the generated file contains the two message
    types the brief defines.

### 1.4 Confirm the pre-existing scaffold is untouched and green
- Files or areas: none changed — verification-only task
- Change: none.
- Verification:
  - `cargo test` passes with the original 7 tests (3 unit, 4 integration) —
    same count and same names as before this phase, proving Phase 1 added
    nothing that broke or altered the `hello`/`doctor` scaffold.
  - `cargo clippy --all-targets -- -D warnings` is clean.
- Done when:
  - Both commands above are green with no new warnings introduced by the
    dependency/proto/build.rs additions.

**Commit boundary:** `Cargo.toml`, `Cargo.lock`, `proto/orderbook.proto`,
`build.rs`. Nothing else. Commit message should state whether `protoc` was
needed on the host (per task 1.3) so Phase 3 knows what to check for in the
builder image.

---

## Phase 2: Config, CLI entry point, and test rework

### 2.1 Extend `Config` with `pair` and change the default port
- Files or areas: `src/config.rs`
- Change:
  - Add a `pub pair: String` field to `Config`.
  - `Default for Config`: `pair: "ethbtc".to_string()`, `port: 50051` (was
    `8080`). `host`/`log_level` defaults unchanged.
  - `Config::from_env()`: read `KEYROCK_PAIR` (via the existing `ENV_PREFIX`
    constant) the same way `KEYROCK_HOST`/`KEYROCK_LOG_LEVEL` are read today
    (`env::var(...).unwrap_or(defaults.pair)`), falling back to the default
    when unset. `KEYROCK_PORT` parsing/error path (`ConfigError::InvalidPort`)
    is unchanged.
  - Update the two existing unit tests in `src/config.rs`'s `mod tests`:
    `defaults_are_usable_with_no_environment` asserts `c.port == 50051` (was
    `8080`) and adds `assert_eq!(c.pair, "ethbtc")`; leave
    `invalid_port_is_an_error_not_a_fallback` as-is (that code path didn't
    change).
- Verification:
  - `cargo test --lib` — both `config::tests` cases pass with the new
    assertions.
- Done when:
  - `Config` has five fields (`log_level`, `host`, `port`, `pair`), defaults
    to `port: 50051, pair: "ethbtc"`, and `KEYROCK_PAIR` is readable via
    `from_env()`.

### 2.2 Replace `src/main.rs`'s subcommand CLI with the flat `--pair`/`--port` entry point
- Files or areas: `src/main.rs`
- Change:
  - Delete the `Command` enum (`Hello`/`Doctor`) and the `--verbose` flag
    entirely.
  - New `Cli` struct: `#[arg(long)] pair: Option<String>` and
    `#[arg(long)] port: Option<u16>` — both `Option<T>` with no clap-level
    default, so "not given" is distinguishable from "given the default
    value."
  - `main()`: parse `Cli`, build `let mut config = Config::from_env()?;`,
    then `if let Some(p) = cli.pair { config.pair = p; }` and
    `if let Some(p) = cli.port { config.port = p; }` (CLI overrides env,
    matching the precedence decided in `spec.md`).
  - Call `telemetry::init(&config.log_level)` (drop the `cli.verbose`
    branching — `RUST_LOG` already covers that need).
  - Log exactly one `tracing::info!` line of the form
    `starting, pair={} port={}` (or equivalent structured fields producing
    that substring on stderr) using `config.pair`/`config.port`.
  - Return `Ok(())`. No `println!` — this step produces no stdout output.
- Verification:
  - `cargo build` succeeds (verified again via the tests below, which run the
    real binary).
- Done when:
  - `src/main.rs` has no `Subcommand`, no `Hello`/`Doctor`, no `--verbose`.

### 2.3 Remove the placeholder scaffold from `src/lib.rs`
- Files or areas: `src/lib.rs`
- Change: Delete `greeting()`, its unit test, and the `VERSION` const (its
  only callers — `greeting()` and the `doctor` printout — are both gone, and
  `#[command(version)]` on the clap `Cli` already surfaces the crate version
  independently). Keep `pub mod config; pub mod telemetry;` and the module
  doc comment explaining the library/binary split.
- Verification:
  - `cargo build` succeeds with no dead-code warnings from the removed items.
  - `grep -rn "greeting\|VERSION" src/ tests/` returns no remaining
    references (confirms nothing else still depends on the deleted items).
- Done when:
  - `src/lib.rs` contains only the module declarations and their doc comment
    — no placeholder function, no unit test for it.

### 2.4 Rewrite `tests/cli.rs` for the new CLI/Config contract
- Files or areas: `tests/cli.rs`
- Change: Replace all 4 existing tests (`hello_greets_and_exits_zero`,
  `doctor_reports_resolved_configuration`,
  `invalid_port_fails_loudly_rather_than_defaulting`,
  `logs_go_to_stderr_not_stdout`) with the 6 cases from `spec.md`'s Testing
  Strategy, each invoking the real binary via
  `env!("CARGO_BIN_EXE_keyrock-case-study")`:
  1. No flags, no relevant env vars: exit 0, stderr contains
     `pair=ethbtc` and `port=50051`, stdout is empty.
  2. `--pair btcusd --port 12345`, no env vars: exit 0, stderr reflects both
     overrides.
  3. `KEYROCK_PAIR=btcusd KEYROCK_PORT=12345` set, no flags: exit 0, stderr
     reflects both env-sourced overrides (proves `Config::from_env()` still
     works standalone).
  4. `KEYROCK_PORT=1` set **and** `--port 12345` passed together: stderr
     shows `12345`, not `1` — the precedence regression test; this is the
     single most important case in this task, don't skip or weaken it.
  5. `--port not-a-number`: non-zero exit, rejected by clap before `Config`
     is constructed (assert on stderr mentioning the arg parse failure, not
     `KEYROCK_PORT`).
  6. `KEYROCK_PORT=not-a-number`, no `--port` flag: non-zero exit, stderr
     contains `KEYROCK_PORT` (via `ConfigError::InvalidPort` — this is the
     one case that's a near-direct carryover of the old
     `invalid_port_fails_loudly_rather_than_defaulting` test).
  Keep the existing module doc comment explaining why `tests/` is the truth
  anchor; keep using `Command::new(BIN)` with `.env(...)`/`.envs(...)` and
  `.output()` as the existing file already does.
- Verification:
  - `cargo test --test cli` — all 6 new tests pass, and no test still
    references `hello`/`doctor`/`--verbose`.
- Done when:
  - `tests/cli.rs` has exactly the 6 cases above (or a documented superset,
    per `spec.md`'s "exact count depends on how many cases land"), and case 4
    (flag-beats-env precedence) is present and passing.

### 2.5 Manually confirm the logged line format before trusting the tests
- Files or areas: none — verification-only task, per `plan.md` Phase 2
- Change: none.
- Verification:
  - Run `cargo run --` and read stderr; confirm it contains
    `pair=ethbtc port=50051`.
  - Run `cargo run -- --pair btcusd --port 12345` and read stderr; confirm it
    contains both overridden values.
  - Confirm stdout is empty in both runs.
- Done when:
  - Both manual runs match what task 2.4's assertions check — belt-and-braces
    per `plan.md`, not a replacement for the automated tests.

### 2.6 Full green check for Phase 2
- Files or areas: none — verification-only task
- Change: none.
- Verification:
  - `cargo test` — full suite green (new `tests/cli.rs` cases + updated
    `src/config.rs` unit tests; `greeting`'s old unit test no longer exists).
  - `cargo clippy --all-targets -- -D warnings` clean.
  - `cargo fmt --check` clean.
- Done when:
  - All three commands above exit 0 with no diffs/warnings.

**Commit boundary:** `src/config.rs`, `src/main.rs`, `src/lib.rs`,
`tests/cli.rs`. Nothing in `Cargo.toml`, `proto/`, `build.rs`, or `Dockerfile`
changes in this phase.

---

## Phase 3: Container shape

### 3.1 Fix the Dockerfile's dependency-cache stub stage to cover `build.rs`/`proto/`
- Files or areas: `Dockerfile`
- Change: In the builder stage, before the stub build:
  - Change `COPY Cargo.toml Cargo.lock ./` to
    `COPY build.rs Cargo.toml Cargo.lock ./`.
  - Add `COPY proto/ ./proto/` immediately after (both before
    `RUN mkdir -p src && ... cargo build --release`), since `build.rs` runs on
    every `cargo build` invocation including the stub-source compile — it is
    not conditional on `src/` being real.
  - Leave the existing `RUN touch src/main.rs src/lib.rs` scoped to `src/`
    only — do not add `build.rs` to that touch command; it doesn't change
    between the stub and real build, so touching it would force
    `tonic-prost-build` to needlessly re-run.
- Verification:
  - `docker compose build` succeeds through both build stages.
- Done when:
  - The builder stage's stub-compile step has `build.rs` and `proto/`
    present before it invokes `cargo build --release` the first time.

### 3.2 Drop the `ca-certificates` apt install from the runtime stage
- Files or areas: `Dockerfile`
- Change: Remove the `RUN apt-get update && apt-get install -y
  --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*`
  block from the runtime (`debian:bookworm-slim`) stage entirely, and its
  explanatory comment. Replace the comment with a one-line note that
  `tokio-tungstenite`'s `rustls-tls-webpki-roots` feature bundles its own
  root certs, so the runtime image needs no system CA package (per `CLAUDE.md`
  Trap #6) — nothing else in the runtime stage uses apt, so no apt block is
  needed at all.
- Verification:
  - `docker compose build` still succeeds without the apt step.
  - `docker history keyrock-case-study:local` (or equivalent) shows no apt
    layer in the runtime stage.
- Done when:
  - The runtime stage contains no `apt-get` invocation.

### 3.3 Fix `CMD` so the container logs the startup line instead of printing help
- Files or areas: `Dockerfile`
- Change: Change `CMD ["--help"]` to `CMD []`. `ENTRYPOINT
  ["keyrock-case-study"]` is unchanged. With no `command:` override in
  `compose.yml`, this resolves to running the binary with no arguments, which
  uses the CLI's own defaults (`pair=ethbtc port=50051`) and produces the real
  startup log line rather than help text.
- Verification:
  - Covered by task 3.5's end-to-end run.
- Done when:
  - `CMD []` is the last line of the Dockerfile (or equivalent explicit empty
    array), replacing `CMD ["--help"]`.

### 3.4 Resolve the `protoc`-in-builder-image question
- Files or areas: `Dockerfile` (conditional change)
- Change: If task 1.3 in Phase 1 reported that `protoc` was missing on the
  host and had to be worked around, add
  `RUN apt-get update && apt-get install -y --no-install-recommends
  protobuf-compiler && rm -rf /var/lib/apt/lists/*` to the **builder** stage
  (before the dependency-cache `COPY`/build step). If Phase 1's host build
  succeeded without any `protoc` workaround, do nothing here, and state
  explicitly in the commit message that the builder image was checked and
  needs no `protobuf-compiler` install — don't leave it unaddressed and
  unmentioned either way.
- Verification:
  - `docker compose build` succeeds (this is the actual test — a builder
    stage missing `protoc` fails exactly here, during the proto-compiling
    `cargo build --release` inside the container, even if the host build
    passed).
- Done when:
  - `docker compose build` succeeds with the builder stage as-is, and the
    commit message records which case applied (needed the install, or
    confirmed unnecessary).

### 3.5 End-to-end container run
- Files or areas: none — verification-only task
- Change: none.
- Verification:
  - `docker compose up --build` — container starts.
  - `docker compose logs` shows the startup line (containing
    `pair=ethbtc port=50051`, matching what Phase 2 proved on the host).
  - Container exits with status 0 (`docker compose ps -a` or `docker inspect
    --format '{{.State.ExitCode}}' keyrock-case-study` reads `0`).
- Done when:
  - All three checks above hold with no manual `command:` override.

### 3.6 Update `compose.yml`'s header comment
- Files or areas: `compose.yml`
- Change: Update the header comment (currently: "There is no long-running
  service yet, so there is nothing to `up -d`.") to also mention `docker
  compose up --build` as a valid way to run the binary once (it starts, logs,
  and exits — still not a server), alongside the existing `docker compose run
  --rm app <cmd>` examples. No other change to `compose.yml` — the
  `KEYROCK_LOG_LEVEL` environment wiring and the "stays off the `echo`
  network" comment block are unchanged, since `Config::from_env()` was kept
  (per `spec.md`'s decided config shape), not replaced.
- Verification:
  - Read the updated comment back; confirm `docker compose run --rm app
    hello Keyrock` is NOT left as a still-implied example anywhere (that
    subcommand no longer exists after Phase 2).
- Done when:
  - The comment block accurately describes both `docker compose run --rm app
    [--pair ... --port ...]` and `docker compose up --build` as valid
    invocations, with no reference to the removed `hello`/`doctor`
    subcommands.

### 3.7 Extend `.env.example` for `KEYROCK_PAIR` and the new port default
- Files or areas: `.env.example`
- Change: Add a `KEYROCK_PAIR=ethbtc` line (with a one-line comment, matching
  the existing style, noting it's the traded pair the aggregator will
  eventually stream — step 0 just plumbs the config field). Change the shown
  `KEYROCK_PORT=8080` to `KEYROCK_PORT=50051` to match the new `Config`
  default from task 2.1.
- Verification:
  - `diff <(grep KEYROCK_ .env.example) <(cat src/config.rs | grep -o 'KEYROCK_[A-Z_]*')`
    or an equivalent manual check confirming every `KEYROCK_*` variable read
    by `Config::from_env()` (`LOG_LEVEL`, `HOST`, `PORT`, `PAIR`) has a
    corresponding line in `.env.example`.
- Done when:
  - `.env.example` has four `KEYROCK_*` lines (`LOG_LEVEL`, `HOST`, `PORT`,
    `PAIR`), with `PORT` defaulting to `50051`.

### 3.8 Re-run `cargo test` after container-only edits
- Files or areas: none — verification-only task, per `plan.md` Phase 3
- Change: none.
- Verification:
  - `cargo test` — still green, confirming none of the Docker/`.env.example`
    edits reached back into `src/` (there should be no `src/` diff in this
    phase at all).
- Done when:
  - The host test suite passes with the exact same test count as Phase 2's
    close-out.

**Commit boundary:** `Dockerfile`, `compose.yml`, `.env.example`. Nothing in
`src/`, `tests/`, `Cargo.toml`, or `proto/` changes in this phase.

---

## Phase 4: Documentation

### 4.1 Update README "Quick start" for the new CLI surface
- Files or areas: `README.md`
- Change: Replace any `hello`/`doctor` command examples with
  `cargo run -- --pair ethbtc --port 50051` and plain `cargo run --` (both
  valid now that both flags default). Replace
  `docker compose run --rm app hello Keyrock` with the equivalent using the
  new binary (e.g. `docker compose run --rm app --pair ethbtc --port 50051`
  and `docker compose up --build`).
- Verification: covered by task 4.4 below (every command shown gets actually
  run).
- Done when:
  - No `hello`/`doctor`/`--verbose` reference remains anywhere in the Quick
    start section.

### 4.2 Update the README configuration table and add precedence + proto notes
- Files or areas: `README.md`
- Change:
  - In the configuration/env-var table: keep the existing `KEYROCK_LOG_LEVEL`
    / `KEYROCK_HOST` rows, add a `KEYROCK_PAIR` row (default `ethbtc`), and
    change the shown `KEYROCK_PORT` default from `8080` to `50051`.
  - Add one sentence stating the precedence rule: `--pair`/`--port` override
    the matching `KEYROCK_*` env var when both are given.
  - Add a one-line mention of `proto/orderbook.proto` under whatever section
    documents repo layout (e.g. "Layout" or "Project structure").
  - Add a placeholder heading "What would change for production" with no
    invented content beyond the heading itself (per `spec.md`'s instruction
    that any opinion about the `.proto` schema belongs there later, not
    fabricated now).
- Verification: covered by task 4.4.
- Done when:
  - The configuration table has 4 rows (`LOG_LEVEL`, `HOST`, `PORT` at
    `50051`, `PAIR` at `ethbtc`), the precedence sentence is present, and the
    two new headings/mentions exist.

### 4.3 Remove stale README content tied to the old scaffold
- Files or areas: `README.md`
- Change: Delete any remaining documentation of the `hello`/`doctor`
  subcommands, the `--verbose` flag, or the old 4-test `tests/cli.rs`
  description (update the test count/description to match Phase 2's actual
  6-case suite, keeping `README.md`'s test-count claim consistent with what
  `cargo test` actually reports).
- Verification:
  - `grep -n "hello\|doctor\|verbose" README.md` — no remaining hits tied to
    the old CLI (a mention in "what changed"/changelog-style prose, if any,
    is fine; a mention as current behavior is not).
- Done when:
  - `README.md` describes only the current `--pair`/`--port` CLI and current
    test count.

### 4.4 Run every command shown in the updated README and confirm output matches
- Files or areas: none — verification-only task, per `plan.md` Phase 4
- Change: none.
- Verification:
  - Run each command literally as written in the updated "Quick start"
    section (`cargo run --`, `cargo run -- --pair ethbtc --port 50051`, the
    `docker compose` examples) and compare actual stdout/stderr/exit code
    against what the README claims happens.
- Done when:
  - Every command in the README produces output consistent with what's
    documented — no doc drift between what's written and what actually runs.

**Commit boundary:** `README.md` only. Reverting it has no effect on build or
test state.

---

## Final Verification

Before closing the packet, run, from the crate root:

- `cargo build` — succeeds.
- `find target -name orderbook.rs` — returns a file whose contents contain
  `Summary` and `Level`.
- `cargo test` — full suite green (updated `tests/cli.rs`, `src/config.rs`
  unit tests; zero references to `hello`/`doctor`/`greeting`/`VERSION`).
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean.
- `docker compose up --build` — container builds, starts, logs the startup
  line (`pair=ethbtc port=50051` visible via `docker compose logs`), and
  exits with status 0. This is the most representative real behavior path for
  this step: it's the one command a reviewer with nothing but Docker
  installed will actually run.
- `git log` on this branch shows one commit per phase (four total), each
  independently buildable at the point it was made — confirm by checking out
  each phase's commit in turn (or reviewing the diff boundaries) and
  re-running `cargo build`/`cargo test` if there's any doubt.
