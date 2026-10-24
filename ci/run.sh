#!/usr/bin/env bash
# The CI, runnable locally. GitHub Actions calls the same jobs, so a green local
# run means a green CI run.
#
#   ci/run.sh                 # every job
#   ci/run.sh clippy test     # selected jobs
#
# The Rust jobs (fmt clippy doc test e2e) run in the pinned Debian 13 image
# (docker/Dockerfile.debian) when docker is there; CI_HOST=1 runs them on the
# host instead (needs Rust, libgtk-4-dev, libadwaita-1-dev, dbus-daemon).
#
# The end-to-end test is a package of its own (e2e/), which depends on
# validity-rs's daemon by git URL and commit (e2e/Cargo.toml): cargo fetches
# it from GitHub, and Cargo.lock is checked as it is (--locked). Nothing else
# needs validity-rs.
#
# Tools that are not cargo subcommands are downloaded once, checked against a
# pinned SHA-256 and cached in $CI_TOOLS (default ~/.cache/fingerprint-settings-ci).
# When bumping a version, bump its checksum with it.
set -euo pipefail

GITLEAKS_VERSION=8.30.1
GITLEAKS_SHA256=551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb
ZIZMOR_VERSION=1.31.1
ZIZMOR_SHA256=d185f726bdaf92df78273217dfdd24ebcd6b0ef48cf3ce776ff5762b6e138c99
IMAGE=fingerprint-settings-ci-debian

cd "$(git rev-parse --show-toplevel)"
# No warning is tolerated, in any cargo invocation.
export RUSTFLAGS="-D warnings" RUSTDOCFLAGS="-D warnings"
CI_TOOLS=${CI_TOOLS:-${XDG_CACHE_HOME:-$HOME/.cache}/fingerprint-settings-ci}
mkdir -p "$CI_TOOLS"
APP_ID=io.github.validity_rs.FingerprintSettings

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

# fetch <url> <sha256> <file>: download into $CI_TOOLS once, verify every time.
fetch() {
    local url=$1 sum=$2 out="$CI_TOOLS/$3"
    [ -f "$out" ] || curl -fsSL --retry 5 --retry-all-errors -o "$out.part" "$url" && { [ -f "$out" ] || mv "$out.part" "$out"; }
    echo "$sum  $out" | sha256sum -c --quiet -
}

tool_gitleaks() {
    local a="gitleaks_${GITLEAKS_VERSION}_linux_x64.tar.gz"
    fetch "https://github.com/gitleaks/gitleaks/releases/download/v${GITLEAKS_VERSION}/$a" "$GITLEAKS_SHA256" "$a"
    [ -x "$CI_TOOLS/gitleaks-$GITLEAKS_VERSION" ] || {
        tar -xzf "$CI_TOOLS/$a" -C "$CI_TOOLS" gitleaks
        mv "$CI_TOOLS/gitleaks" "$CI_TOOLS/gitleaks-$GITLEAKS_VERSION"
    }
    echo "$CI_TOOLS/gitleaks-$GITLEAKS_VERSION"
}

tool_zizmor() {
    local a=zizmor-x86_64-unknown-linux-gnu.tar.gz
    fetch "https://github.com/zizmorcore/zizmor/releases/download/v${ZIZMOR_VERSION}/$a" "$ZIZMOR_SHA256" "zizmor-$ZIZMOR_VERSION.tar.gz"
    [ -x "$CI_TOOLS/zizmor-$ZIZMOR_VERSION" ] || {
        tar -xzf "$CI_TOOLS/zizmor-$ZIZMOR_VERSION.tar.gz" -C "$CI_TOOLS" zizmor
        mv "$CI_TOOLS/zizmor" "$CI_TOOLS/zizmor-$ZIZMOR_VERSION"
    }
    echo "$CI_TOOLS/zizmor-$ZIZMOR_VERSION"
}

use_image() { [ -z "${CI_HOST:-}" ] && command -v docker >/dev/null; }

# Runs a shell command in the Debian image, with this checkout at /src.
# Target directory, crate downloads and cargo's git checkouts live in named
# volumes, so the host's target/ stays free of root-owned files.
in_image() {
    docker build -q -t "$IMAGE" -f docker/Dockerfile.debian docker >/dev/null
    docker run --rm \
        -v "$PWD:/src" -w /src \
        -v fingerprint-settings-target:/target -e CARGO_TARGET_DIR=/target \
        -v fingerprint-settings-cargo:/usr/local/cargo/registry \
        -v fingerprint-settings-cargo-git:/usr/local/cargo/git \
        -e RUSTFLAGS -e RUSTDOCFLAGS -e CARGO_TERM_COLOR -e CARGO_NET_RETRY \
        "$IMAGE" sh -c "rustc --version && $*"
}

run() { if use_image; then in_image "$*"; else sh -c "$*"; fi; }

job_noai() { bash .github/scripts/no-ai-traces.sh HEAD; }
# Every finding fails (the pedantic persona reports all of them).
job_zizmor() { "$(tool_zizmor)" --offline --persona=pedantic .github/workflows; }
job_gitleaks() { "$(tool_gitleaks)" git --redact --exit-code 1 --no-banner .; }
# e2e/ is a package of its own: rustfmt checks its files without resolving
# its dependencies, so that fmt never needs validity-rs.
job_fmt() { run "cargo fmt --all --check && rustfmt --check --edition 2024 e2e/src/*.rs e2e/tests/*.rs e2e/examples/*.rs"; }
job_clippy() { run cargo clippy --locked --all-targets -- -D warnings; }
job_doc() { run cargo doc --locked --no-deps; }
job_test() { run cargo test --locked; }
# The end-to-end test: e2e/ against validity-rs's daemon, at the commit
# e2e/Cargo.toml pins.
job_e2e() {
    if ! use_image; then
        command -v dbus-daemon >/dev/null || {
            echo "dbus-daemon is needed by the end-to-end test (Debian: apt install dbus-daemon)" >&2
            return 1
        }
    fi
    run "cargo clippy --manifest-path e2e/Cargo.toml --locked --all-targets -- -D warnings" \
        "&& cargo test --manifest-path e2e/Cargo.toml --locked"
}
# The desktop entry and the AppStream metainfo (validate with the image's
# tools, or the host's: desktop-file-utils, appstream).
job_packaging() {
    run "desktop-file-validate packaging/common/$APP_ID.desktop" \
        "&& appstreamcli validate --no-net packaging/common/$APP_ID.metainfo.xml"
}

ALL=(noai zizmor gitleaks fmt clippy doc test packaging e2e)

jobs=("$@")
[ ${#jobs[@]} -eq 0 ] && jobs=("${ALL[@]}")

failed=()
for j in "${jobs[@]}"; do
    if ! declare -F "job_$j" >/dev/null; then echo "unknown job: $j" >&2; exit 2; fi
    say "$j"
    if ! "job_$j"; then failed+=("$j"); fi
done

if [ ${#failed[@]} -gt 0 ]; then
    printf '\n\033[31mFAILED: %s\033[0m\n' "${failed[*]}"
    exit 1
fi
printf '\n\033[32mOK: %s\033[0m\n' "${jobs[*]}"
