#!/usr/bin/env bash
# Builds the .deb from the vendored tarball and checks it:
#
#   ci/packages/debian.sh TARBALL OUTDIR [DEB REVISION]
#
# DEB REVISION (default 1) is how an Ubuntu build is told apart
# (ci/packages/ubuntu-resolute.sh).
#
# Runs in the Debian image's package stage (docker/Dockerfile.debian), as
# root. Offline: everything comes from the tarball. SOURCE_DATE_EPOCH is the
# tarball's (its files' mtime), so that the .deb is reproducible.
# Leaves in OUTDIR: the .deb, and stub_service for the smoke test.
set -euo pipefail

tarball=$(realpath "${1:?usage: debian.sh TARBALL OUTDIR}")
out=$(realpath -m "${2:?usage: debian.sh TARBALL OUTDIR}")
revision=${3:-}
mkdir -p "$out"
# A fixed build directory: paths end up in the binary (panic locations), and
# the same paths make the same bytes. Each build runs in a fresh container.
work=/tmp/build-debian
rm -rf "$work"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

tar -xzf "$tarball" -C "$work"
cd "$work"/fingerprint-settings-*/
export SOURCE_DATE_EPOCH
SOURCE_DATE_EPOCH=$(stat -c %Y Cargo.toml)
unset CARGO_TARGET_DIR

cargo deb --locked --offline ${revision:+--deb-revision "$revision"} --output "$out/"
cargo build --release --locked --offline --example stub_service
install -m755 target/release/examples/stub_service "$out/stub_service"

deb=$(ls "$out"/fingerprint-settings_*.deb)

# cargo-deb writes no md5sums, which dpkg --verify and debsums check the
# installed files against: add them, and build the .deb again with dpkg-deb
# (which clamps every time to SOURCE_DATE_EPOCH).
root="$work/deb"
dpkg-deb --raw-extract "$deb" "$root"
(cd "$root" && find . -path ./DEBIAN -prune -o -type f -printf '%P\0' | LC_ALL=C sort -z | xargs -0 md5sum) \
    > "$root/DEBIAN/md5sums"
chmod 644 "$root/DEBIAN/md5sums"
dpkg-deb --root-owner-group -Zxz --build "$root" "$deb"

dpkg-deb --info "$deb"
dpkg-deb --contents "$deb"
# Errors, warnings and informational tags all fail; pedantic ones are shown.
# Not as root (lintian advises against it).
chmod 755 "$out"
chmod 644 "$deb"
cd /
runuser -u nobody -- env HOME=/tmp lintian --fail-on error,warning,info --display-info --pedantic "$deb"
echo "lintian: clean"
