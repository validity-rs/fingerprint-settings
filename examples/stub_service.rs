//! The smallest fingerprint service the application starts against: one
//! reader with one enrolled finger, on the bus at ADDRESS. The installed
//! packages are launched against it (ci/packages/smoke.sh), so that their
//! test needs nothing but the package and a bus.
//!
//! Usage: stub_service --address ADDRESS
//!
//! It owns `net.reactivated.Fprint` and answers what the application asks
//! when it opens: `Manager.GetDefaultDevice`, `Device.ListEnrolledFingers`
//! and the device's `name`, `num-enroll-stages` and `scan-type` properties.
//! Every other call fails with D-Bus's UnknownMethod. Each call is printed on
//! standard output, one line each (`call ListEnrolledFingers`), after a
//! first `ready` line once the name is owned: that is what the smoke test
//! waits for.
//!
//! For a service that enrols and verifies, see `e2e/examples/demo_service.rs`.

use std::io::Write;

use zbus::OwnedObjectPath;

const DEVICE: &str = "/net/reactivated/Fprint/Device/0";

fn said(what: &str) {
    let mut out = std::io::stdout().lock();
    // A closed stdout must not take the service down with it.
    let _ = writeln!(out, "{what}").and_then(|()| out.flush());
}

struct Manager;

#[zbus::interface(name = "net.reactivated.Fprint.Manager")]
impl Manager {
    fn get_default_device(&self) -> zbus::fdo::Result<OwnedObjectPath> {
        said("call GetDefaultDevice");
        OwnedObjectPath::try_from(DEVICE).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }
}

struct Device;

#[zbus::interface(name = "net.reactivated.Fprint.Device")]
impl Device {
    fn list_enrolled_fingers(&self, username: &str) -> Vec<String> {
        said(&format!("call ListEnrolledFingers {username:?}"));
        vec!["right-index-finger".into()]
    }

    #[zbus(property, name = "name")]
    fn name(&self) -> String {
        "Stub reader".into()
    }

    #[zbus(property, name = "num-enroll-stages")]
    fn num_enroll_stages(&self) -> i32 {
        5
    }

    #[zbus(property, name = "scan-type")]
    fn scan_type(&self) -> String {
        "press".into()
    }
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let address = match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--address"), Some(a), None) => a,
        _ => return Err("usage: stub_service --address ADDRESS".into()),
    };
    zbus::block_on(async {
        let _conn = zbus::connection::Builder::address(address.as_str())
            .serve_at("/net/reactivated/Fprint/Manager", Manager)
            .serve_at(DEVICE, Device)
            .name("net.reactivated.Fprint")
            .build()
            .await
            .map_err(|e| format!("connecting to {address}: {e}"))?;
        said("ready");
        // Serves until killed.
        std::future::pending::<()>().await;
        Ok(())
    })
}
