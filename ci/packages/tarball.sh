#!/usr/bin/env bash
# Builds the vendored source tarball every package is built from:
#
#   ci/packages/tarball.sh OUTDIR   ->  OUTDIR/fingerprint-settings-VERSION.tar.gz
#
# The tarball holds the tracked files (as they are in the working tree; a
# release is built from a clean checkout of its tag), every crate the build
# needs under vendor/, and .cargo/config.toml pointing cargo at them, so that
# the packages build offline: `cargo build --release --locked --offline`.
#
# Reproducible: every entry has the mtime of the last commit
# (SOURCE_DATE_EPOCH, unless already set), owner 0, sorted names, normalised
# modes, and gzip stores no name or time. Building it twice gives the same
# bytes (the release workflow checks it).
#
# Runs in the Debian image (ci/run.sh tarball): needs git, cargo, GNU tar and
# gzip, and crates.io.
set -euo pipefail

out=${1:?usage: tarball.sh OUTDIR}
cd "$(git rev-parse --show-toplevel)"
mkdir -p "$out"
out=$(cd "$out" && pwd)

version=$(sed -n '/^\[package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' Cargo.toml)
[ -n "$version" ] || { echo "no version in Cargo.toml" >&2; exit 1; }
name=fingerprint-settings-$version
epoch=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/$name"

# Tracked files that still exist (a file deleted but not yet committed is left out).
git ls-files -z --cached | while IFS= read -r -d '' f; do
    [ -e "$f" ] && printf '%s\0' "$f"
done | tar --null -T - -cf - | tar -xf - -C "$work/$name"

(
    cd "$work/$name"
    mkdir -p .cargo
    # cargo vendor prints the source replacement to use the vendored crates.
    # Its progress (one line per crate) is shown only if it fails.
    if ! cargo vendor --locked --versioned-dirs vendor > .cargo/config.toml 2> "$work/vendor.log"; then
        cat "$work/vendor.log" >&2
        exit 1
    fi
    grep -q '^directory = "vendor"$' .cargo/config.toml \
        || { echo "cargo vendor printed no source replacement" >&2; exit 1; }
)

tar --sort=name --mtime="@$epoch" --owner=0 --group=0 --numeric-owner \
    --mode='u+rwX,go+rX,go-w' --format=pax \
    --pax-option=exthdr.name=%d/PaxHeaders/%f,delete=atime,delete=ctime \
    -C "$work" -cf - "$name" | gzip -9n > "$out/$name.tar.gz"

(cd "$out" && sha256sum "$name.tar.gz")
