//! Whether the archiver can be reached, for the archiver pill.
//!
//! A `bpl/getVersion` probe runs when an archiver is set (at start-up, or
//! after a change). After that the state is passive: every data fetch and
//! search says how the archiver answered. A success turns it reachable; a
//! failure that is the archiver's (no connection, a gateway error, a wrong
//! URL) turns it unreachable. While unreachable it is probed again every
//! [`REPROBE`] seconds, so it turns reachable again when the archiver comes
//! back. A 404 for one PV (not archived) says nothing about the archiver.

use archiver::Error;

/// While the archiver is unreachable, probe it this often (seconds).
pub const REPROBE: f64 = 60.0;

/// A probe gives up after this long (seconds): a host that drops packets
/// should not leave the pill checking for the fetch timeout.
pub const PROBE_TIMEOUT: u64 = 10;

/// What is known of the archiver.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArchiverState {
    /// None is set: the plot is live only.
    #[default]
    None,
    /// Set, and not yet heard from.
    Checking,
    /// It answered last time.
    Reachable,
    /// It failed last time.
    Unreachable,
}

/// The archiver's state as the pill shows it ([`crate::Plot::archiver_status`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArchiverStatus {
    pub state: ArchiverState,
    /// The appliance's version, from `bpl/getVersion`.
    pub version: Option<String>,
    /// The last archiver error and when it happened (seconds since 1970).
    pub last_error: Option<(String, f64)>,
    /// When the archiver last answered (seconds since 1970).
    pub last_ok: Option<f64>,
}

/// The status, and when to probe.
#[derive(Debug)]
pub struct Monitor {
    pub status: ArchiverStatus,
    /// Counts archiver changes: a probe's answer about an earlier archiver
    /// is dropped.
    pub epoch: u64,
    /// A probe should be sent (an archiver was just set).
    pub due: bool,
    /// A probe is in flight.
    pub probing: bool,
    /// When the last probe was sent (seconds since 1970).
    pub probed_at: f64,
    /// The re-probe interval while unreachable ([`REPROBE`]; shorter in
    /// tests).
    pub reprobe: f64,
}

impl Default for Monitor {
    fn default() -> Self {
        Self {
            status: ArchiverStatus::default(),
            epoch: 0,
            due: false,
            probing: false,
            probed_at: f64::NEG_INFINITY,
            reprobe: REPROBE,
        }
    }
}

impl Monitor {
    /// A new archiver (or none): forget what was known, and probe it.
    pub fn reset(&mut self, archiver: bool) {
        self.epoch += 1;
        self.status = ArchiverStatus {
            state: if archiver {
                ArchiverState::Checking
            } else {
                ArchiverState::None
            },
            ..Default::default()
        };
        self.due = archiver;
        self.probing = false;
    }

    /// Whether to send a probe now: one is due, or the archiver has been
    /// unreachable since the last one for [`Monitor::reprobe`] seconds.
    pub fn should_probe(&self, now: f64) -> bool {
        !self.probing
            && (self.due
                || (self.status.state == ArchiverState::Unreachable
                    && now - self.probed_at >= self.reprobe))
    }

    /// A probe was sent.
    pub fn sent(&mut self, now: f64) {
        self.due = false;
        self.probing = true;
        self.probed_at = now;
    }

    /// The archiver answered. True if it was unreachable until now, so
    /// that what failed can be asked for again.
    pub fn ok(&mut self, now: f64) -> bool {
        if self.status.state == ArchiverState::None {
            return false;
        }
        let back = self.status.state == ArchiverState::Unreachable;
        self.status.state = ArchiverState::Reachable;
        self.status.last_ok = Some(now);
        back
    }

    /// The archiver failed with `e`.
    pub fn failed(&mut self, e: &Error, now: f64) {
        if self.status.state == ArchiverState::None {
            return;
        }
        self.status.state = ArchiverState::Unreachable;
        self.status.last_error = Some((e.to_string(), now));
    }

