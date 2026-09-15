#!/usr/bin/env bash
#
# Each binary is a boundary, not just an entry point.
#
# `docs/rust-migration.md` gives a table whose third column says what each binary
# may *not* link, and says that column is asserted here rather than in review.
# The rule is **no chain and no socket** — not "no alloy". `alloy-primitives` is
# an address and two integers, which `bins/api` links through `aave-positions`
# and is meant to; a provider or an HTTP transport is what it may not.
#
# Reaching across then fails the build rather than passing review, and it fails
# at the link graph rather than at an import: cargo links crates, so a
# dependency added three crates away is still this binary's problem.
#
# Runnable on its own — `./scripts/binary-boundaries.sh` — which is the reason it
# is a file rather than a `run:` block.

set -euo pipefail

# The chain-facing half of alloy. `bins/indexer` is the one binary that will
# link these, and it joins the list below in Phase 3.
readonly CHAIN=(alloy-provider alloy-transport-http alloy-rpc-client alloy-pubsub)

bad=0

fail() {
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    echo "::error file=bins/$1/Cargo.toml::$1 links $2"
  else
    echo "bins/$1/Cargo.toml: $1 links $2" >&2
  fi
  bad=1
}

# Every crate the binary links through normal edges — dev-dependencies are not
# in the shipped graph and a test reaching for a provider is nobody's problem.
forbid() {
  local binary="$1"
  shift

  local linked
  linked=$(cargo tree --package "$binary" --edges normal --prefix none --format '{p}' |
    awk '{print $1}' | sort -u)

  # An `if` rather than `grep … && fail`: under `set -e` the second form exits
  # the script the first time a crate is *absent*, which is the passing case.
  local crate
  for crate in "$@"; do
    if grep -qxF "$crate" <<<"$linked"; then
      fail "$binary" "$crate"
    fi
  done
}

forbid api "${CHAIN[@]}"
forbid migrate "${CHAIN[@]}" axum

exit "$bad"
