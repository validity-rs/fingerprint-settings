#!/usr/bin/env bash
# Builds the Arch package from the vendored tarball and checks it:
#
#   ci/packages/archlinux.sh TARBALL OUTDIR
#
# Runs in the Arch image's package stage (docker/Dockerfile.archlinux), as
# root; makepkg itself runs as "builder" (it refuses root). The PKGBUILD is
# written from packaging/archlinux/PKGBUILD.in with the tarball's version and
# SHA-256, and makepkg checks the tarball against it. Offline: everything
# comes from the tarball. SOURCE_DATE_EPOCH is the tarball's.
# Leaves in OUTDIR: the package, its PKGBUILD and .SRCINFO (for the AUR), and
# stub_service for the smoke test.
set -euo pipefail

tarball=$(realpath "${1:?usage: archlinux.sh TARBALL OUTDIR}")
out=$(realpath -m "${2:?usage: archlinux.sh TARBALL OUTDIR}")
mkdir -p "$out"
# A fixed build directory: paths end up in the binary (panic locations), and
# the same paths make the same bytes. Each build runs in a fresh container.
work=/tmp/build-archlinux
rm -rf "$work"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT
chmod 755 "$work"
unset CARGO_TARGET_DIR RUSTFLAGS RUSTDOCFLAGS

name=$(basename "$tarball" .tar.gz)
version=${name#fingerprint-settings-}
sum=$(sha256sum "$tarball" | cut -d' ' -f1)

tar -xzf "$tarball" -C "$work"
src=$work/$name
export SOURCE_DATE_EPOCH
SOURCE_DATE_EPOCH=$(stat -c %Y "$src/Cargo.toml")

build=$work/build
mkdir "$build"
sed -e "s/@VERSION@/$version/g" -e "s/@SHA256@/$sum/g" "$src/packaging/archlinux/PKGBUILD.in" > "$build/PKGBUILD"
cp "$tarball" "$build/"
chown -R builder: "$build"
(cd "$build" && runuser -u builder -- env HOME=/home/builder CARGO_HOME=/home/builder/.cargo SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH" makepkg --nodeps --noconfirm)
(cd "$build" && runuser -u builder -- makepkg --printsrcinfo) > "$build/.SRCINFO"
cp "$build"/fingerprint-settings-[0-9]*.pkg.tar.zst "$build/PKGBUILD" "$build/.SRCINFO" "$out/"

(cd "$src" && cargo build --release --locked --offline --example stub_service)
install -m755 "$src/target/release/examples/stub_service" "$out/stub_service"

pkg=$(ls "$out"/fingerprint-settings-[0-9]*.pkg.tar.zst)
pacman -Qip "$pkg"
pacman -Qlp "$pkg"
# namcap prints its findings and exits 0. They must be exactly the expected
# ones (packaging/archlinux/namcap-expected, each with its reason).
namcap "$out/PKGBUILD" "$pkg" | sort > "$work/namcap"
grep -v '^#' "$src/packaging/archlinux/namcap-expected" | sed '/^$/d' | sort > "$work/expected"
cat "$work/namcap"
if ! diff -u "$work/expected" "$work/namcap"; then
    echo "namcap: findings differ from packaging/archlinux/namcap-expected (- expected, + found)" >&2
    exit 1
fi
echo "namcap: only the expected findings"