    /// A probe's answer: the version (`None` when the archiver answers but
    /// not to `getVersion`), or why it failed. Answers about an earlier
    /// archiver are dropped; true when the archiver is back (as
    /// [`Monitor::ok`]).
    pub fn probed(&mut self, epoch: u64, result: Result<Option<String>, Error>, now: f64) -> bool {
        if epoch != self.epoch {
            return false;
        }
        self.probing = false;
        match result {
            Ok(version) => {
                if version.is_some() {
                    self.status.version = version;
                }
                self.ok(now)
            }
            Err(e) => {
                self.failed(&e, now);
                false
            }
        }
    }
}

/// Whether `e`, from a data request or a search, is the archiver's fault
/// rather than the PV's (or the search pattern's): no answer, a wrong URL,
/// or a gateway or availability error. A 404 is the archiver's fault only
/// for a search, which any archiver answers; for data it means "not
/// archived" (a wrong URL is told apart before it gets here, as
/// [`Error::BadUrl`]). Other HTTP errors are about the request (a 400 for
/// an odd name, a 500 for a bad regex).
pub fn archiver_fault(e: &Error, search: bool) -> bool {
    match e {
        Error::Transport(_) | Error::BadUrl(_) => true,
        Error::Http { status, .. } => matches!(status, 502..=504) || (search && *status == 404),
        Error::Parse(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down() -> Error {
        Error::Transport("connection refused".into())
    }

    #[test]
    fn a_new_archiver_is_checked_then_answers_decide() {
        let mut m = Monitor::default();
        assert_eq!(m.status.state, ArchiverState::None);
        assert!(!m.should_probe(0.0));
        // Without an archiver nothing changes it.
        m.ok(1.0);
        m.failed(&down(), 1.0);
        assert_eq!(m.status.state, ArchiverState::None);

        m.reset(true);
        assert_eq!(m.status.state, ArchiverState::Checking);
        assert!(m.should_probe(0.0));
        m.sent(0.0);
        assert!(!m.should_probe(0.0), "one probe at a time");
        assert!(!m.probed(m.epoch, Ok(Some("v1".into())), 1.0));
        assert_eq!(m.status.state, ArchiverState::Reachable);
        assert_eq!(m.status.version.as_deref(), Some("v1"));
        assert!(!m.should_probe(1000.0), "no polling while reachable");

        // Passive: a failure turns it red, and it is probed again slowly.
        m.failed(&down(), 10.0);
        assert_eq!(m.status.state, ArchiverState::Unreachable);
        assert_eq!(
            m.status.last_error,
            Some(("connection refused".to_string(), 10.0))
        );
        assert!(!m.should_probe(30.0), "probed less than a minute ago");
        assert!(m.should_probe(REPROBE), "a minute since the last probe");
        m.sent(10.0);
        assert!(!m.probed(m.epoch, Err(down()), 11.0));
        assert!(!m.should_probe(30.0));
        assert!(m.should_probe(10.0 + REPROBE));
        // Any success brings it back.
        assert!(m.ok(40.0));
        assert_eq!(m.status.state, ArchiverState::Reachable);
        assert_eq!(m.status.version.as_deref(), Some("v1"), "kept");
    }

    #[test]
    fn answers_about_an_earlier_archiver_are_dropped() {
        let mut m = Monitor::default();
        m.reset(true);
        m.sent(0.0);
        let old = m.epoch;
        m.reset(true);
        assert!(!m.probed(old, Err(down()), 1.0));
        assert_eq!(m.status.state, ArchiverState::Checking);
        m.reset(false);
        assert_eq!(m.status.state, ArchiverState::None);
        assert!(!m.should_probe(0.0));
    }

    #[test]
    fn only_the_archivers_faults_count() {
        let http = |status| Error::Http {
            status,
            message: String::new(),
        };
        assert!(archiver_fault(&down(), false));
        assert!(archiver_fault(&Error::BadUrl("x".into()), false));
        assert!(archiver_fault(&http(503), false));
        assert!(!archiver_fault(&http(404), false), "not archived");
        assert!(archiver_fault(&http(404), true), "no search: a wrong URL");
        assert!(!archiver_fault(&http(400), false));
        assert!(!archiver_fault(&http(500), true), "a bad regex");
        assert!(!archiver_fault(&Error::Parse("x".into()), false));
    }
}
