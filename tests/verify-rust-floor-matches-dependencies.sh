#!/usr/bin/env bash
# Verify that the `rustc` / `cargo` floors declared in `repro.nim`'s `uses:`
# block are not below the highest `rust-version` any locked crate declares.
# A `uses:` range is a constraint (reprobuild Package-Model, "Uses Declaration
# Model"), so a floor below a dependency's MSRV admits toolchains that cannot
# build the crate.
#
# Wire-up: `Justfile` (`just lint`, `just test`, `just verify-rust-floor`).
#
# Exit codes:
#   0  both declared floors are >= the highest dependency MSRV
#   1  a floor is missing, unparsable, or too low (reported on stderr)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Highest `rust-version` across every package in the locked graph, as
# "major.minor". `--locked` keeps Cargo.lock authoritative.
required="$(
  cd "${REPO_ROOT}" &&
    cargo metadata --locked --format-version 1 |
      grep -o '"rust_version":"[0-9.]*"' |
      sed 's/.*:"\([0-9]*\.[0-9]*\).*/\1/' |
      sort -t. -k1,1n -k2,2n |
      tail -n 1
)"
if [ -z "${required}" ]; then
  echo "FAIL: no dependency declares a rust-version; cannot compute the floor" >&2
  exit 1
fi

# `a.b >= c.d`?
version_ge() {
  [ "$(printf '%s\n%s\n' "$1" "$2" | sort -t. -k1,1n -k2,2n | head -n 1)" = "$2" ]
}

status=0
for tool in rustc cargo; do
  declared="$(sed -n "s/^[[:space:]]*\"${tool} >=\([0-9][0-9.]*\)\"[[:space:]]*\$/\1/p" \
    "${REPO_ROOT}/repro.nim")"
  if [ -z "${declared}" ]; then
    echo "FAIL: repro.nim declares no \"${tool} >=X.Y\" in uses:" >&2
    status=1
  elif ! version_ge "${declared}" "${required}"; then
    echo "FAIL: repro.nim declares ${tool} >=${declared}, but a locked dependency needs rust-version ${required}" >&2
    status=1
  else
    echo "ok: ${tool} >=${declared} covers the highest dependency rust-version ${required}"
  fi
done
exit "${status}"
