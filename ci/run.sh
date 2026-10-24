#!/usr/bin/env bash
# The CI, runnable locally. GitHub Actions calls the same jobs, so a green local
# run means a green CI run.
#
#   ci/run.sh                    # quick: noai zizmor gitleaks fmt clippy doc test packaging e2e
#   ci/run.sh all                # quick + every distro and every package
#   ci/run.sh clippy test        # selected jobs
#   ci/run.sh distro:fedora      # the tests on one distro (distro: all four)
#   ci/run.sh package:alpine     # one package's full cycle (package: all four)
#
# Distros: debian (the reference, Debian 13), fedora, archlinux, alpine. Each
# has one image, docker/Dockerfile.<distro>, with three stages: `ci` (build
# and test), `package` (build and check the package) and `install` (a fresh
# system, where the package is installed, launched and removed).
#
# Jobs:
#   noai zizmor gitleaks   on the host
#   fmt clippy doc test    in the Debian image (or on the host with CI_HOST=1:
#                          needs Rust, libgtk-4-dev, libadwaita-1-dev)
#   packaging              validates the desktop entry and the metainfo
#   distro:<d>             cargo test in the distro's image
#   e2e                    the end-to-end test (e2e/) against validity-rs's
#                          daemon, in the Debian image (or on the host)
#   tarball                the vendored source tarball, in dist/
#   package:<d>            tarball, package, checks (lintian, rpmlint,
#                          namcap, apkbuild-lint), then install, headless
#                          launch and removal in a fresh container; in
#                          dist/<d>/. REPRODUCE=1 builds the package twice
#                          and fails if the two differ.
#
# The end-to-end test is a package of its own (e2e/), which depends on
# validity-rs's daemon by git URL and commit (e2e/Cargo.toml): cargo fetches
# it from GitHub, and Cargo.lock is checked as it is (--locked). Nothing else
# needs validity-rs.
#
# Tools that are not cargo subcommands are downloaded once, checked against a
# pinned SHA-256 and cached in $CI_TOOLS (default ~/.cache/fingerprint-settings-ci).
# When bumping a version, bump its checksum with it.
#
# To keep docker within limits on a shared machine: CI_DOCKER_RUN_ARGS and
# CI_DOCKER_BUILD_ARGS are added to every `docker run` and `docker build`
# (e.g. "--cpuset-cpus 0-3"; for docker build, with DOCKER_BUILDKIT=0), and
# CARGO_BUILD_JOBS is passed into the containers.
set -euo pipefail

GITLEAKS_VERSION=8.30.1
GITLEAKS_SHA256=551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb
ZIZMOR_VERSION=1.31.1
ZIZMOR_SHA256=d185f726bdaf92df78273217dfdd24ebcd6b0ef48cf3ce776ff5762b6e138c99
DISTROS=(debian fedora archlinux alpine)

cd "$(git rev-parse --show-toplevel)"
# No warning is tolerated, in any cargo invocation.
export RUSTFLAGS="-D warnings" RUSTDOCFLAGS="-D warnings"
CI_TOOLS=${CI_TOOLS:-${XDG_CACHE_HOME:-$HOME/.cache}/fingerprint-settings-ci}
mkdir -p "$CI_TOOLS"
CI_TOOLS=$(cd "$CI_TOOLS" && pwd)
APP_ID=io.github.validity_rs.FingerprintSettings
read -r -a RUN_ARGS <<< "${CI_DOCKER_RUN_ARGS:-}"
read -r -a BUILD_ARGS <<< "${CI_DOCKER_BUILD_ARGS:-}"

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

# image <distro> <stage>: builds docker/Dockerfile.<distro> up to <stage> and
# prints its tag. Image builds pull from public registries and distro
# mirrors: transient failures are retried.
image() {
    local tag="fingerprint-settings-$1-$2" attempt
    for attempt in 1 2 3; do
        if docker build -q "${BUILD_ARGS[@]}" --target "$2" -t "$tag" -f "docker/Dockerfile.$1" docker >/dev/null; then
            echo "$tag"
            return 0
        fi
        [ "$attempt" -lt 3 ] && { echo "image $tag: build attempt $attempt failed, retrying in 30s" >&2; sleep 30; }
    done
    return 1
}

