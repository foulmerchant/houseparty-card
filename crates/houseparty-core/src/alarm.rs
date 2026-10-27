//! Alarm states, arm modes, and which actions are allowed when.

/// State of an `alarm_control_panel` entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AlarmState {
    Disarmed,
    ArmedHome,
    ArmedAway,
    ArmedNight,
    ArmedVacation,
    ArmedCustomBypass,
    /// Exit delay.
    Arming,
    /// Entry delay, before triggering.
    Pending,
    Triggered,
    Disarming,
    Unavailable,
    Unknown,
}

impl AlarmState {
    /// Parse HA's state string. Anything unrecognised becomes `Unknown`,
    /// never a "safe" state.
    pub fn from_ha(s: &str) -> Self {
        match s {
            "disarmed" => Self::Disarmed,
            "armed_home" => Self::ArmedHome,
            "armed_away" => Self::ArmedAway,
            "armed_night" => Self::ArmedNight,
            "armed_vacation" => Self::ArmedVacation,
            "armed_custom_bypass" => Self::ArmedCustomBypass,
            "arming" => Self::Arming,
            "pending" => Self::Pending,
            "triggered" => Self::Triggered,
            "disarming" => Self::Disarming,
            "unavailable" => Self::Unavailable,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Disarmed => "Disarmed",
            Self::ArmedHome => "Armed home",
            Self::ArmedAway => "Armed away",
            Self::ArmedNight => "Armed night",
            Self::ArmedVacation => "Armed vacation",
            Self::ArmedCustomBypass => "Armed (bypass)",
            Self::Arming => "Arming",
            Self::Pending => "Entry delay",
            Self::Triggered => "Triggered",
            Self::Disarming => "Disarming",
            Self::Unavailable => "Unavailable",
            Self::Unknown => "Unknown",
        }
    }

    pub fn tone(self) -> Tone {
        match self {
            Self::Disarmed => Tone::Safe,
            Self::ArmedHome
            | Self::ArmedAway
            | Self::ArmedNight
            | Self::ArmedVacation
            | Self::ArmedCustomBypass => Tone::Armed,
            Self::Arming | Self::Pending | Self::Disarming => Tone::Transition,
            Self::Triggered => Tone::Alert,
            Self::Unavailable | Self::Unknown => Tone::Unknown,
        }
    }
}

/// Visual tone; the UI maps this to theme colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Safe,
    Armed,
    Transition,
    Alert,
    Unknown,
}

impl Tone {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Armed => "armed",
            Self::Transition => "transition",
            Self::Alert => "alert",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArmMode {
    Away,
    Home,
    Night,
    Vacation,
    CustomBypass,
}

impl ArmMode {
    pub const ALL: [ArmMode; 5] = [
        Self::Away,
        Self::Home,
        Self::Night,
        Self::Vacation,
        Self::CustomBypass,
    ];

    /// Bit in HA's `supported_features` (AlarmControlPanelEntityFeature).
    pub fn feature_bit(self) -> u32 {
        match self {
            Self::Home => 1,
            Self::Away => 2,
            Self::Night => 4,
            Self::CustomBypass => 16,
            Self::Vacation => 32,
        }
    }

    pub fn service(self) -> &'static str {
        match self {
            Self::Away => "alarm_arm_away",
            Self::Home => "alarm_arm_home",
            Self::Night => "alarm_arm_night",
            Self::Vacation => "alarm_arm_vacation",
            Self::CustomBypass => "alarm_arm_custom_bypass",
        }
    }

    /// The state this mode ends in once any exit delay is over.
    pub fn armed_state(self) -> AlarmState {
        match self {
            Self::Away => AlarmState::ArmedAway,
            Self::Home => AlarmState::ArmedHome,
            Self::Night => AlarmState::ArmedNight,
            Self::Vacation => AlarmState::ArmedVacation,
            Self::CustomBypass => AlarmState::ArmedCustomBypass,
        }
    }

    /// The mode an armed state belongs to, if any.
    pub fn from_armed_state(state: AlarmState) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.armed_state() == state)
    }

    /// Mode name for the `houseparty.arm` service (used to force-arm).
    pub fn service_mode(self) -> &'static str {
        match self {
            Self::Away => "away",
            Self::Home => "home",
            Self::Night => "night",
            Self::Vacation => "vacation",
            Self::CustomBypass => "custom",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Away => "away",
            Self::Home => "home",
            Self::Night => "night",
            Self::Vacation => "vacation",
            Self::CustomBypass => "with bypass",
        }
    }
}

pub fn supported_modes(features: u32) -> Vec<ArmMode> {
    ArmMode::ALL
        .into_iter()
        .filter(|m| features & m.feature_bit() != 0)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Arm(ArmMode),
    Disarm,
}

