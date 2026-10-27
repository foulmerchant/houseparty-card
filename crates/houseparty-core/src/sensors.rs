//! Sensors HouseParty reports as blocking arming. The server decides; the
//! card only shows what it was told and echoes back what was approved.

/// One sensor the server says is blocking arming (or tripped the alarm).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocker {
    pub entity_id: String,
    pub name: String,
    pub state: String,
}

impl Blocker {
    pub fn is_open(&self) -> bool {
        self.state == "on"
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreArmReport {
    pub blockers: Vec<Blocker>,
}

impl PreArmReport {
    pub fn ready(&self) -> bool {
        self.blockers.is_empty()
    }

    pub fn open_names(&self) -> Vec<String> {
        self.blockers
            .iter()
            .filter(|b| b.is_open())
            .map(|b| b.name.clone())
            .collect()
    }

    /// Anything not "on" is listed as not reporting (unavailable, unknown).
    pub fn not_reporting_names(&self) -> Vec<String> {
        self.blockers
            .iter()
            .filter(|b| !b.is_open())
            .map(|b| b.name.clone())
            .collect()
    }

    /// The exact entity ids the person approves bypassing. The server
    /// refuses a force-arm if anything outside this set is blocking.
    pub fn ids(&self) -> Vec<String> {
        self.blockers.iter().map(|b| b.entity_id.clone()).collect()
    }

    /// True if this report blocks on any sensor `earlier` didn't.
    pub fn has_new_blockers(&self, earlier: &PreArmReport) -> bool {
        self.blockers
            .iter()
            .any(|b| !earlier.blockers.iter().any(|e| e.entity_id == b.entity_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(id: &str, state: &str) -> Blocker {
        Blocker {
            entity_id: format!("binary_sensor.{id}"),
            name: id.into(),
            state: state.into(),
        }
    }

    #[test]
    fn groups_and_ids() {
        let r = PreArmReport {
            blockers: vec![b("door", "on"), b("patio", "unavailable")],
        };
        assert_eq!(r.open_names(), vec!["door"]);
        assert_eq!(r.not_reporting_names(), vec!["patio"]);
        assert_eq!(r.ids(), vec!["binary_sensor.door", "binary_sensor.patio"]);
        assert!(!r.ready());
    }

    #[test]
    fn new_blockers_by_id() {
        let before = PreArmReport {
            blockers: vec![b("door", "on")],
        };
        let moved = PreArmReport {
            blockers: vec![b("door", "unavailable")],
        };
        let more = PreArmReport {
            blockers: vec![b("door", "on"), b("win", "on")],
        };
        assert!(!moved.has_new_blockers(&before));
        assert!(more.has_new_blockers(&before));
        assert!(!PreArmReport::default().has_new_blockers(&before));
    }
}