# docker run with this checkout at /src. Files the container writes are given
# back to the caller's user (HOST_UID, HOST_GID: the scripts chown dist/).
drun() {
    docker run --rm "${RUN_ARGS[@]}" \
        -v "$PWD:/src" -w /src \
        -e CARGO_TERM_COLOR -e CARGO_NET_RETRY -e CARGO_BUILD_JOBS \
        -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
        "$@"
}

# in_image <distro> [docker run options --] <shell command>: runs the command
# in the distro's ci stage. The target directory and crate downloads live in
# named volumes per distro (different libc and toolchains make different
# binaries; the host's target/ stays free of root-owned files).
in_image() {
    local d=$1 img
    shift
    # docker run options come first, up to a lone "--" (a command never
    # starts with "-").
    local opts=()
    if [[ ${1:-} == -* ]]; then
        while [ $# -gt 0 ] && [ "$1" != -- ]; do opts+=("$1"); shift; done
        [ $# -gt 0 ] || { echo "in_image: docker options without -- and a command" >&2; return 2; }
        shift
    fi
    img=$(image "$d" ci) || return 1
    local rustflags=$RUSTFLAGS
    # The official image's Rust links statically on musl, and GTK is only
    # available as shared libraries (see docker/Dockerfile.alpine).
    [ "$d" = alpine ] && rustflags="$rustflags -C target-feature=-crt-static"
    drun \
        -v "fingerprint-settings-target-$d:/target" -e CARGO_TARGET_DIR=/target \
        -v "fingerprint-settings-cargo-$d:/usr/local/cargo/registry" \
        -e RUSTFLAGS="$rustflags" -e RUSTDOCFLAGS \
        "${opts[@]}" "$img" sh -c "rustc --version && $*"
}

run() { if use_image; then in_image debian "$*"; else sh -c "$*"; fi; }

# Gives files written as root in a container back to the caller.
# shellcheck disable=SC2016 # expanded by the container's shell, on purpose
give_back() { printf 'chown -R "$HOST_UID:$HOST_GID" %s' "$*"; }

job_noai() { bash .github/scripts/no-ai-traces.sh HEAD; }
job_zizmor() { "$(tool_zizmor)" --offline --persona=pedantic .github/workflows; }
job_gitleaks() { "$(tool_gitleaks)" git --redact --exit-code 1 --no-banner .; }
# e2e/ is a package of its own (see e2e/Cargo.toml): rustfmt checks its files
# without resolving its dependencies, so that fmt never needs validity-rs.
job_fmt() { run "cargo fmt --all --check && rustfmt --check --edition 2024 e2e/src/*.rs e2e/tests/*.rs e2e/examples/*.rs"; }
job_clippy() { run cargo clippy --locked --all-targets -- -D warnings; }
job_doc() { run cargo doc --locked --no-deps; }
job_test() { if use_image; then job_distro debian; else cargo test --locked; fi; }
job_distro() {
    local d
    for d in "${@:-${DISTROS[@]}}"; do
        say "distro $d"
        in_image "$d" "cargo test --locked" || return 1
    done
}
# The desktop entry and the AppStream metainfo (validate with the image's
# tools, or the host's: desktop-file-utils, appstream).
job_packaging() {
    run "desktop-file-validate packaging/common/$APP_ID.desktop" \
        "&& appstreamcli validate --no-net packaging/common/$APP_ID.metainfo.xml"
}

# The end-to-end test: e2e/ against validity-rs's daemon, at the commit
# e2e/Cargo.toml pins. Cargo's git checkouts live in a named volume.
job_e2e() {
    local cmd="cargo clippy --manifest-path e2e/Cargo.toml --locked --all-targets -- -D warnings && cargo test --manifest-path e2e/Cargo.toml --locked"
    if use_image; then
        in_image debian \
            -v fingerprint-settings-cargo-git:/usr/local/cargo/git -e CARGO_TARGET_DIR=/target/e2e \
            -- "$cmd"
    else
        command -v dbus-daemon >/dev/null || {
            echo "dbus-daemon is needed by the end-to-end test (Debian: apt install dbus-daemon)" >&2
            return 1
        }
        sh -c "$cmd"
    fi
}

# The vendored source tarball (ci/packages/tarball.sh), in dist/. With
# REPRODUCE=1 it is built a second time and must be the same bytes.
job_tarball() {
    local img
    img=$(image debian ci) || return 1
    rm -f dist/fingerprint-settings-*.tar.gz
    mkdir -p dist
    drun -v fingerprint-settings-cargo-debian:/usr/local/cargo/registry "$img" \
        sh -c "ci/packages/tarball.sh dist; rc=\$?; $(give_back dist); exit \$rc" || return 1
    if [ -n "${REPRODUCE:-}" ]; then
        drun -v fingerprint-settings-cargo-debian:/usr/local/cargo/registry "$img" \
            sh -c "ci/packages/tarball.sh dist/again; rc=\$?; $(give_back dist); exit \$rc" || return 1
        cmp dist/fingerprint-settings-*.tar.gz dist/again/fingerprint-settings-*.tar.gz || {
            echo "NOT reproducible: the vendored tarball" >&2
            return 1
        }
        echo "reproducible: $(basename dist/fingerprint-settings-*.tar.gz)"
        rm -rf dist/again
    fi
}

# package:<distro>: the tarball, then the package built and checked in the
# package stage, then installed, launched and removed in the install stage.
job_package() {
    local d
    job_tarball || return 1
    for d in "${@:-${DISTROS[@]}}"; do
        say "package $d"
        package_one "$d" || return 1
    done
}

package_one() {
    local d=$1 build install tarball
    tarball=$(ls dist/fingerprint-settings-*.tar.gz)
    build=$(image "$d" package) || return 1
    install=$(image "$d" install) || return 1
    build_package "$build" "$d" "$tarball" "dist/$d" || return 1
    if [ -n "${REPRODUCE:-}" ]; then
        say "package $d: second build, compared with the first"
        build_package "$build" "$d" "$tarball" "dist/$d.again" || return 1
        reproducible "$d" || return 1
    fi
    say "package $d: install, launch, remove in a fresh $d"
    drun "$install" ci/packages/install.sh "$d" "dist/$d"
}

build_package() {
    local img=$1 d=$2 tarball=$3 out=$4
    rm -rf "$out"
    drun "$img" sh -c "ci/packages/$d.sh '$tarball' '$out'; rc=\$?; $(give_back dist); exit \$rc"
}

# The packages of two builds must be the same bytes. Alpine's .apk carries
# its signature (made with a key generated for each image) and the build
# time, so it is compared without its signature.
reproducible() {
    local d=$1 f g bad=0
    for f in "dist/$d"/*; do
        case $f in */stub_service | *.rsa.pub) continue ;; esac
        g="dist/$d.again/$(basename "$f")"
        if cmp -s "$f" "$g"; then
            echo "reproducible: $(basename "$f")"
        else
            echo "NOT reproducible: $(basename "$f")" >&2
            bad=1
        fi
    done
    rm -rf "dist/$d.again"
    return "$bad"
}

QUICK=(noai zizmor gitleaks fmt clippy doc test packaging e2e)
ALL=("${QUICK[@]}" distro:fedora distro:archlinux distro:alpine package)

jobs=("$@")
[ ${#jobs[@]} -eq 0 ] && jobs=("${QUICK[@]}")
[ "${jobs[*]}" = all ] && jobs=("${ALL[@]}")

failed=()
for j in "${jobs[@]}"; do
    name=${j%%:*}
    arg=${j#*:}
    [ "$arg" = "$j" ] && arg=
    fn="job_$name"
    if ! declare -F "$fn" >/dev/null; then echo "unknown job: $j" >&2; exit 2; fi
    say "$j"
    if ! "$fn" ${arg:+"$arg"}; then failed+=("$j"); fi
done

if [ ${#failed[@]} -gt 0 ]; then
    printf '\n\033[31mFAILED: %s\033[0m\n' "${failed[*]}"
    exit 1
fi
printf '\n\033[32mOK: %s\033[0m\n' "${jobs[*]}"