impl Action {
    pub fn service(self) -> &'static str {
        match self {
            Self::Arm(m) => m.service(),
            Self::Disarm => "alarm_disarm",
        }
    }

    /// Button text, e.g. "Arm away".
    pub fn label(self) -> String {
        match self {
            Self::Arm(m) => format!("Arm {}", m.name()),
            Self::Disarm => "Disarm".to_string(),
        }
    }

    /// Lowercase verb phrase, e.g. "arm away".
    pub fn verb(self) -> String {
        self.label().to_lowercase()
    }

    /// Only these states count as HA having carried out this action.
    /// Any other change is someone or something else, not a confirmation.
    pub fn is_confirmed_by(self, state: AlarmState) -> bool {
        match self {
            Self::Disarm => state == AlarmState::Disarmed,
            Self::Arm(m) => state == AlarmState::Arming || state == m.armed_state(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeFormat {
    None,
    Number,
    Text,
}

impl CodeFormat {
    /// Any non-null format we don't recognise is treated as text, so a code
    /// is still required (fail closed).
    pub fn from_ha(v: Option<&str>) -> Self {
        match v {
            None => Self::None,
            Some("number") => Self::Number,
            Some(_) => Self::Text,
        }
    }
}

/// Whether to ask for a code before sending `action`.
///
/// HA defaults `code_arm_required` to true, so a missing attribute means
/// "required". Disarming always needs the code when the panel has one.
pub fn needs_code(action: Action, format: CodeFormat, code_arm_required: Option<bool>) -> bool {
    match (format, action) {
        (CodeFormat::None, _) => false,
        (_, Action::Disarm) => true,
        (_, Action::Arm(_)) => code_arm_required.unwrap_or(true),
    }
}

/// Actions offered for a state. Unknown or unavailable states offer nothing.
pub fn available_actions(state: AlarmState, features: u32) -> Vec<Action> {
    match state {
        AlarmState::Disarmed => supported_modes(features)
            .into_iter()
            .map(Action::Arm)
            .collect(),
        AlarmState::Unavailable | AlarmState::Unknown | AlarmState::Disarming => Vec::new(),
        _ => vec![Action::Disarm],
    }
}

/// How sure we are that we're seeing live data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// Connected, and the last heartbeat was answered.
    Live,
    /// Connected as far as the browser knows, but unproven: a heartbeat went
    /// unanswered, or the screen just woke. Could be a half-open socket.
    Stale,
    /// Known to be disconnected.
    Lost,
}

impl Link {
    pub fn from_link_status(s: &str) -> Self {
        match s {
            "live" => Self::Live,
            "lost" => Self::Lost,
            _ => Self::Stale, // anything unexpected is unproven
        }
    }
}

/// What the card may show. Only a proven-live link shows the alarm state;
/// a stale "Disarmed" or "Armed" is a lie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayState {
    ConnectionLost,
    Checking,
    Missing,
    Live(AlarmState),
}

pub fn display_state(link: Link, state: Option<AlarmState>) -> DisplayState {
    match (link, state) {
        (Link::Lost, _) => DisplayState::ConnectionLost,
        (Link::Stale, _) => DisplayState::Checking,
        (Link::Live, None) => DisplayState::Missing,
        (Link::Live, Some(s)) => DisplayState::Live(s),
    }
}

/// How long to wait for HA to confirm a command before telling the user.
pub const CONFIRM_TIMEOUT_MS: f64 = 15_000.0;

/// A command that was sent but not yet confirmed. The UI shows "waiting",
/// never the new state, until HA reports the *expected* state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PendingAction {
    pub action: Action,
    /// Sent as a force-arm (bypassing approved sensors).
    pub forced: bool,
    pub from: AlarmState,
    pub started_ms: f64,
    /// The alarm entity's `last_updated` (server clock) when sent. A change
    /// in it, not the client clock, tells us the integration has reacted.
    pub baseline_updated_ms: f64,
}

/// What the card currently sees on the alarm entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observation {
    pub state: AlarmState,
    pub updated_ms: f64,
    /// HouseParty reports sensors blocking arming (`open_sensors`).
    pub blocked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Waiting,
    Confirmed,
    /// The state changed, but not to what this action produces.
    Diverged(AlarmState),
    /// HouseParty declined to arm because of open sensors. Also covers a
    /// force-arm refused because something beyond the approved list opened.
    Refused,
    TimedOut,
}

