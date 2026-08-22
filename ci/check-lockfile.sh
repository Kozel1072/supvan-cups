#!/usr/bin/env bash
#
# Reject a Cargo.lock that pins a dependency to a local path.
#
# Local development points ipp-printer-app at a sibling checkout through a
# [patch.crates-io] in .cargo/config.toml. That patch strips source+checksum
# from the lockfile on every build, and a lockfile in that state is
# unresolvable for anyone without the sibling directory — the reason master
# sat unpushable until ipp-printer-app 0.9.0 was published.
#
# Every [[package]] needs a `source`, except this workspace's own crates.
#
# Usage: check-lockfile.sh [path]   ("-" reads stdin, for `git show :Cargo.lock`)

set -euo pipefail

# Resolve the argument before moving, so a relative path still works.
lockfile="${1:-Cargo.lock}"
if [[ "$lockfile" != "-" ]]; then
    lockfile=$(realpath "$lockfile")
fi

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# Workspace members legitimately carry no source.
local_crates=$(sed -n 's/^name = "\(.*\)"/\1/p' crates/*/Cargo.toml | sort -u)

offenders=$(
    awk -v locals="$local_crates" '
        BEGIN { split(locals, a, "\n"); for (i in a) is_local[a[i]] = 1 }
        function flush() { if (name != "" && !sourced && !is_local[name]) print name }
        /^\[\[package\]\]/ { flush(); name = ""; sourced = 0; next }
        /^name = / { gsub(/^name = "|"$/, ""); name = $0; next }
        /^source = / { sourced = 1; next }
        END { flush() }
    ' "$lockfile"
)

if [[ -n "$offenders" ]]; then
    echo "error: Cargo.lock pins these packages to a local path:" >&2
    sed 's/^/  - /' <<<"$offenders" >&2
    echo >&2
    echo "A clone cannot resolve them. Restore the registry lockfile with:" >&2
    echo "  cargo build --workspace --config 'patch.crates-io={}'" >&2
    echo "or check out the committed one:  git checkout Cargo.lock" >&2
    exit 1
fi

echo "Cargo.lock: all dependencies resolve from a registry"
