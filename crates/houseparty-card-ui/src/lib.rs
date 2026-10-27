//! WASM bridge for the HouseParty card. The JS shim drives `AlarmCard`;
//! components live in `components.rs`; decisions come from `houseparty-core`.
//!
//! The HouseParty integration is the authority: it checks codes, enforces
//! the lockout, decides readiness, and bypasses only what was approved. The
//! card shows what the server reports and never claims more than it knows.
#![forbid(unsafe_code)]

mod components;

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use houseparty_core::alarm::{
    display_state, Action, AlarmState, ArmMode, CodeFormat, DisplayState, Link, Observation,
    PendingAction, Resolution,
};
use houseparty_core::keypad;
use houseparty_core::pin::{wipe_string, PinBuffer};
use houseparty_core::policy;
use houseparty_core::sensors::{Blocker, PreArmReport};
use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

// ---------------------------------------------------------------------------
// Data crossing the JS boundary (sanitized by the shim).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct CardConfig {
    pub entity: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Hide who/when and sensor details until "Show details". On unless false.
    #[serde(default)]
    pub privacy: Option<bool>,
    /// Show seconds left during the *entry* delay. Off by default.
    #[serde(default)]
    pub show_entry_countdown: Option<bool>,
    /// Looping media shown while idle. Local files only (validated by shim).
    #[serde(default)]
    pub idle: Option<IdleConfig>,
    /// "full", "subtle" or "off" (the shim resolves "auto" per device).
    #[serde(default)]
    pub motion: Option<String>,
    /// Short vibrations on press, success and error, where supported.
    #[serde(default)]
    pub haptics: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct IdleConfig {
    pub media: Vec<String>,
    #[serde(default)]
    pub after: Option<u32>,
    #[serde(default)]
    pub rotate: Option<u32>,
}

impl CardConfig {
    pub(crate) fn privacy(&self) -> bool {
        self.privacy != Some(false)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct SensorRef {
    pub entity_id: String,
    pub name: String,
    #[serde(default)]
    pub state: String,
}

/// HouseParty's own attributes. `None` means the entity isn't HouseParty.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct HouseParty {
    #[serde(default)]
    pub open_sensors: Vec<SensorRef>,
    #[serde(default)]
    pub bypassed: Vec<SensorRef>,
    #[serde(default)]
    pub triggered_by: Option<SensorRef>,
}

#[derive(Deserialize)]
struct AlarmSnap {
    entity_id: String,
    state: String,
    #[serde(default)]
    last_changed_ms: f64,
    #[serde(default)]
    last_updated_ms: f64,
    #[serde(default)]
    friendly_name: Option<String>,
    #[serde(default)]
    code_format: Option<String>,
    #[serde(default)]
    code_arm_required: Option<bool>,
    #[serde(default)]
    supported_features: Option<u32>,
    #[serde(default)]
    changed_by: Option<String>,
    #[serde(default)]
    arm_mode: Option<String>,
    #[serde(default)]
    delay: Option<u32>,
    #[serde(default)]
    houseparty: Option<HouseParty>,
}

#[derive(Deserialize)]
struct Snapshot {
    alarm: Option<AlarmSnap>,
    now_ms: f64,
}

// ---------------------------------------------------------------------------
// Reactive state for one card instance.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AlarmView {
    pub state: AlarmState,
    pub name: String,
    pub code_format: CodeFormat,
    pub code_arm_required: Option<bool>,
    pub features: u32,
    pub changed_by: Option<String>,
    pub last_changed_ms: f64,
    pub updated_ms: f64,
    pub arm_mode: Option<ArmMode>,
    pub delay: Option<u32>,
    pub hp: Option<HouseParty>,
}

