//! Service errors, reduced to what the user can understand and act on.

use zbus::DBusError;

use crate::message::{Message, msg};

/// What went wrong, as far as the user is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// No fingerprint service on the system bus (or no system bus).
    Unavailable,
    /// The service runs but has no reader, or the reader went away.
    NoReader,
    /// Another program holds the reader.
    Busy,
    /// polkit said no.
    PermissionDenied,
    /// The user has no enrolled finger (testing).
    NoPrints,
    /// The finger is enrolled already (enrolling).
    AlreadyEnrolled,
    /// The service lacks the method (an older fprintd): internal, used to
    /// fall back to another call.
    Unsupported,
    /// The print could not be deleted.
    NotDeleted,
    /// The service did not answer in time.
    Timeout,
    /// Anything else.
    Failed,
}

/// A failed service call: its kind and the technical detail (error name and
/// message), shown only on request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceError {
    /// What it means.
    pub kind: ErrorKind,
    /// The D-Bus error name and message, or the transport error.
    pub detail: String,
}

impl ServiceError {
    /// An error of this kind.
    pub fn new(kind: ErrorKind, detail: impl Into<String>) -> Self {
        ServiceError {
            kind,
            detail: detail.into(),
        }
    }

    /// From a D-Bus error reply.
    pub fn from_dbus_name(name: &str, message: Option<&str>) -> Self {
        let detail = match message {
            Some(m) if !m.is_empty() => format!("{name}: {m}"),
            _ => name.to_string(),
        };
        ServiceError::new(kind_of(name), detail)
    }

    /// From a zbus error.
    pub fn from_zbus(e: &zbus::Error) -> Self {
        match e {
            zbus::Error::MethodError(name, message, _) => {
                ServiceError::from_dbus_name(name.as_str(), message.as_deref())
            }
            zbus::Error::FDO(fdo) => {
                ServiceError::from_dbus_name(fdo.name().as_str(), fdo.description())
            }
            zbus::Error::DBus(err) => {
                ServiceError::from_dbus_name(err.name().as_str(), err.description())
            }
            zbus::Error::InputOutput(io) if io.kind() == std::io::ErrorKind::TimedOut => {
                ServiceError::new(ErrorKind::Timeout, e.to_string())
            }
            // No bus to talk to.
            zbus::Error::InputOutput(_)
            | zbus::Error::Connection(..)
            | zbus::Error::Address(_)
            | zbus::Error::Handshake(_) => ServiceError::new(ErrorKind::Unavailable, e.to_string()),
            _ => ServiceError::new(ErrorKind::Failed, e.to_string()),
        }
    }

