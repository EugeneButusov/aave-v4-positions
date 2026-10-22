# Working in this repository

A TypeScript service being ported to Rust, phase by phase. `apps/` and `packages/` are the
TypeScript still in production; `crates/` and `bins/` are the port. `docs/rust-migration.md` is the
plan and the record, `docs/design-notes.md` the long-form reasoning.

## The gate

Everything below passes before a commit is pushed:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --no-fail-fast
cargo build --workspace --release
cargo deny check
./scripts/binary-boundaries.sh
```

The store suites need real servers, and the readiness cases need a migrated schema in the two
databases those variables name. `docker compose up -d clickhouse postgres`, then:

```bash
CLICKHOUSE_USER=aave CLICKHOUSE_PASSWORD=aave CLICKHOUSE_DATABASE=aave \
POSTGRES_URL='postgres://aave:aave@localhost:5432/aave' cargo run -p migrate
CLICKHOUSE_USER=aave CLICKHOUSE_PASSWORD=aave CLICKHOUSE_DATABASE=aave \
POSTGRES_URL='postgres://aave:aave@localhost:5432/aave' cargo test --workspace
```

CI runs the same two images with no password at all — `CLICKHOUSE_SKIP_USER_SETUP` and
`POSTGRES_HOST_AUTH_METHOD: trust` — and sets none of these variables, so the readers' own defaults
apply there. A suite that passes only locally is reading something CI will not give it.

## Comments

**The code is the explanation. A comment earns its place by saying what the code cannot.**

**Budget: one line per comment, unless it names a specific failure a future edit could bring back.
One to three lines for a module header.** The whole tree sat at 25% comment lines once and had to
lose 824 of them; it is at 16% now and that is a floor to stay near, not a ceiling to grow back
into. `grep -cE '^\s*(///|//!|//)'` over `crates bins` is the check.

Before writing one, try the three cheaper fixes: a better name, a smaller function, a type that
makes the wrong thing unrepresentable. A paragraph explaining a parameter usually means the
parameter is wrong — a `Deadline` enum beats a comment warning that `None` means unlimited.

Keep a comment when it prevents a real regression — a library or protocol fact that makes
odd-looking code correct. Drop it when it explains the current shape:

- **The reasoning for a change belongs in the commit message, not beside the code.** Once the change
  lands, the code is the decision and nobody is asking. This is the failure that recurs most.
- A language fact a Rust reader already knows is noise. A `#[derive]` being required, `str::parse`
  inferring its target, a trait needing to be in scope — all noise.
- "This is deliberately not X" and "not a duplicate of Y" go, unless something would actually
  reintroduce X.
- A module header does not enumerate its own submodules; the `mod` declarations below already do.
- Measurements and version-choice rationale belong in the PR body or `docs/`, not in a doc comment.
- **A descriptive test name is the test's documentation.** A `///` above
  `refuses_a_chain_id_past_the_protocol_bound` restating that it refuses one is a second copy of
  the name. Write the doc only where the name cannot carry the _why_.
- Restating a signature is noise: "takes the config by value", "returns `None` when absent",
  "infallible" where the type already says so.

**A `///` is not always a comment.** On a type deriving `ToSchema` or `IntoParams` it is the
published schema description, so a maintainer's note there ships to callers. Check the derives
before writing one; put the note on a `//` line above the item instead.

## Code

- `lib.rs` is a barrel: `mod` declarations and re-exports only. The same holds for any module with
  submodules, except where a nested path carries information worth reading at the call site.
- Visibility is private or `pub(crate)`. `pub(super)` only when the module is more visible than the
  item. A binary crate's `pub` claims a reach it does not have, and `unreachable_pub` is denied.
- Port to the target idiom; never transliterate. Research what the Rust ecosystem does, cite the
  reference beside the code it shaped — `docs/rust-migration.md` has the table — and say when a
  reference was read and then declined.
- The Rust does not name the TypeScript it replaces. Comparisons live in the migration doc.

## Changes

- **Measure before asserting.** Every claim about cost, behaviour or a dependency is checked, not
  recalled. Read the dependency's source or compile a probe.
- **Break every guard on purpose and watch it fail.** A test that passes against broken code is not
  a test. Say which case caught which break.
- One increment per branch and pull request, pushed without being asked. **The commit message is
  the standalone artefact** — it is what the squash puts in history, so the full reasoning, the
  measurements and the rejected alternatives go there. The PR body is something else; see below.
- Scaffolding copied a third time is evidence about the API, not about the tests. Fix the API; a
  shared dev-only crate is the last resort. Where the duplication genuinely remains, say why.

## Pull request descriptions

**The body is an invitation to review, not the record of the change.** Its job is to get a careful
reader to open the diff and look at the right places. A body nobody finishes is worse than a short
one, because the alternative to reading is approving unread.

**Budget: a screenful — under 25 rendered lines, which is roughly 250 words. No horizontal rules.**
The first five PRs here averaged 1,000 words, a five-to-seven minute read before anyone could
approve; that is the failure this section exists to stop. The line count is the real constraint and
the word count is its proxy, so prefer cutting a whole point to compressing every sentence.

Four parts, in this order, and none of them a wall:

1. **Why you are being asked to look** — one or two sentences. The defect, not the mechanism.
2. **Where to look** — the two or three places that deserve real attention, with paths. Say which
   one a reviewer might reasonably disagree with, and put that first. Disagreement should be cheap.
3. **What might bite** — anything a reader would want to object to and cannot see from the diff: a
   dropped capability, a widened contract, a residual hazard. One line each.
4. **Verified** — one line. The gate ran; the numbers that moved.

Not in the body, because they are in the commit message: the mechanism in full, the break table,
the measurements, the alternatives considered and declined, the things deliberately out of scope.
Link to the commit for anyone who wants them.

Prose over tables; a table of five rows reads as a form to skim past. No section that exists only
for completeness — if nothing bites, say nothing instead of writing "None."