impl AlarmView {
    pub(crate) fn report(&self) -> PreArmReport {
        PreArmReport {
            blockers: self
                .hp
                .as_ref()
                .map(|h| {
                    h.open_sensors
                        .iter()
                        .map(|s| Blocker {
                            entity_id: s.entity_id.clone(),
                            name: s.name.clone(),
                            state: s.state.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Flow {
    Idle,
    /// HouseParty refused to arm; showing its blockers.
    Confirming(Action, PreArmReport),
    /// Entering a code. `Some(ids)` = force-arm bypassing exactly these.
    EnteringCode(Action, Option<Vec<String>>),
}

#[derive(Clone, Copy)]
pub(crate) struct Store {
    pub instance: u32,
    pub config: RwSignal<CardConfig>,
    pub loaded: RwSignal<bool>,
    pub alarm: RwSignal<Option<AlarmView>>,
    pub link: RwSignal<Link>,
    pub data_error: RwSignal<bool>,
    pub now_ms: RwSignal<f64>,
    pub pending: RwSignal<Option<PendingAction>>,
    pub error: RwSignal<Option<String>>,
    pub error_since_ms: RwSignal<f64>,
    pub flow: RwSignal<Flow>,
    /// Only the *length* is reactive; the digits stay in the registry.
    pub pin_len: RwSignal<usize>,
    pub keypad: RwSignal<[u8; 10]>,
    pub last_input_ms: RwSignal<f64>,
    pub revealed_until_ms: RwSignal<f64>,
    /// Last touch anywhere on the card, or anything worth waking up for.
    pub last_activity_ms: RwSignal<f64>,
    /// When the screensaver was last dismissed (for the wake guard).
    pub woke_at_ms: RwSignal<f64>,
    pub idle_index: RwSignal<usize>,
    pub idle_rotated_ms: RwSignal<f64>,
    /// Bumped each time HouseParty reports a *new* state; the success bloom
    /// is keyed on it, so it can only ever celebrate a confirmed change.
    pub bloom_seq: RwSignal<u32>,
}

impl Store {
    fn new(instance: u32) -> Self {
        Self {
            instance,
            config: RwSignal::new(CardConfig::default()),
            loaded: RwSignal::new(false),
            alarm: RwSignal::new(None),
            link: RwSignal::new(Link::Stale), // unproven until the shim says otherwise
            data_error: RwSignal::new(false),
            now_ms: RwSignal::new(js_sys::Date::now()),
            pending: RwSignal::new(None),
            error: RwSignal::new(None),
            error_since_ms: RwSignal::new(0.0),
            flow: RwSignal::new(Flow::Idle),
            pin_len: RwSignal::new(0),
            keypad: RwSignal::new(keypad::STANDARD),
            last_input_ms: RwSignal::new(0.0),
            revealed_until_ms: RwSignal::new(0.0),
            last_activity_ms: RwSignal::new(js_sys::Date::now()),
            woke_at_ms: RwSignal::new(0.0),
            idle_index: RwSignal::new(0),
            idle_rotated_ms: RwSignal::new(0.0),
            bloom_seq: RwSignal::new(0),
        }
    }

    /// Nothing that needs a person's attention right now.
    pub(crate) fn calm(&self) -> bool {
        !self.data_error.get()
            && self.is_houseparty()
            && self.pending.with(|p| p.is_none())
            && self.error.with(|e| e.is_none())
            && self.flow.with(|f| *f == Flow::Idle)
    }

    /// Whether the idle screensaver is showing.
    pub(crate) fn screensaver_on(&self) -> bool {
        let Some(after) = self.config.with(|c| {
            c.idle
                .as_ref()
                .filter(|i| !i.media.is_empty())
                .map(|i| i.after.unwrap_or(policy::IDLE_AFTER_DEFAULT_S))
        }) else {
            return false;
        };
        policy::screensaver_allowed(self.display(), self.calm())
            && policy::should_idle(self.last_activity_ms.get(), self.now_ms.get(), after)
    }

    /// Count as activity: restarts the idle timer (and wakes the screen).
    pub(crate) fn activity(&self) {
        self.last_activity_ms.set(js_sys::Date::now());
    }

    pub(crate) fn display(&self) -> DisplayState {
        display_state(
            self.link.get(),
            self.alarm.with(|a| a.as_ref().map(|a| a.state)),
        )
    }

    pub(crate) fn is_houseparty(&self) -> bool {
        self.alarm
            .with(|a| a.as_ref().is_some_and(|a| a.hp.is_some()))
    }

    /// Controls work only on proven-live, readable HouseParty data with
    /// nothing pending.
    pub(crate) fn controls_enabled(&self) -> bool {
        self.link.get() == Link::Live
            && !self.data_error.get()
            && self.is_houseparty()
            && self.pending.with(|p| p.is_none())
    }

    pub(crate) fn touch(&self) {
        self.last_input_ms.set(js_sys::Date::now());
        self.activity();
    }
}

/// A short vibration, where the device supports it and haptics are on.
pub(crate) fn buzz(s: Store, pattern: &[u32]) {
    if s.config.with_untracked(|c| c.haptics == Some(false)) {
        return;
    }
    if let Some(nav) = web_sys::window().map(|w| w.navigator()) {
        let arr = js_sys::Array::new();
        for ms in pattern {
            arr.push(&JsValue::from(*ms));
        }
        let _ = nav.vibrate_with_pattern(&arr);
    }
}

pub(crate) const BUZZ_TAP: &[u32] = &[8];
pub(crate) const BUZZ_OK: &[u32] = &[18];
pub(crate) const BUZZ_ERR: &[u32] = &[30, 50, 30];

fn set_if_changed<T: PartialEq + Send + Sync + 'static>(sig: RwSignal<T>, value: T) {
    if sig.with_untracked(|v| *v != value) {
        sig.set(value);
    }
}

// ---------------------------------------------------------------------------
// Non-reactive, per-instance secrets and callbacks.
// ---------------------------------------------------------------------------

struct Instance {
    call_service: js_sys::Function,
    pin: PinBuffer,
}

thread_local! {
    static INSTANCES: RefCell<HashMap<u32, Instance>> = RefCell::new(HashMap::new());
}
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

pub(crate) fn with_pin<R>(id: u32, f: impl FnOnce(&mut PinBuffer) -> R) -> Option<R> {
    INSTANCES.with(|m| m.borrow_mut().get_mut(&id).map(|i| f(&mut i.pin)))
}

/// Ask the shim to send a command. `bypass = Some(ids)` makes it a force-arm
/// through `houseparty.arm`, approving exactly those sensors.
pub(crate) fn call_service(
    id: u32,
    action: Action,
    code: Option<String>,
    bypass: Option<&[String]>,
) -> Result<(), String> {
    let func = INSTANCES
        .with(|m| m.borrow().get(&id).map(|i| i.call_service.clone()))
        .ok_or_else(|| "This card was removed.".to_string())?;
    let data = js_sys::Object::new();
    let result = (|| {
        if let Some(code) = code.as_deref().filter(|c| !c.is_empty()) {
            js_sys::Reflect::set(&data, &JsValue::from_str("code"), &JsValue::from_str(code))?;
        }
        if let Some(ids) = bypass {
            let arr = js_sys::Array::new();
            for id in ids {
                arr.push(&JsValue::from_str(id));
            }
            js_sys::Reflect::set(&data, &JsValue::from_str("force"), &JsValue::TRUE)?;
            js_sys::Reflect::set(&data, &JsValue::from_str("bypass"), &arr)?;
        }
        func.call2(&JsValue::NULL, &JsValue::from_str(action.service()), &data)
    })();
    if let Some(code) = code {
        wipe_string(code);
    }
    result.map(|_| ()).map_err(|e| {
        e.dyn_ref::<js_sys::Error>()
            .map(|err| String::from(err.message()))
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "Couldn't send the command to Home Assistant.".into())
            .chars()
            .take(200)
            .collect()
    })
}

pub(crate) fn secure_shuffle() -> [u8; 10] {
    let Some(crypto) = web_sys::window().and_then(|w| w.crypto().ok()) else {
        return keypad::STANDARD;
    };
    keypad::shuffled_digits(|| {
        let mut b = [0u8; 4];
        let _ = crypto.get_random_values_with_u8_array(&mut b);
        u32::from_le_bytes(b)
    })
}

fn settle_pending(s: Store, now_ms: f64) {
    let Some(p) = s.pending.get_untracked() else {
        return;
    };
    let Some((obs, report)) = s.alarm.with_untracked(|a| {
        a.as_ref().map(|a| {
            let report = a.report();
            (
                Observation {
                    state: a.state,
                    updated_ms: a.updated_ms,
                    blocked: !report.ready(),
                },
                report,
            )
        })
    }) else {
        return;
    };

    let mut resolution = p.resolve(obs, now_ms);
    // Refused for the same sensors as last time: HouseParty still rewrites
    // the attribute, but if `last_updated` didn't move, a timeout with
    // blockers present is the same refusal.
    if resolution == Resolution::TimedOut && obs.blocked && matches!(p.action, Action::Arm(_)) {
        resolution = Resolution::Refused;
    }

    match resolution {
        Resolution::Waiting => {}
        Resolution::Confirmed => {
            s.pending.set(None);
            buzz(s, BUZZ_OK);
        }
        Resolution::Refused => {
            s.pending.set(None);
            buzz(s, BUZZ_ERR);
            s.touch();
            s.flow.set(Flow::Confirming(p.action, report));
        }
        Resolution::Diverged(other) => {
            s.pending.set(None);
            buzz(s, BUZZ_ERR);
            s.error.set(Some(format!(
                "The alarm changed to “{}” before your command was confirmed. Check it before trying again.",
                other.label()
            )));
        }
        Resolution::TimedOut => {
            s.pending.set(None);
            buzz(s, BUZZ_ERR);
            s.error.set(Some(
                "No confirmation from Home Assistant. Check the alarm state before trying again."
                    .into(),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// The exported card.
// ---------------------------------------------------------------------------

#[wasm_bindgen]
pub struct AlarmCard {
    id: u32,
    store: Store,
    // Type-erasure keeps the concrete Leptos view state out of the public
    // bridge. Dropping the handle unmounts the view and cancels its owner.
    mount: Option<Box<dyn Any>>,
}

#[wasm_bindgen]
impl AlarmCard {
    #[wasm_bindgen(constructor)]
    pub fn new(mount: web_sys::HtmlElement, call_service: js_sys::Function) -> AlarmCard {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        INSTANCES.with(|m| {
            m.borrow_mut().insert(
                id,
                Instance {
                    call_service,
                    pin: PinBuffer::new(),
                },
            )
        });
        let store = Store::new(id);
        let mount = leptos::mount::mount_to(mount, move || {
            provide_context(store);
            view! { <components::Card/> }
        });
        AlarmCard {
            id,
            store,
            mount: Some(Box::new(mount)),
        }
    }

    pub fn set_config(&self, raw: JsValue) -> Result<(), JsValue> {
        let cfg: CardConfig = serde_wasm_bindgen::from_value(raw)?;
        components::cancel_flow(self.store);
        self.store.config.set(cfg);
        Ok(())
    }

    /// Never throws. Unreadable data keeps the last good view, controls off.
    pub fn update(&self, raw: JsValue) {
        let s = self.store;
        let snap: Snapshot = match serde_wasm_bindgen::from_value(raw) {
            Ok(snap) => snap,
            Err(_) => {
                components::cancel_flow(s);
                set_if_changed(s.data_error, true);
                return;
            }
        };

        let alarm = snap.alarm.map(|e| AlarmView {
            state: AlarmState::from_ha(&e.state),
            name: e.friendly_name.unwrap_or(e.entity_id),
            code_format: CodeFormat::from_ha(e.code_format.as_deref()),
            code_arm_required: e.code_arm_required,
            features: e.supported_features.unwrap_or(0),
            changed_by: e.changed_by.filter(|c| !c.is_empty()),
            last_changed_ms: e.last_changed_ms,
            updated_ms: e.last_updated_ms,
            arm_mode: e
                .arm_mode
                .as_deref()
                .and_then(|m| ArmMode::from_armed_state(AlarmState::from_ha(m))),
            delay: e.delay,
            hp: e.houseparty,
        });

        let prev = s.alarm.with_untracked(|a| a.as_ref().map(|a| a.state));
        let next = alarm.as_ref().map(|a| a.state);
        if prev != next {
            // A state change is worth seeing: wake, and restart the idle timer.
            s.activity();
            if prev.is_some() && next.is_some() {
                s.bloom_seq.update(|n| *n = n.wrapping_add(1));
            }
            if s.flow.with_untracked(|f| *f != Flow::Idle) {
                components::cancel_flow(s);
            }
        }

        set_if_changed(s.now_ms, snap.now_ms);
        set_if_changed(s.alarm, alarm);
        set_if_changed(s.data_error, false);
        set_if_changed(s.loaded, true);

        // An approval must still match HouseParty's latest list. (The server
        // enforces this too; this keeps the screen honest.)
        if let Flow::Confirming(action, approved) = s.flow.get_untracked() {
            let fresh = s
                .alarm
                .with_untracked(|a| a.as_ref().map(|a| a.report()).unwrap_or_default());
            if fresh.has_new_blockers(&approved) {
                s.flow.set(Flow::Confirming(action, fresh));
                s.error
                    .set(Some("The list changed. Review it before arming.".into()));
            }
        }

        settle_pending(s, snap.now_ms);
    }

    /// "live", "stale" or "lost". Anything else counts as stale.
    pub fn set_link(&self, link: &str) {
        let s = self.store;
        let link = Link::from_link_status(link);
        if link != Link::Live {
            components::cancel_flow(s);
        }
        if link == Link::Lost && s.pending.get_untracked().is_some() {
            s.pending.set(None);
            s.error.set(Some(
                "Connection dropped before Home Assistant confirmed. Check the alarm state once it reconnects."
                    .into(),
            ));
        }
        if s.link.get_untracked() != link {
            s.activity();
        }
        set_if_changed(s.link, link);
    }

    pub fn tick(&self, now_ms: f64) {
        let s = self.store;
        s.now_ms.set(now_ms);
        settle_pending(s, now_ms);
        let in_flow = s.flow.with_untracked(|f| *f != Flow::Idle);
        if in_flow && policy::is_idle(s.last_input_ms.get_untracked(), now_ms) {
            components::cancel_flow(s);
        }
        if s.error.with_untracked(|e| e.is_some())
            && policy::error_expired(s.error_since_ms.get_untracked(), now_ms)
        {
            s.error.set(None);
        }
        // Rotate the idle media while it's showing.
        if untrack(|| s.screensaver_on()) {
            let (len, every) = s.config.with_untracked(|c| {
                c.idle.as_ref().map_or((0, 0), |i| {
                    (i.media.len(), i.rotate.unwrap_or(policy::ROTATE_DEFAULT_S))
                })
            });
            if len > 1 && now_ms - s.idle_rotated_ms.get_untracked() >= f64::from(every) * 1000.0 {
                let next =
                    policy::next_media(s.idle_index.get_untracked(), len, js_sys::Math::random());
                s.idle_index.set(next);
                s.idle_rotated_ms.set(now_ms);
            }
        }
    }

    pub fn reset_flow(&self) {
        components::cancel_flow(self.store);
        self.store.activity();
        set_if_changed(self.store.revealed_until_ms, 0.0);
        set_if_changed(self.store.error, None);
    }

    /// HouseParty rejected a command (wrong code, locked out, not allowed).
    pub fn report_error(&self, message: String) {
        let s = self.store;
        s.pending.set(None);
        buzz(s, BUZZ_ERR);
        let msg: String = message.chars().take(200).collect();
        s.error.set(Some(if msg.trim().is_empty() {
            "Home Assistant rejected the command.".into()
        } else {
            msg
        }));
    }

    pub fn destroy(mut self) {
        self.cleanup();
    }

    fn cleanup(&mut self) {
        if self.mount.is_none() {
            return;
        }
        components::cancel_flow(self.store);
        INSTANCES.with(|m| m.borrow_mut().remove(&self.id));
        // Dropping UnmountHandle unmounts the DOM and cancels reactive work.
        self.mount.take();
    }
}

impl Drop for AlarmCard {
    fn drop(&mut self) {
        self.cleanup();
    }
}