    /// What to tell the user.
    pub fn message(&self) -> Message {
        self.kind.message()
    }
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for ServiceError {}

/// The kind of a D-Bus error name.
pub fn kind_of(name: &str) -> ErrorKind {
    match name {
        "net.reactivated.Fprint.Error.NoSuchDevice" => ErrorKind::NoReader,
        "net.reactivated.Fprint.Error.AlreadyInUse" => ErrorKind::Busy,
        "net.reactivated.Fprint.Error.PermissionDenied" => ErrorKind::PermissionDenied,
        "net.reactivated.Fprint.Error.NoEnrolledPrints" => ErrorKind::NoPrints,
        "net.reactivated.Fprint.Error.FingerAlreadyEnrolled" => ErrorKind::AlreadyEnrolled,
        "net.reactivated.Fprint.Error.PrintsNotDeleted"
        | "net.reactivated.Fprint.Error.PrintsNotDeletedFromDevice" => ErrorKind::NotDeleted,
        // ClaimDevice (the device could not be opened), Internal,
        // InvalidFingername, NoActionInProgress: nothing the user can do
        // beyond trying again.
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner"
        | "org.freedesktop.DBus.Error.Spawn.ExecFailed"
        | "org.freedesktop.DBus.Error.Spawn.ChildExited"
        | "org.freedesktop.DBus.Error.Spawn.ChildSignaled"
        | "org.freedesktop.DBus.Error.Spawn.Failed"
        | "org.freedesktop.DBus.Error.Spawn.ServiceNotFound"
        | "org.freedesktop.DBus.Error.Disconnected" => ErrorKind::Unavailable,
        "org.freedesktop.DBus.Error.AccessDenied"
        | "org.freedesktop.DBus.Error.AuthFailed"
        | "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired" => {
            ErrorKind::PermissionDenied
        }
        "org.freedesktop.DBus.Error.UnknownMethod" => ErrorKind::Unsupported,
        "org.freedesktop.DBus.Error.UnknownObject"
        | "org.freedesktop.DBus.Error.UnknownInterface" => ErrorKind::NoReader,
        "org.freedesktop.DBus.Error.NoReply" | "org.freedesktop.DBus.Error.Timeout" => {
            ErrorKind::Timeout
        }
        _ => ErrorKind::Failed,
    }
}

impl ErrorKind {
    /// Every kind, for tests and documentation.
    pub const ALL: [ErrorKind; 10] = [
        ErrorKind::Unavailable,
        ErrorKind::NoReader,
        ErrorKind::Busy,
        ErrorKind::PermissionDenied,
        ErrorKind::NoPrints,
        ErrorKind::AlreadyEnrolled,
        ErrorKind::Unsupported,
        ErrorKind::NotDeleted,
        ErrorKind::Timeout,
        ErrorKind::Failed,
    ];

