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

The store suites need real servers. `docker compose up -d clickhouse postgres`, then:

```bash
CLICKHOUSE_USER=aave CLICKHOUSE_PASSWORD=aave CLICKHOUSE_DATABASE=aave \
POSTGRES_URL='postgres://aave:aave@localhost:5432/aave' cargo test --workspace
```

CI runs the same two images with no password at all — `CLICKHOUSE_SKIP_USER_SETUP` and
`POSTGRES_HOST_AUTH_METHOD: trust` — and sets none of these variables, so the readers' own defaults
apply there. A suite that passes only locally is reading something CI will not give it.

## Comments

**Budget: one line per comment, unless it names a specific failure a future edit could bring back.
One to three lines for a module header.** The whole tree sat at 25% comment lines once and had to
lose 824 of them; it is at 19% now and that is not a ceiling to grow back into.

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
- One increment per branch and pull request, pushed without being asked. The PR body is a standalone
  artefact: what changed, the decisions that need a reason, the cost, and what was verified. The
  repository squashes from commit messages, so keep those readable too.
- Scaffolding copied a third time is evidence about the API, not about the tests. Fix the API; a
  shared dev-only crate is the last resort. Where the duplication genuinely remains, say why.
