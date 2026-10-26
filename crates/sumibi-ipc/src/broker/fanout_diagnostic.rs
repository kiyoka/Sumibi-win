//! Disposable two-client Broker transport diagnostic.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Normal,
    FailedWrite,
    Lagging,
    ReadyFailure,
    BadHello,
}

impl Scenario {
    fn label(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::FailedWrite => "failed-notice",
            Self::Lagging => "lagging-notice",
            Self::ReadyFailure => "failed-ready",
            Self::BadHello => "bad-hello",
        }
    }

    fn mode(self) -> Mode {
        match self {
            Self::Normal => Mode::DiagnosticFixed { delay_ms: 0 },
            Self::FailedWrite => Mode::DiagnosticFanoutFailedNotice,
            Self::Lagging => Mode::DiagnosticFanoutLaggingNotice,
            Self::ReadyFailure => Mode::DiagnosticFanoutReadyFailure,
            Self::BadHello => Mode::DiagnosticFanoutBadHello,
        }
    }
}

mod probe;
mod serve;

pub use probe::{probe, probe_denied};
pub(super) use serve::serve_fanout;