impl PendingAction {
    pub fn resolve(&self, now: Observation, now_ms: f64) -> Resolution {
        if self.action.is_confirmed_by(now.state) {
            Resolution::Confirmed
        } else if now.state != self.from {
            Resolution::Diverged(now.state)
        } else if matches!(self.action, Action::Arm(_))
            && now.blocked
            && now.updated_ms != self.baseline_updated_ms
        {
            Resolution::Refused
        } else if now_ms - self.started_ms >= CONFIRM_TIMEOUT_MS {
            Resolution::TimedOut
        } else {
            Resolution::Waiting
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrecognised_states_are_never_safe() {
        for s in ["", "DISARMED", "disarmed ", "armed", "off", "null"] {
            assert_eq!(AlarmState::from_ha(s), AlarmState::Unknown, "{s:?}");
        }
    }

    #[test]
    fn only_a_live_link_shows_state() {
        let armed = Some(AlarmState::ArmedAway);
        assert_eq!(
            display_state(Link::Lost, armed),
            DisplayState::ConnectionLost
        );
        assert_eq!(display_state(Link::Stale, armed), DisplayState::Checking);
        assert_eq!(display_state(Link::Live, None), DisplayState::Missing);
        assert_eq!(
            display_state(Link::Live, armed),
            DisplayState::Live(AlarmState::ArmedAway)
        );
    }

    #[test]
    fn unknown_link_strings_are_unproven() {
        assert_eq!(Link::from_link_status("LIVE"), Link::Stale);
        assert_eq!(Link::from_link_status(""), Link::Stale);
        assert_eq!(Link::from_link_status("live"), Link::Live);
    }

    #[test]
    fn disarm_always_needs_code_when_panel_has_one() {
        assert!(needs_code(Action::Disarm, CodeFormat::Number, Some(false)));
        assert!(needs_code(Action::Disarm, CodeFormat::Text, None));
        assert!(!needs_code(Action::Disarm, CodeFormat::None, Some(true)));
    }

    #[test]
    fn arming_defaults_to_requiring_code() {
        let arm = Action::Arm(ArmMode::Away);
        assert!(needs_code(arm, CodeFormat::Number, None));
        assert!(!needs_code(arm, CodeFormat::Number, Some(false)));
    }

    #[test]
    fn unknown_code_format_still_requires_code() {
        assert_eq!(CodeFormat::from_ha(Some("biometric")), CodeFormat::Text);
    }

    #[test]
    fn actions_by_state() {
        let feats = 1 | 2; // home + away
        assert_eq!(
            available_actions(AlarmState::Disarmed, feats),
            vec![Action::Arm(ArmMode::Away), Action::Arm(ArmMode::Home)]
        );
        assert_eq!(
            available_actions(AlarmState::Triggered, feats),
            vec![Action::Disarm]
        );
        assert_eq!(
            available_actions(AlarmState::Pending, feats),
            vec![Action::Disarm]
        );
        assert!(available_actions(AlarmState::Unknown, feats).is_empty());
        assert!(available_actions(AlarmState::Unavailable, feats).is_empty());
        assert!(available_actions(AlarmState::Disarmed, 0).is_empty());
    }

    fn pending(action: Action, from: AlarmState) -> PendingAction {
        PendingAction {
            action,
            forced: false,
            from,
            started_ms: 1_000.0,
            baseline_updated_ms: 500.0,
        }
    }

    fn obs(state: AlarmState) -> Observation {
        Observation {
            state,
            updated_ms: 500.0,
            blocked: false,
        }
    }

    #[test]
    fn arm_confirmation_needs_the_right_state() {
        let p = pending(Action::Arm(ArmMode::Away), AlarmState::Disarmed);
        assert_eq!(
            p.resolve(obs(AlarmState::Disarmed), 2_000.0),
            Resolution::Waiting
        );
        assert_eq!(
            p.resolve(obs(AlarmState::Arming), 2_000.0),
            Resolution::Confirmed
        );
        assert_eq!(
            p.resolve(obs(AlarmState::ArmedAway), 2_000.0),
            Resolution::Confirmed
        );
        assert_eq!(
            p.resolve(obs(AlarmState::ArmedHome), 2_000.0),
            Resolution::Diverged(AlarmState::ArmedHome)
        );
        assert_eq!(
            p.resolve(obs(AlarmState::Disarmed), 1_000.0 + CONFIRM_TIMEOUT_MS),
            Resolution::TimedOut
        );
    }

    #[test]
    fn failed_disarm_during_entry_delay_is_not_confirmed() {
        let p = pending(Action::Disarm, AlarmState::Pending);
        assert_eq!(
            p.resolve(obs(AlarmState::Triggered), 1_500.0),
            Resolution::Diverged(AlarmState::Triggered)
        );
        assert_eq!(
            p.resolve(obs(AlarmState::Disarmed), 1_500.0),
            Resolution::Confirmed
        );
    }

    #[test]
    fn integration_refusal_needs_a_fresh_update() {
        let p = pending(Action::Arm(ArmMode::Away), AlarmState::Disarmed);
        let stale = Observation {
            state: AlarmState::Disarmed,
            updated_ms: 500.0,
            blocked: true,
        };
        let fresh = Observation {
            updated_ms: 900.0,
            ..stale
        };
        // Blocked from an earlier attempt, nothing new yet: keep waiting.
        assert_eq!(p.resolve(stale, 1_500.0), Resolution::Waiting);
        assert_eq!(p.resolve(fresh, 1_500.0), Resolution::Refused);
        // A force-arm is refused too if something beyond the approved list opened.
        let forced = PendingAction { forced: true, ..p };
        assert_eq!(forced.resolve(fresh, 1_500.0), Resolution::Refused);
    }

    #[test]
    fn mode_helpers() {
        assert_eq!(
            ArmMode::from_armed_state(AlarmState::ArmedNight),
            Some(ArmMode::Night)
        );
        assert_eq!(ArmMode::from_armed_state(AlarmState::Disarmed), None);
        assert_eq!(ArmMode::CustomBypass.service_mode(), "custom");
    }
}
