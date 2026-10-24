#!/usr/bin/env bash
# Builds the .rpm from the vendored tarball and checks it:
#
#   ci/packages/fedora.sh TARBALL OUTDIR
#
# Runs in the Fedora image's package stage (docker/Dockerfile.fedora), as
# root, with rpmbuild and packaging/fedora/fingerprint-settings.spec.
# Offline: everything comes from the tarball. SOURCE_DATE_EPOCH is the
# tarball's, and rpmbuild uses it for the build time and clamps file times
# to it, for reproducible packages.
# Leaves in OUTDIR: the binary .rpm, the source .rpm, and stub_service for
# the smoke test.
set -euo pipefail

tarball=$(realpath "${1:?usage: fedora.sh TARBALL OUTDIR}")
out=$(realpath -m "${2:?usage: fedora.sh TARBALL OUTDIR}")
mkdir -p "$out"
# A fixed build directory: paths end up in the binary (panic locations), and
# the same paths make the same bytes. Each build runs in a fresh container.
work=/tmp/build-fedora
rm -rf "$work"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT
unset CARGO_TARGET_DIR

tar -xzf "$tarball" -C "$work"
src=$(echo "$work"/fingerprint-settings-*/)
spec=$src/packaging/fedora/fingerprint-settings.spec
export SOURCE_DATE_EPOCH
SOURCE_DATE_EPOCH=$(stat -c %Y "$src/Cargo.toml")

# The License tag must follow the crates compiled in: the summary in the spec
# is compared with the one cargo2rpm prints for this tree.
summary() { sed -n '/BEGIN LICENSE SUMMARY/,/END LICENSE SUMMARY/{/LICENSE SUMMARY/d;s/^# //p}'; }
diff <(summary < "$spec") <(cd "$src" && cargo2rpm --path Cargo.toml license-summary | summary) || {
    echo "the crates' licenses changed: update the License tag and its summary in $spec" >&2
    exit 1
}

top=$work/rpmbuild
mkdir -p "$top/SOURCES"
cp "$tarball" "$top/SOURCES/"
rpmbuild -ba "$spec" \
    --define "_topdir $top" \
    --define "use_source_date_epoch_as_buildtime 1" \
    --define "clamp_mtime_to_source_date_epoch 1" \
    --define "_buildhost reproducible"
cp "$top"/RPMS/*/fingerprint-settings-[0-9]*.rpm "$top"/SRPMS/*.src.rpm "$out/"

(cd "$src" && cargo build --release --locked --offline --example stub_service)
install -m755 "$src/target/release/examples/stub_service" "$out/stub_service"

rpm -qip "$out"/fingerprint-settings-[0-9]*.x86_64.rpm
rpm -qlp "$out"/fingerprint-settings-[0-9]*.x86_64.rpm
rpm -qRp "$out"/fingerprint-settings-[0-9]*.x86_64.rpm
# Every finding fails, warnings included.
rpmlint --strict -r "$src/packaging/fedora/fingerprint-settings.rpmlintrc" "$out"/*.rpm "$spec"
echo "rpmlint: clean"
