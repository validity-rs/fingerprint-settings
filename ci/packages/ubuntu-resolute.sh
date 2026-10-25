#!/usr/bin/env bash
# Builds the Ubuntu 26.04 (resolute) .deb from the vendored tarball and checks
# it: the Debian build (ci/packages/debian.sh), in this release's package stage
# (docker/Dockerfile.ubuntu-resolute), with the Debian revision suffixed with
# the release, as validity-rs's APT repository files it (its
# ci/packages/suites.sh): fingerprint-settings_<v>-1.ubuntu26.04_amd64.deb.
#
#   ci/packages/ubuntu-resolute.sh TARBALL OUTDIR
set -euo pipefail
exec "$(dirname "$0")/debian.sh" "${1:?usage: ubuntu-resolute.sh TARBALL OUTDIR}" \
    "${2:?usage: ubuntu-resolute.sh TARBALL OUTDIR}" 1.ubuntu26.04
