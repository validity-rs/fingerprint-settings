#!/bin/sh
# The package's life in a fresh system: install, check, launch, remove.
#
#   ci/packages/install.sh DISTRO DIR
#
# Runs as root in the distro's install stage (docker/Dockerfile.DISTRO), a
# plain image of the distro plus the smoke test's tools. DIR holds what the
# package build left (ci/packages/DISTRO.sh): the package, stub_service and,
# for Alpine, the public key the .apk is signed with.
#
#   1. install the package with the distro's package manager, which pulls its
#      dependencies from the distro's repositories;
#   2. validate the installed desktop entry and metainfo, check that the
#      binary, icon and manual page are where the menus and man expect them;
#   3. launch it headless against the stub service (ci/packages/smoke.sh);
#   4. remove it, and check that none of its files is left.
set -eu

distro=${1:?usage: install.sh DISTRO DIR}
dir=$(cd "${2:?usage: install.sh DISTRO DIR}" && pwd)
here=$(cd "$(dirname "$0")" && pwd)
app_id=io.github.validity_rs.FingerprintSettings
files=$(mktemp)

say() { printf '\n== %s\n' "$*"; }

say "install ($distro)"
case $distro in
debian | ubuntu-*)
    apt-get update -q
    apt-get install -y --no-install-recommends "$dir"/fingerprint-settings_*.deb
    dpkg-query -W fingerprint-settings
    dpkg --verify fingerprint-settings
    dpkg -L fingerprint-settings > "$files"
    ;;
fedora)
    # The container image installs no documentation (tsflags=nodocs); a
    # system does, and the manual page is checked.
    dnf install -y --setopt=tsflags= "$dir"/fingerprint-settings-[0-9]*.x86_64.rpm
    rpm -q fingerprint-settings
    rpm -V fingerprint-settings
    rpm -ql fingerprint-settings > "$files"
    ;;
archlinux)
    # The container image extracts no documentation (NoExtract in
    # pacman.conf); a system does, and the manual page is checked.
    sed -i '/^NoExtract/d' /etc/pacman.conf
    pacman -U --noconfirm "$dir"/fingerprint-settings-[0-9]*.pkg.tar.zst
    pacman -Q fingerprint-settings
    pacman -Qkk fingerprint-settings
    pacman -Qlq fingerprint-settings > "$files"
    ;;
alpine)
    cp "$dir"/*.rsa.pub /etc/apk/keys/
    apk add "$dir"/fingerprint-settings-[0-9]*.apk "$dir"/fingerprint-settings-doc-[0-9]*.apk
    apk info -e fingerprint-settings fingerprint-settings-doc
    apk audit --packages 2>&1 | grep fingerprint-settings && { echo "apk audit: changed files" >&2; exit 1; }
    apk info -qL fingerprint-settings fingerprint-settings-doc | sed '/^$/d; s|^|/|' > "$files"
    ;;
*)
    echo "unknown distro: $distro" >&2
    exit 2
    ;;
esac

say "installed files"
cat "$files"
for f in \
    /usr/bin/fingerprint-settings \
    /usr/share/applications/$app_id.desktop \
    /usr/share/metainfo/$app_id.metainfo.xml \
    /usr/share/icons/hicolor/scalable/apps/$app_id.svg; do
    [ -f "$f" ] || { echo "missing: $f" >&2; exit 1; }
done
ls /usr/share/man/man1/fingerprint-settings.1* >/dev/null || { echo "missing: manual page" >&2; exit 1; }

say "validate the installed desktop entry and metainfo"
desktop-file-validate /usr/share/applications/$app_id.desktop
appstreamcli validate --no-net /usr/share/metainfo/$app_id.metainfo.xml

say "headless launch"
sh "$here/smoke.sh" "$dir/stub_service"

say "remove"
case $distro in
debian | ubuntu-*) apt-get purge -y fingerprint-settings ;;
fedora) dnf remove -y fingerprint-settings ;;
archlinux) pacman -R --noconfirm fingerprint-settings ;;
alpine) apk del fingerprint-settings fingerprint-settings-doc ;;
esac
left=0
while IFS= read -r f; do
    # Directories may be shared with other packages; files may not stay.
    if [ -e "$f" ] && [ ! -d "$f" ]; then echo "left after removal: $f" >&2; left=1; fi
done < "$files"
[ "$left" -eq 0 ] || exit 1
[ ! -e /usr/bin/fingerprint-settings ] || { echo "/usr/bin/fingerprint-settings still there" >&2; exit 1; }
echo "removed, no file left"
say "install, launch and removal passed ($distro)"
