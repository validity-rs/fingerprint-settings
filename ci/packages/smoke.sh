#!/bin/sh
# Launches the installed application headless and checks that it comes up:
#
#   ci/packages/smoke.sh STUB_SERVICE
#
# A private bus (used as both the session and the system bus), the stub
# fingerprint service on it (examples/stub_service.rs, built with the
# package), and a virtual X server (Xvfb). Then /usr/bin/fingerprint-settings
# (or $FINGERPRINT_SETTINGS) is started, and the test passes when:
#   1. the application asked the service for the fingerprints
#      (Manager.GetDefaultDevice, then Device.ListEnrolledFingers): its window
#      came up and its D-Bus side works;
#   2. a window named "Fingerprints" is mapped on the X server;
#   3. the application is still running, and its log holds no panic and no
#      GLib/GTK critical message.
# Every step has a deadline. Needs: dbus-daemon, Xvfb, xwininfo. Runs in a
# container only: the X server is the container's, never the host's.
set -eu

stub=${1:?usage: smoke.sh STUB_SERVICE}
# The installed binary; FINGERPRINT_SETTINGS=path tries another build.
app=${FINGERPRINT_SETTINGS:-/usr/bin/fingerprint-settings}
dir=$(mktemp -d)
pids=

cleanup() {
    for p in $pids; do kill "$p" 2>/dev/null || true; done
    for p in $pids; do wait "$p" 2>/dev/null || true; done
    rm -rf "$dir"
}
trap cleanup EXIT

fail() {
    echo "smoke test FAILED: $*" >&2
    for f in bus stub xvfb app; do
        [ -s "$dir/$f.log" ] && { echo "--- $f.log" >&2; cat "$dir/$f.log" >&2; }
    done
    exit 1
}

# wait_for SECONDS DESCRIPTION COMMAND...: polls COMMAND every 0.2 s. Once
# the application is started ($app_pid), its exit ends the wait at once.
app_pid=
wait_for() {
    secs=$1 what=$2
    shift 2
    n=$((secs * 5))
    while ! "$@" >/dev/null 2>&1; do
        [ -z "$app_pid" ] || alive "$app_pid" || fail "the application exited while waiting for $what"
        n=$((n - 1))
        [ "$n" -gt 0 ] || fail "no $what after ${secs}s"
        sleep 0.2
    done
    echo "ok: $what"
}

alive() { kill -0 "$1" 2>/dev/null; }

# Only in a container (docker: /.dockerenv, podman: /run/.containerenv): the
# X server, the bus and the X socket directory are the container's own, and
# nothing of the host's session is touched.
[ -e /.dockerenv ] || [ -e /run/.containerenv ] || {
    echo "smoke.sh runs in a container only (ci/run.sh package:<distro>)" >&2
    exit 2
}
[ -x "$app" ] || fail "$app is not installed"
[ -x "$stub" ] || fail "$stub is not executable"

# The bus.
cat > "$dir/bus.conf" <<CONF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path=$dir/bus</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
CONF
dbus-daemon --config-file="$dir/bus.conf" --nofork --print-address=1 > "$dir/bus.addr" 2> "$dir/bus.log" &
pids="$pids $!"
wait_for 10 "bus address" test -s "$dir/bus.addr"
bus=$(head -n 1 "$dir/bus.addr")

# The service.
"$stub" --address "$bus" > "$dir/stub.log" 2>&1 &
stub_pid=$!
pids="$pids $stub_pid"
wait_for 10 "stub service on the bus" grep -qx ready "$dir/stub.log"

# The X server, on the first free display from :99.
display=99
while [ -e "/tmp/.X11-unix/X$display" ] || [ -e "/tmp/.X$display-lock" ]; do display=$((display + 1)); done
Xvfb ":$display" -screen 0 1280x1024x24 -nolisten tcp > "$dir/xvfb.log" 2>&1 &
pids="$pids $!"
wait_for 10 "X server :$display" test -S "/tmp/.X11-unix/X$display"

# The application. GSK_RENDERER=cairo: Xvfb has no GPU (GTK would fall back
# to it anyway, with a warning). GTK_A11Y=none: no accessibility bus here.
# LANGUAGE=en: the window title is checked.
env DISPLAY=":$display" \
    DBUS_SESSION_BUS_ADDRESS="$bus" DBUS_SYSTEM_BUS_ADDRESS="$bus" \
    GSK_RENDERER=cairo GTK_A11Y=none LANGUAGE=en LANG=C.UTF-8 \
    XDG_CONFIG_HOME="$dir/config" XDG_RUNTIME_DIR="$dir" \
    "$app" > "$dir/app.log" 2>&1 &
app_pid=$!
pids="$pids $app_pid"

wait_for 60 "Manager.GetDefaultDevice call" grep -q '^call GetDefaultDevice' "$dir/stub.log"
wait_for 30 "Device.ListEnrolledFingers call" grep -q '^call ListEnrolledFingers' "$dir/stub.log"
alive "$app_pid" || fail "the application exited"
wait_for 30 'window "Fingerprints"' sh -c "DISPLAY=:$display xwininfo -root -tree | grep -q '\"Fingerprints\"'"
DISPLAY=":$display" xwininfo -root -tree | grep '"Fingerprints"'

# Let it run a little longer: a crash right after the first frame counts.
sleep 2
alive "$app_pid" || fail "the application exited"
alive "$stub_pid" || fail "the stub service exited"
if grep -Eq 'panicked|-CRITICAL|-ERROR' "$dir/app.log"; then
    fail "the application logged a panic or a critical error"
fi

echo "--- stub service saw:"
cat "$dir/stub.log"
echo "--- application log:"
cat "$dir/app.log"
echo "smoke test passed"
