// SPDX-License-Identifier: Apache-2.0
//
// Is the reader's own host clock synchronized? The reader runs beside the
// router it reads, on the same host, so this is the router's clock too. A
// rejection line compares the peer's clock with this one; if this one is not
// grounded, a peer cannot be blamed.
//
// The default check asks the kernel (adjtimex with no modes: a read, no
// privilege needed). chrony, ntpd and systemd-timesyncd clear STA_UNSYNC when
// they hold the clock. An operator can override the check with
// PEER_HEALTH_LOCAL_CLOCK=synced|unsynced, for a host whose sync state the
// kernel does not reflect or a container whose seccomp profile refuses the call.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum LocalClock {
    Synced,
    Unsynced(String),
    /// The check could not run. Treated as not grounded: no peer is blamed.
    Unknown(String),
}

impl LocalClock {
    pub fn is_synced(&self) -> bool {
        matches!(self, LocalClock::Synced)
    }

    pub fn label(&self) -> &'static str {
        match self {
            LocalClock::Synced => "synced",
            LocalClock::Unsynced(_) => "unsynced",
            LocalClock::Unknown(_) => "unknown",
        }
    }
}

impl fmt::Display for LocalClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LocalClock::Synced => write!(f, "synced"),
            LocalClock::Unsynced(why) => write!(f, "unsynced: {why}"),
            LocalClock::Unknown(why) => write!(f, "unknown: {why}"),
        }
    }
}

/// The injectable check.
pub trait LocalClockCheck: Send + Sync {
    fn check(&self) -> LocalClock;
}

/// A fixed answer: the operator override, and tests.
pub struct Fixed(pub LocalClock);

impl LocalClockCheck for Fixed {
    fn check(&self) -> LocalClock {
        self.0.clone()
    }
}

/// The kernel's view, via adjtimex(2).
pub struct Kernel;

impl LocalClockCheck for Kernel {
    #[cfg(target_os = "linux")]
    fn check(&self) -> LocalClock {
        // SAFETY: timex is plain data; modes = 0 makes adjtimex a pure read.
        let mut tx: libc::timex = unsafe { std::mem::zeroed() };
        let state = unsafe { libc::adjtimex(&mut tx) };
        if state == -1 {
            return LocalClock::Unknown(format!(
                "adjtimex failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        if state == libc::TIME_ERROR || tx.status & libc::STA_UNSYNC != 0 {
            return LocalClock::Unsynced(format!(
                "the kernel reports the clock unsynchronized (maxerror {} us)",
                tx.maxerror
            ));
        }
        LocalClock::Synced
    }

    #[cfg(not(target_os = "linux"))]
    fn check(&self) -> LocalClock {
        LocalClock::Unknown("no clock check on this platform".to_string())
    }
}

/// Build the check from the `--local-clock` mode: `auto` (the kernel),
/// `synced` or `unsynced` (operator override).
pub fn from_mode(mode: &str) -> Result<Box<dyn LocalClockCheck>, String> {
    match mode {
        "auto" => Ok(Box::new(Kernel)),
        "synced" => Ok(Box::new(Fixed(LocalClock::Synced))),
        "unsynced" => Ok(Box::new(Fixed(LocalClock::Unsynced(
            "declared unsynchronized by PEER_HEALTH_LOCAL_CLOCK".to_string(),
        )))),
        other => Err(format!(
            "--local-clock must be auto, synced or unsynced, not {other:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert!(from_mode("synced").unwrap().check().is_synced());
        assert_eq!(from_mode("unsynced").unwrap().check().label(), "unsynced");
        assert!(from_mode("sometimes").is_err());
        // The kernel check answers something without panicking.
        let _ = from_mode("auto").unwrap().check();
    }
}
