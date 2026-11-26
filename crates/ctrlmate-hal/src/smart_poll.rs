//! Smart Adaptive Polling & Deadband Delta Filter for Home Assistant.
//!
//! Keeps HA Recorder lean while delivering instantaneous event pushes and
//! responsive telemetry during active / gaming workloads.

use ctrlmate_protocol::{EntityState, EntityValue, PollingProfile};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityTier {
    Gaming,
    Active,
    LockedOrIdle,
}

#[derive(Debug)]
pub struct SmartPoller {
    profile: PollingProfile,
    last_sent: HashMap<String, (EntityValue, Instant)>,
}

impl SmartPoller {
    pub fn new(profile: PollingProfile) -> Self {
        Self {
            profile,
            last_sent: HashMap::new(),
        }
    }

    pub fn profile(&self) -> PollingProfile {
        self.profile
    }

    pub fn set_profile(&mut self, profile: PollingProfile) {
        self.profile = profile;
    }

    /// Forces the next batch to transmit all states (e.g. when a new HA client connects).
    pub fn reset(&mut self) {
        self.last_sent.clear();
    }

    /// Filters a full polled snapshot down to only states that should be pushed to HA right now.
    pub fn filter_batch(
        &mut self,
        states: &[EntityState],
        tier: ActivityTier,
        now: Instant,
    ) -> Vec<EntityState> {
        let (min_interval, heartbeat) = self.cadence(tier);
        let mut out = Vec::new();

        for st in states {
            let should_send = match self.last_sent.get(&st.id) {
                None => true,
                Some((prev_val, last_ts)) => {
                    let elapsed = now.saturating_duration_since(*last_ts);
                    if elapsed >= heartbeat {
                        true
                    } else if is_instant_entity(&st.value) {
                        prev_val != &st.value
                    } else if elapsed < min_interval {
                        false
                    } else {
                        exceeds_deadband(&st.id, prev_val, &st.value)
                    }
                }
            };

            if should_send {
                self.last_sent
                    .insert(st.id.clone(), (st.value.clone(), now));
                out.push(st.clone());
            }
        }

        out
    }

    fn cadence(&self, tier: ActivityTier) -> (Duration, Duration) {
        match self.profile {
            PollingProfile::Intelligent => match tier {
                ActivityTier::Gaming => (Duration::from_secs(2), Duration::from_secs(60)),
                ActivityTier::Active => (Duration::from_secs(5), Duration::from_secs(60)),
                ActivityTier::LockedOrIdle => (Duration::from_secs(30), Duration::from_secs(120)),
            },
            PollingProfile::Eco => (Duration::from_secs(15), Duration::from_secs(120)),
            PollingProfile::Balanced => (Duration::from_secs(5), Duration::from_secs(60)),
            PollingProfile::Realtime => (Duration::from_secs(1), Duration::from_secs(30)),
        }
    }
}

fn is_instant_entity(v: &EntityValue) -> bool {
    matches!(
        v,
        EntityValue::Bool(_)
            | EntityValue::Text(_)
            | EntityValue::Light(_)
            | EntityValue::Update(_)
    )
}

fn exceeds_deadband(id: &str, prev: &EntityValue, next: &EntityValue) -> bool {
    match (prev, next) {
        (EntityValue::Float(a), EntityValue::Float(b)) => {
            let delta = (a - b).abs();
            if id.contains("temperature") {
                delta >= 0.8
            } else if id.contains("power") {
                delta >= 2.0
            } else if id.contains("load") || id.contains("usage") {
                delta >= 2.5
            } else {
                delta >= 0.5
            }
        }
        (EntityValue::Int(a), EntityValue::Int(b)) => {
            let delta = (a - b).abs();
            if id.contains("clock") || id.contains("frequency") {
                delta >= 50
            } else {
                delta >= 1
            }
        }
        _ => prev != next,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::BTreeMap;

    fn state(id: &str, value: EntityValue) -> EntityState {
        EntityState {
            id: id.into(),
            value,
            attributes: BTreeMap::new(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn deadband_suppresses_tiny_noise_and_passes_instant_events() {
        let mut poller = SmartPoller::new(PollingProfile::Intelligent);
        let t0 = Instant::now();

        let initial = vec![
            state("gpu_0_temperature", EntityValue::Float(43.0)),
            state("microphone_in_use", EntityValue::Bool(false)),
        ];
        assert_eq!(
            poller
                .filter_batch(&initial, ActivityTier::Active, t0)
                .len(),
            2
        );

        // 1s later: mic toggles ON (instant!), temp jitters +0.2°C (filtered!)
        let t1 = t0 + Duration::from_secs(1);
        let next = vec![
            state("gpu_0_temperature", EntityValue::Float(43.2)),
            state("microphone_in_use", EntityValue::Bool(true)),
        ];
        let batch = poller.filter_batch(&next, ActivityTier::Active, t1);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].id, "microphone_in_use");

        // 6s later: temp rises by +1.5°C -> passes!
        let t2 = t0 + Duration::from_secs(6);
        let hotter = vec![
            state("gpu_0_temperature", EntityValue::Float(44.5)),
            state("microphone_in_use", EntityValue::Bool(true)),
        ];
        let batch2 = poller.filter_batch(&hotter, ActivityTier::Active, t2);
        assert_eq!(batch2.len(), 1);
        assert_eq!(batch2[0].id, "gpu_0_temperature");
    }
}
