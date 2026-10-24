#!/usr/bin/env bash
# Builds the .apk from the vendored tarball and checks it:
#
#   ci/packages/alpine.sh TARBALL OUTDIR
#
# Runs in the Alpine image's package stage (docker/Dockerfile.alpine), as
# root; abuild itself runs as "builder". The APKBUILD is written from
# packaging/alpine/APKBUILD.in with the tarball's version and SHA-512, and
# abuild checks the tarball against it. The packages are signed with a key
# made for this build: CI's .apk files are for CI (the release's signing key
# is a decision of its own). Offline: everything comes from the tarball.
# Leaves in OUTDIR: the .apk files, the APKBUILD, the public key, and
# stub_service for the smoke test.
set -euo pipefail

tarball=$(realpath "${1:?usage: alpine.sh TARBALL OUTDIR}")
out=$(realpath -m "${2:?usage: alpine.sh TARBALL OUTDIR}")
mkdir -p "$out"
# A fixed build directory: paths end up in the binary (panic locations), and
# the same paths make the same bytes. Each build runs in a fresh container.
work=/tmp/build-alpine
rm -rf "$work"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT
chmod 755 "$work"
unset CARGO_TARGET_DIR RUSTFLAGS RUSTDOCFLAGS

name=$(basename "$tarball" .tar.gz)
version=${name#fingerprint-settings-}
sum=$(sha512sum "$tarball" | cut -d' ' -f1)
export SOURCE_DATE_EPOCH
tar -xzf "$tarball" -C "$work" "$name/Cargo.toml" "$name/packaging/alpine/APKBUILD.in"
SOURCE_DATE_EPOCH=$(stat -c %Y "$work/$name/Cargo.toml")

# abuild wants the APKBUILD in <repo>/<package>/.
build=$work/ci/fingerprint-settings
mkdir -p "$build" "$work/distfiles" "$work/packages"
sed -e "s/@VERSION@/$version/g" -e "s/@SHA512@/$sum/g" "$work/$name/packaging/alpine/APKBUILD.in" > "$build/APKBUILD"
cp "$tarball" "$work/distfiles/"
chown -R builder: "$work"

as_builder() {
    runuser -u builder -- env HOME=/home/builder CARGO_HOME=/home/builder/.cargo \
        SRCDEST="$work/distfiles" REPODEST="$work/packages" SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" "$@"
}
as_builder abuild-keygen -a -n -q
# abuild indexes the packages it signed with apk, which must trust the key
# (abuild-keygen -i would need root; this script is root, builder is not).
cp /home/builder/.abuild/*.rsa.pub /etc/apk/keys/
apkbuild-lint "$build/APKBUILD"
echo "apkbuild-lint: clean"
# -d: the build dependencies are in the image already (no root here to add them).
(cd "$build" && as_builder abuild -d)

cp "$work"/packages/ci/x86_64/fingerprint-settings-*.apk "$build/APKBUILD" "$out/"
cp /home/builder/.abuild/*.rsa.pub "$out/"

src=$build/src/$name
(cd "$src" && RUSTFLAGS="-C target-feature=-crt-static" cargo build --release --locked --offline --example stub_service)
install -m755 "$src/target/release/examples/stub_service" "$out/stub_service"

ls -l "$out"