    /// What to tell the user: one plain sentence.
    pub fn message(self) -> Message {
        match self {
            ErrorKind::Unavailable => msg("The fingerprint service is not available."),
            ErrorKind::NoReader => msg("No fingerprint reader detected."),
            ErrorKind::Busy => {
                msg("The reader is in use by another program. Try again in a moment.")
            }
            ErrorKind::PermissionDenied => msg("Permission denied."),
            ErrorKind::NoPrints => msg("No fingerprint registered."),
            ErrorKind::AlreadyEnrolled => msg("This finger is already registered."),
            ErrorKind::NotDeleted => msg("The fingerprint could not be removed. Try again."),
            ErrorKind::Timeout => msg("The fingerprint service is not answering. Try again."),
            ErrorKind::Unsupported | ErrorKind::Failed => {
                msg("The fingerprint service reported an error. Try again.")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Catalogue;

    #[test]
    fn fprintd_error_names() {
        let cases = [
            (
                "net.reactivated.Fprint.Error.NoSuchDevice",
                ErrorKind::NoReader,
            ),
            ("net.reactivated.Fprint.Error.AlreadyInUse", ErrorKind::Busy),
            (
                "net.reactivated.Fprint.Error.PermissionDenied",
                ErrorKind::PermissionDenied,
            ),
            (
                "net.reactivated.Fprint.Error.NoEnrolledPrints",
                ErrorKind::NoPrints,
            ),
            (
                "net.reactivated.Fprint.Error.FingerAlreadyEnrolled",
                ErrorKind::AlreadyEnrolled,
            ),
            (
                "net.reactivated.Fprint.Error.PrintsNotDeleted",
                ErrorKind::NotDeleted,
            ),
            (
                "net.reactivated.Fprint.Error.PrintsNotDeletedFromDevice",
                ErrorKind::NotDeleted,
            ),
            (
                "net.reactivated.Fprint.Error.ClaimDevice",
                ErrorKind::Failed,
            ),
            ("net.reactivated.Fprint.Error.Internal", ErrorKind::Failed),
            (
                "net.reactivated.Fprint.Error.InvalidFingername",
                ErrorKind::Failed,
            ),
            (
                "net.reactivated.Fprint.Error.NoActionInProgress",
                ErrorKind::Failed,
            ),
            (
                "net.reactivated.Fprint.Error.SomethingNew",
                ErrorKind::Failed,
            ),
        ];
        for (name, kind) in cases {
            assert_eq!(kind_of(name), kind, "{name}");
        }
    }

    #[test]
    fn bus_error_names() {
        let cases = [
            (
                "org.freedesktop.DBus.Error.ServiceUnknown",
                ErrorKind::Unavailable,
            ),
            (
                "org.freedesktop.DBus.Error.NameHasNoOwner",
                ErrorKind::Unavailable,
            ),
            (
                "org.freedesktop.DBus.Error.Spawn.ExecFailed",
                ErrorKind::Unavailable,
            ),
            (
                "org.freedesktop.DBus.Error.Spawn.ChildExited",
                ErrorKind::Unavailable,
            ),
            (
                "org.freedesktop.DBus.Error.Spawn.ChildSignaled",
                ErrorKind::Unavailable,
            ),
            (
                "org.freedesktop.DBus.Error.Spawn.Failed",
                ErrorKind::Unavailable,
            ),
            (
                "org.freedesktop.DBus.Error.Spawn.ServiceNotFound",
                ErrorKind::Unavailable,
            ),
            (
                "org.freedesktop.DBus.Error.Disconnected",
                ErrorKind::Unavailable,
            ),
            (
                "org.freedesktop.DBus.Error.AccessDenied",
                ErrorKind::PermissionDenied,
            ),
            (
                "org.freedesktop.DBus.Error.AuthFailed",
                ErrorKind::PermissionDenied,
            ),
            (
                "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired",
                ErrorKind::PermissionDenied,
            ),
            (
                "org.freedesktop.DBus.Error.UnknownMethod",
                ErrorKind::Unsupported,
            ),
            (
                "org.freedesktop.DBus.Error.UnknownObject",
                ErrorKind::NoReader,
            ),
            (
                "org.freedesktop.DBus.Error.UnknownInterface",
                ErrorKind::NoReader,
            ),
            ("org.freedesktop.DBus.Error.NoReply", ErrorKind::Timeout),
            ("org.freedesktop.DBus.Error.Timeout", ErrorKind::Timeout),
            ("org.freedesktop.DBus.Error.Failed", ErrorKind::Failed),
        ];
        for (name, kind) in cases {
            assert_eq!(kind_of(name), kind, "{name}");
        }
    }

    #[test]
    fn detail_keeps_the_technical_text() {
        let e = ServiceError::from_dbus_name("net.reactivated.Fprint.Error.Internal", Some("boom"));
        assert_eq!(e.kind, ErrorKind::Failed);
        assert_eq!(e.detail, "net.reactivated.Fprint.Error.Internal: boom");
        let e = ServiceError::from_dbus_name("x.Y", Some(""));
        assert_eq!(e.detail, "x.Y");
        let e = ServiceError::from_dbus_name("x.Y", None);
        assert_eq!(e.detail, "x.Y");
        assert!(e.to_string().contains("x.Y"));
    }

    #[test]
    fn zbus_errors() {
        let io = |k| zbus::Error::InputOutput(std::sync::Arc::new(std::io::Error::from(k)));
        assert_eq!(
            ServiceError::from_zbus(&io(std::io::ErrorKind::TimedOut)).kind,
            ErrorKind::Timeout
        );
        assert_eq!(
            ServiceError::from_zbus(&io(std::io::ErrorKind::NotFound)).kind,
            ErrorKind::Unavailable
        );
        assert_eq!(
            ServiceError::from_zbus(&io(std::io::ErrorKind::ConnectionRefused)).kind,
            ErrorKind::Unavailable
        );
        assert_eq!(
            ServiceError::from_zbus(&zbus::Error::Address("bad".into())).kind,
            ErrorKind::Unavailable
        );
        assert_eq!(
            ServiceError::from_zbus(&zbus::Error::Handshake("no".into())).kind,
            ErrorKind::Unavailable
        );
        let e = ServiceError::from_zbus(&zbus::Error::FDO(Box::new(
            zbus::fdo::Error::ServiceUnknown("gone".into()),
        )));
        assert_eq!(e.kind, ErrorKind::Unavailable);
        assert_eq!(e.detail, "org.freedesktop.DBus.Error.ServiceUnknown: gone");
        let e = ServiceError::from_zbus(&zbus::Error::FDO(Box::new(
            zbus::fdo::Error::UnknownMethod("old".into()),
        )));
        assert_eq!(e.kind, ErrorKind::Unsupported);
        let e = ServiceError::from_zbus(&zbus::Error::FDO(Box::new(
            zbus::fdo::Error::AccessDenied("no".into()),
        )));
        assert_eq!(e.kind, ErrorKind::PermissionDenied);
        assert_eq!(
            ServiceError::from_zbus(&zbus::Error::InvalidReply).kind,
            ErrorKind::Failed
        );
        // The bus socket is missing.
        let addr: zbus::Address = "unix:path=/nonexistent".parse().unwrap();
        let e = zbus::Error::Connection(
            std::sync::Arc::new(std::io::Error::from(std::io::ErrorKind::NotFound)),
            Box::new(addr),
        );
        assert_eq!(ServiceError::from_zbus(&e).kind, ErrorKind::Unavailable);
        // A typed D-Bus error.
        let e = zbus::Error::DBus(std::sync::Arc::new(zbus::fdo::Error::NoReply(
            "late".into(),
        )));
        let s = ServiceError::from_zbus(&e);
        assert_eq!(s.kind, ErrorKind::Timeout);
        assert_eq!(s.detail, "org.freedesktop.DBus.Error.NoReply: late");
    }

    #[test]
    fn zbus_method_errors() {
        let call = zbus::message::Message::method_call("/x", "Y")
            .build(&())
            .unwrap();
        let reply = zbus::message::Message::error(
            &call.header(),
            "net.reactivated.Fprint.Error.AlreadyInUse",
        )
        .build(&("Device already in use",))
        .unwrap();
        let e = zbus::Error::from(reply);
        let s = ServiceError::from_zbus(&e);
        assert_eq!(s.kind, ErrorKind::Busy);
        assert_eq!(
            s.detail,
            "net.reactivated.Fprint.Error.AlreadyInUse: Device already in use"
        );
    }

    /// Every error, and exactly what the user reads in French.
    #[test]
    fn every_kind_has_its_french_sentence() {
        let fr = Catalogue::builtin("fr").unwrap();
        let cases = [
            (
                ErrorKind::Unavailable,
                "Le service d'empreintes n'est pas disponible.",
            ),
            (ErrorKind::NoReader, "Aucun lecteur d'empreintes détecté."),
            (
                ErrorKind::Busy,
                "Le lecteur est utilisé par un autre programme. Réessayez dans un instant.",
            ),
            (ErrorKind::PermissionDenied, "Autorisation refusée."),
            (ErrorKind::NoPrints, "Aucune empreinte enregistrée."),
            (ErrorKind::AlreadyEnrolled, "Ce doigt est déjà enregistré."),
            (
                ErrorKind::NotDeleted,
                "L'empreinte n'a pas pu être supprimée. Réessayez.",
            ),
            (
                ErrorKind::Timeout,
                "Le service d'empreintes ne répond pas. Réessayez.",
            ),
            (
                ErrorKind::Unsupported,
                "Le service d'empreintes a signalé une erreur. Réessayez.",
            ),
            (
                ErrorKind::Failed,
                "Le service d'empreintes a signalé une erreur. Réessayez.",
            ),
        ];
        assert_eq!(cases.len(), ErrorKind::ALL.len());
        for (k, want) in cases {
            assert_eq!(k.message().render(&fr), want, "{k:?}");
            assert_eq!(ServiceError::new(k, "detail").message(), k.message());
        }
        // polkit's refusal, by its D-Bus name.
        assert_eq!(
            ServiceError::from_dbus_name("net.reactivated.Fprint.Error.PermissionDenied", None)
                .message()
                .render(&fr),
            "Autorisation refusée."
        );
    }
}
