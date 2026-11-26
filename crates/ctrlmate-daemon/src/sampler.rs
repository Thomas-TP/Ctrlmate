//! Adaptive hardware sampling: how often `poll()` runs depends on what the user is doing and on
//! who is looking, never slower than [`MAX_SAMPLE_INTERVAL`] because the cooling watchdog needs
//! fresh temperatures (stale data is treated as unsafe).
//!
//! | situation | gaming | active | idle (>= 5 min) | locked |
//! |---|---|---|---|---|
//! | desktop UI connected | 1 s | 1 s | 2 s | 5 s |
//! | only Home Assistant / MQTT, `realtime` | 1 s | 1 s | 2 s | 5 s |
//! | only Home Assistant / MQTT, `intelligent` | 2 s | 5 s | 5 s | 5 s |
//! | only Home Assistant / MQTT, `balanced` | 3 s | 5 s | 5 s | 5 s |
//! | only Home Assistant / MQTT, `eco` | 5 s | 5 s | 5 s | 5 s |
//! | nobody listening | 2 s | 5 s | 5 s | 5 s |

use ctrlmate_hal::smart_poll::ActivityTier;
use ctrlmate_protocol::{IDLE_THRESHOLD_SECS, PollingProfile, SessionState};
use std::time::Duration;

/// Upper bound of the sampling interval (cooling safety).
pub const MAX_SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

/// What the user is doing, derived from the agent's `SessionState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Gaming,
    Active,
    Idle,
    Locked,
}

/// Who currently consumes telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listeners {
    pub ui: bool,
    pub remote: bool,
}

impl Tier {
    /// Without any session information the user is assumed to be active.
    pub fn from_session(session: Option<&SessionState>) -> Self {
        let Some(s) = session else {
            return Self::Active;
        };
        if s.locked {
            Self::Locked
        } else if s.gaming_mode {
            Self::Gaming
        } else if s.idle_seconds >= IDLE_THRESHOLD_SECS {
            Self::Idle
        } else {
            Self::Active
        }
    }

    /// Tier handed to the publish-side `SmartPoller` (deadband / heartbeat).
    pub fn publish_tier(self) -> ActivityTier {
        match self {
            Self::Gaming => ActivityTier::Gaming,
            Self::Active => ActivityTier::Active,
            Self::Idle | Self::Locked => ActivityTier::LockedOrIdle,
        }
    }
}

const fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

/// Sampling interval for the given situation.
pub fn sample_interval(profile: PollingProfile, tier: Tier, l: Listeners) -> Duration {
    let d = if l.ui {
        match tier {
            Tier::Gaming | Tier::Active => secs(1),
            Tier::Idle => secs(2),
            Tier::Locked => secs(5),
        }
    } else if l.remote {
        match (profile, tier) {
            (PollingProfile::Realtime, Tier::Gaming | Tier::Active) => secs(1),
            (PollingProfile::Realtime, Tier::Idle) => secs(2),
            (PollingProfile::Intelligent, Tier::Gaming) => secs(2),
            (PollingProfile::Balanced, Tier::Gaming) => secs(3),
            _ => secs(5),
        }
    } else if tier == Tier::Gaming {
        secs(2)
    } else {
        secs(5)
    };
    d.min(MAX_SAMPLE_INTERVAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UI: Listeners = Listeners {
        ui: true,
        remote: false,
    };
    const HA: Listeners = Listeners {
        ui: false,
        remote: true,
    };
    const NONE: Listeners = Listeners {
        ui: false,
        remote: false,
    };

    fn session(locked: bool, gaming: bool, idle: u64) -> SessionState {
        SessionState {
            locked,
            gaming_mode: gaming,
            idle_seconds: idle,
            ..SessionState::default()
        }
    }

    #[test]
    fn tier_from_session() {
        assert_eq!(Tier::from_session(None), Tier::Active);
        assert_eq!(
            Tier::from_session(Some(&session(false, false, 0))),
            Tier::Active
        );
        assert_eq!(
            Tier::from_session(Some(&session(false, true, 0))),
            Tier::Gaming
        );
        assert_eq!(
            Tier::from_session(Some(&session(false, true, 9999))),
            Tier::Gaming
        );
        assert_eq!(
            Tier::from_session(Some(&session(false, false, 299))),
            Tier::Active
        );
        assert_eq!(
            Tier::from_session(Some(&session(false, false, 300))),
            Tier::Idle
        );
        // Locked beats everything, including a game left running.
        assert_eq!(
            Tier::from_session(Some(&session(true, true, 0))),
            Tier::Locked
        );
    }

    #[test]
    fn interval_is_actually_adaptive() {
        use PollingProfile::*;
        use Tier::*;
        assert_eq!(sample_interval(Intelligent, Active, UI), secs(1));
        assert_eq!(sample_interval(Intelligent, Idle, UI), secs(2));
        assert_eq!(sample_interval(Intelligent, Locked, UI), secs(5));
        assert_eq!(sample_interval(Intelligent, Gaming, HA), secs(2));
        assert_eq!(sample_interval(Intelligent, Active, HA), secs(5));
        assert_eq!(sample_interval(Realtime, Active, HA), secs(1));
        assert_eq!(sample_interval(Balanced, Gaming, HA), secs(3));
        assert_eq!(sample_interval(Eco, Gaming, HA), secs(5));
        // Fewer ticks when nobody listens.
        assert_eq!(sample_interval(Realtime, Active, NONE), secs(5));
        assert_eq!(sample_interval(Realtime, Gaming, NONE), secs(2));
    }

    #[test]
    fn never_slower_than_the_cooling_cap() {
        for p in [
            PollingProfile::Intelligent,
            PollingProfile::Eco,
            PollingProfile::Balanced,
            PollingProfile::Realtime,
        ] {
            for t in [Tier::Gaming, Tier::Active, Tier::Idle, Tier::Locked] {
                for l in [UI, HA, NONE] {
                    let d = sample_interval(p, t, l);
                    assert!(d <= MAX_SAMPLE_INTERVAL && d >= secs(1), "{p:?} {t:?}");
                }
            }
        }
    }

    #[test]
    fn publish_tier_mapping() {
        assert_eq!(Tier::Idle.publish_tier(), ActivityTier::LockedOrIdle);
        assert_eq!(Tier::Locked.publish_tier(), ActivityTier::LockedOrIdle);
        assert_eq!(Tier::Gaming.publish_tier(), ActivityTier::Gaming);
    }
}
