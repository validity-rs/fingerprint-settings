//! A fingerprint service with an emulated reader, to try the application
//! without hardware: validity-rs's daemon and its fake sensor, on the bus at
//! ADDRESS, allowing everything (no polkit). Development only.
//!
//! Usage: demo_service --address ADDRESS
//!
//! The emulated finger is controlled over the same bus, at
//! `/io/github/validity_rs/FingerprintSettings/Demo`, interface
//! `io.github.validity_rs.FingerprintSettings.Demo`:
//!   Touch(u finger)   the reader sees finger number `finger` once
//!   ScanError()       the reader fails one capture
//!
//! See the README ("Trying it without a reader").

use std::sync::Arc;

use validity_daemon::fake::{FakeSensor, Touch, blobs};
use validity_daemon::polkit::{Authority, Check};
use validity_daemon::service::{self, Bus, Deps, Options, Setup};
use validity_daemon::users::{SidMap, SystemUsers};
use zbus::Connection;

struct AllowAll;

impl Authority for AllowAll {
    fn check<'a>(&'a self, _: &'a Connection, _: &'a str, _: &'a str) -> Check<'a> {
        Box::pin(async { Ok(true) })
    }
}

struct Demo(FakeSensor);

#[zbus::interface(name = "io.github.validity_rs.FingerprintSettings.Demo")]
impl Demo {
    fn touch(&self, finger: u32) {
        self.0.touch([Touch::Finger(finger)]);
    }
    fn scan_error(&self) {
        self.0.touch([Touch::ScanError]);
    }
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let address = match (args.next().as_deref(), args.next()) {
        (Some("--address"), Some(a)) => a,
        _ => return Err("usage: demo_service --address ADDRESS".into()),
    };
    let state =
        std::env::temp_dir().join(format!("fingerprint-settings-demo-{}", std::process::id()));
    std::fs::create_dir_all(&state).map_err(|e| e.to_string())?;
    let fake = FakeSensor::new();
    zbus::block_on(async {
        let control = zbus::connection::Builder::address(address.as_str())
            .serve_at(
                "/io/github/validity_rs/FingerprintSettings/Demo",
                Demo(fake.clone()),
            )
            .name("io.github.validity_rs.FingerprintSettings.Demo")
            .build()
            .await
            .map_err(|e| format!("demo control object: {e}"))?;
        let r = service::run(
            Bus::Address(address.clone()),
            fake.hardware(),
            Setup {
                blobs: Ok(blobs()),
                calibration: state.join("calibration.bin"),
            },
            Deps {
                authority: Arc::new(AllowAll),
                users: Arc::new(SystemUsers),
                sids: SidMap::default(),
            },
            Options::default(),
            service::termination(),
        )
        .await;
        drop(control);
        r
    })
}
