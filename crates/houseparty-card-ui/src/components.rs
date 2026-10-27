//! Leptos components. All text goes through Leptos text nodes (escaped);
//! nothing here uses `inner_html`, so entity names can't inject markup.

use houseparty_core::alarm::{
    available_actions, needs_code, Action, AlarmState, CodeFormat, DisplayState, PendingAction,
};
use houseparty_core::countdown;
use houseparty_core::policy::{self, MASK_DOTS, REVEAL_MS};
use houseparty_core::sensors::PreArmReport;
use leptos::prelude::*;

use crate::{buzz, call_service, secure_shuffle, with_pin, Flow, Store, BUZZ_TAP};

const STYLE: &str = include_str!("style.css");

// ---------------------------------------------------------------------------
// Flow helpers (also used by the bridge in lib.rs)
// ---------------------------------------------------------------------------

/// Leave any confirm/code-entry flow and wipe the code buffer.
pub(crate) fn cancel_flow(s: Store) {
    with_pin(s.instance, |p| p.clear());
    s.pin_len.set(0);
    if s.flow.with_untracked(|f| *f != Flow::Idle) {
        s.flow.set(Flow::Idle);
    }
}

fn controls_ok(s: Store) -> bool {
    // The tap that dismissed the screensaver must not also press a button.
    let guarded = js_sys::Date::now() - s.woke_at_ms.get_untracked() < policy::WAKE_GUARD_MS;
    !guarded && untrack(|| s.controls_enabled())
}

/// HouseParty decides readiness, so arming just asks; a refusal comes back
/// as a list (see settle_pending) and opens the "Not ready" panel.
fn start(s: Store, action: Action) {
    if !controls_ok(s) {
        return;
    }
    buzz(s, BUZZ_TAP);
    s.touch();
    s.error.set(None);
    proceed(s, action, None);
}

fn proceed(s: Store, action: Action, bypass: Option<Vec<String>>) {
    if !controls_ok(s) {
        cancel_flow(s);
        return;
    }
    s.touch();
    let needs = s.alarm.with_untracked(|a| {
        a.as_ref()
            .map(|a| needs_code(action, a.code_format, a.code_arm_required))
    });
    match needs {
        None => cancel_flow(s),
        Some(true) => {
            with_pin(s.instance, |p| p.clear());
            s.pin_len.set(0);
            s.keypad.set(secure_shuffle());
            s.flow.set(Flow::EnteringCode(action, bypass));
        }
        Some(false) => send(s, action, None, bypass),
    }
}

/// Send and wait. The UI shows "waiting" until HouseParty reports the
/// expected state; it never shows the hoped-for state on tap.
fn send(s: Store, action: Action, code: Option<String>, bypass: Option<Vec<String>>) {
    cancel_flow(s);
    if !controls_ok(s) {
        return;
    }
    let Some((from, baseline)) = s
        .alarm
        .with_untracked(|a| a.as_ref().map(|a| (a.state, a.updated_ms)))
    else {
        return;
    };
    match call_service(s.instance, action, code, bypass.as_deref()) {
        Ok(()) => s.pending.set(Some(PendingAction {
            action,
            forced: bypass.is_some(),
            from,
            started_ms: js_sys::Date::now(),
            baseline_updated_ms: baseline,
        })),
        Err(message) => s.error.set(Some(message)),
    }
}

// ---------------------------------------------------------------------------
// Root
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Loading,
    Lost,
    Checking,
    Missing,
    NotHouseParty,
    Live,
}

#[component]
pub(crate) fn Card() -> impl IntoView {
    let s = expect_context::<Store>();
    let kind = Memo::new(move |_| match s.display() {
        DisplayState::ConnectionLost => Kind::Lost,
        DisplayState::Checking => Kind::Checking,
        DisplayState::Missing if !s.loaded.get() => Kind::Loading,
        DisplayState::Missing => Kind::Missing,
        DisplayState::Live(_) if !s.is_houseparty() => Kind::NotHouseParty,
        DisplayState::Live(_) => Kind::Live,
    });
    // Stamp each new error so the tick can expire it.
    Effect::new(move |_| {
        if s.error.with(|e| e.is_some()) {
            s.error_since_ms.set(js_sys::Date::now());
        }
    });
    let title = move || {
        s.config
            .with(|c| c.name.clone())
            .or_else(|| s.alarm.with(|a| a.as_ref().map(|a| a.name.clone())))
            .unwrap_or_else(|| "HouseParty".to_string())
    };

    let idle = Memo::new(move |_| s.screensaver_on());

    view! {
        <style>{STYLE}</style>
        // Any touch on the card counts as activity and restarts the idle timer,
        // except while the screensaver is up: then only its own click wakes,
        // so the overlay is still there when the click lands.
        <div
            class="card"
            data-motion=move || s.config.with(|c| c.motion.clone().unwrap_or_else(|| "full".into()))
            data-idle=move || idle.get().then_some("true")
            on:pointerdown=move |_| if !idle.get_untracked() { s.activity() }
        >
            <Show when=move || idle.get()>
                <Screensaver/>
            </Show>
            <h2 class="title">{title}</h2>
            {move || match kind.get() {
                Kind::Loading => view! {
                    <p class="notice" role="status">"Waiting for Home Assistant…"</p>
                }.into_any(),
                Kind::Lost => view! { <ConnectionLost/> }.into_any(),
                Kind::Checking => view! { <Checking/> }.into_any(),
                Kind::Missing => view! {
                    <p class="notice" role="alert">
                        "Alarm not found. Check the entity in this card's settings."
                    </p>
                }.into_any(),
                Kind::NotHouseParty => view! {
                    <p class="notice" role="alert">
                        "This card only works with HouseParty alarms. Pick a HouseParty alarm in the card's settings."
                    </p>
                }.into_any(),
                Kind::Live => view! {
                    <StatusRing/>
                    <DataError/>
                    <Controls/>
                    <Details/>
                }.into_any(),
            }}
        </div>
    }
}

#[component]
fn ConnectionLost() -> impl IntoView {
    let s = expect_context::<Store>();
    view! {
        <div class="lost" role="alert">
            <p class="state-label">"Connection lost"</p>
            <p class="meta">
                "The alarm state is unknown until Home Assistant reconnects. Controls are off until then."
            </p>
        </div>
        {move || s.error.get().map(|e| view! { <p class="banner error" role="alert">{e}</p> })}
    }
}

#[component]
fn Checking() -> impl IntoView {
    view! {
        <div class="lost" role="status">
            <span class="spinner" aria-hidden="true"></span>
            <p class="state-label">"Checking connection…"</p>
            <p class="meta">
                "Making sure what's shown is current. Controls come back once Home Assistant answers."
            </p>
        </div>
    }
}

#[component]
fn DataError() -> impl IntoView {
    let s = expect_context::<Store>();
    view! {
        <Show when=move || s.data_error.get()>
            <p class="banner error" role="alert">
                "Home Assistant sent data this card couldn't read. Showing the last good state with controls off."
            </p>
        </Show>
    }
}

// ---------------------------------------------------------------------------
// Status ring
// ---------------------------------------------------------------------------

#[component]
fn StatusRing() -> impl IntoView {
    let s = expect_context::<Store>();
    let state = Memo::new(move |_| {
        s.alarm
            .with(|a| a.as_ref().map(|a| a.state))
            .unwrap_or(AlarmState::Unknown)
    });
    let urgent = move || state.get() == AlarmState::Triggered;

    // (delay, started, show_number), from HouseParty's reported delay.
    let timing = Memo::new(move |_| {
        let show_entry = s.config.with(|c| c.show_entry_countdown == Some(true));
        let (reported, started) = s.alarm.with(|a| {
            a.as_ref()
                .map(|a| (a.delay, a.last_changed_ms))
                .unwrap_or((None, 0.0))
        });
        let st = state.get();
        // An intruder at the door shouldn't read their deadline.
        let show_number = st == AlarmState::Arming || (st == AlarmState::Pending && show_entry);
        countdown::delay_for(st, reported).map(|d| (d, started, show_number))
    });
    let label = Memo::new(move |_| {
        let st = state.get();
        let mode = s.alarm.with(|a| a.as_ref().and_then(|a| a.arm_mode));
        match (st, mode) {
            (AlarmState::Arming, Some(m)) => format!("Arming {}", m.name()),
            _ => st.label().to_string(),
        }
    });

    // Computed once per delay start: CSS animates the drain from there.
    let ring_style = move || match timing.get() {
        Some((d, started, _)) => {
            let elapsed = countdown::elapsed_secs(started, d, js_sys::Date::now());
            format!("--delay:{d}s;--elapsed:{elapsed:.2}s")
        }
        None => String::new(),
    };
    let remaining = move || {
        timing.get().and_then(|(d, started, show)| {
            show.then(|| format!("{}s", countdown::remaining_secs(started, d, s.now_ms.get())))
        })
    };

    let sending = move || s.pending.with(|p| p.is_some());

    view! {
        <div
            class="status"
            data-tone=move || state.get().tone().as_str()
            class:counting=move || timing.get().is_some()
            class:urgent=urgent
            class:entry=move || state.get() == AlarmState::Pending
            class:sending=sending
        >
            // One-shot bloom, re-created (so replayed) only when HouseParty
            // reports a new state. Never on tap.
            {move || {
                let n = s.bloom_seq.get();
                (n > 0).then(|| view! { <span class="bloom" data-seq=n.to_string() aria-hidden="true"></span> })
            }}
            <svg class="ring" viewBox="0 0 120 120" aria-hidden="true">
                <circle class="ring-track" cx="60" cy="60" r="52"></circle>
                <circle class="ring-fill" cx="60" cy="60" r="52" style=ring_style></circle>
                // "Sending": a comet orbits while a command awaits confirmation.
                <g class="comet">
                    <circle class="comet-arc" cx="60" cy="60" r="52"></circle>
                </g>
            </svg>
            <div
                class="status-text"
                role="status"
                aria-live=move || if urgent() { "assertive" } else { "polite" }
            >
                // Re-mounted on change so the new label eases in. The text
                // itself is never delayed.
                {move || {
                    let text = label.get();
                    view! { <span class="state-label">{text}</span> }
                }}
                <span class="countdown">{remaining}</span>
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

fn pending_text(p: PendingAction) -> String {
    format!(
        "{} sent. Waiting for Home Assistant to confirm…",
        p.action.label()
    )
}

#[component]
fn Controls() -> impl IntoView {
    let s = expect_context::<Store>();
    view! {
        {move || s.pending.get().map(|p| view! {
            <p class="banner pending" role="status">{pending_text(p)}</p>
        })}
        {move || s.error.get().map(|e| view! {
            <p class="banner error" role="alert">{e}</p>
        })}
        {move || match s.flow.get() {
            Flow::Idle => view! { <ActionButtons/> }.into_any(),
            Flow::Confirming(action, report) => {
                view! { <PreArmPanel action=action report=report/> }.into_any()
            }
            Flow::EnteringCode(action, bypass) => {
                view! { <CodeEntry action=action bypass=bypass/> }.into_any()
            }
        }}
    }
}

#[component]
fn ActionButtons() -> impl IntoView {
    let s = expect_context::<Store>();
    let actions = Memo::new(move |_| {
        s.alarm.with(|a| {
            a.as_ref()
                .map(|a| available_actions(a.state, a.features))
                .unwrap_or_default()
        })
    });
    let disabled = move || !s.controls_enabled();

    view! {
        <div class="actions">
            <For each=move || actions.get() key=|a| *a let:action>
                <button
                    class="action"
                    class:primary=matches!(action, Action::Disarm)
                    disabled=disabled
                    on:click=move |_| start(s, action)
                >
                    {action.label()}
                </button>
            </For>
        </div>
    }
}

#[component]
fn PreArmPanel(action: Action, report: PreArmReport) -> impl IntoView {
    let s = expect_context::<Store>();
    let list = |title: &'static str, items: Vec<String>| {
        (!items.is_empty()).then(|| {
            view! {
                <p class="list-title">{title}</p>
                <ul class="blockers">
                    {items.into_iter().map(|n| view! { <li>{n}</li> }).collect_view()}
                </ul>
            }
        })
    };
    let approve = report.ids();

    view! {
        <div class="prearm" role="alertdialog" aria-labelledby="prearm-title">
            <p class="prearm-title" id="prearm-title">"Not ready to arm"</p>
            {list("Open", report.open_names())}
            {list("Not reporting", report.not_reporting_names())}
            <p class="meta">
                "Arming anyway bypasses exactly these until you disarm. If anything else opens first, HouseParty won't arm."
            </p>
            <div class="row">
                <button on:click=move |_| cancel_flow(s)>"Cancel"</button>
                <button class="warn" on:click=move |_| proceed(s, action, Some(approve.clone()))>
                    {format!("{} anyway", action.label())}
                </button>
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Code entry
// ---------------------------------------------------------------------------

#[component]
fn CodeEntry(action: Action, bypass: Option<Vec<String>>) -> impl IntoView {
    let s = expect_context::<Store>();
    let numeric = s.alarm.with_untracked(|a| {
        a.as_ref()
            .map(|a| a.code_format == CodeFormat::Number)
            .unwrap_or(true)
    });
    if numeric {
        view! { <Keypad action=action bypass=bypass/> }.into_any()
    } else {
        view! { <TextCode action=action bypass=bypass/> }.into_any()
    }
}

#[component]
fn Keypad(action: Action, bypass: Option<Vec<String>>) -> impl IntoView {
    let s = expect_context::<Store>();
    let digits = s.keypad.get_untracked(); // fixed for this entry
    let entered = move || s.pin_len.get() > 0;
    let bypass = StoredValue::new(bypass);

    let key = move |d: u8| {
        view! {
            <button
                class="key"
                on:click=move |_| {
                    s.touch();
                    buzz(s, BUZZ_TAP);
                    if let Some(len) = with_pin(s.instance, |p| {
                        p.push_digit(char::from(b'0' + d));
                        p.len()
                    }) {
                        s.pin_len.set(len);
                    }
                }
            >
                {d.to_string()}
            </button>
        }
    };
    let backspace = move |_| {
        s.touch();
        if let Some(len) = with_pin(s.instance, |p| {
            p.pop();
            p.len()
        }) {
            s.pin_len.set(len);
        }
    };
    let submit = move |_| {
        let code = with_pin(s.instance, |p| p.take());
        s.pin_len.set(0);
        if let Some(code) = code.filter(|c| !c.is_empty()) {
            send(s, action, Some(code), bypass.get_value());
        }
    };

    // A fixed row of dots, all lit once anything is typed: shows that a code
    // is entered without revealing its length. Re-rendered on every press so
    // the row pulses as feedback (the presses themselves are visible anyway).
    let dots = move || {
        let _press = s.pin_len.get();
        let lit = entered();
        (0..MASK_DOTS)
            .map(|_| view! { <span class="pin-dot" class:lit=lit></span> })
            .collect_view()
    };

    view! {
        <div class="code-entry">
            <p class="code-title">{format!("Enter code to {}", action.verb())}</p>
            <div class="pin-dots" aria-hidden="true">{dots}</div>
            <span class="sr-only" aria-live="polite">
                {move || if entered() { "Code entered" } else { "No code entered" }}
            </span>
            <div class="keypad" role="group" aria-label="Code keypad">
                {digits[..9].iter().map(|&d| key(d)).collect_view()}
                <button class="key quiet" on:click=move |_| cancel_flow(s)>"Cancel"</button>
                {key(digits[9])}
                <button class="key quiet" aria-label="Delete last digit" on:click=backspace>
                    "⌫"
                </button>
            </div>
            <button class="primary wide" disabled=move || !entered() on:click=submit>
                {action.label()}
            </button>
        </div>
    }
}

#[component]
fn TextCode(action: Action, bypass: Option<Vec<String>>) -> impl IntoView {
    let s = expect_context::<Store>();
    let input: NodeRef<leptos::html::Input> = NodeRef::new();
    let bypass = StoredValue::new(bypass);
    let submit = move || {
        let Some(el) = input.get_untracked() else {
            return;
        };
        let code = el.value();
        el.set_value("");
        if !code.is_empty() {
            send(s, action, Some(code), bypass.get_value());
        }
    };

    // `data-1p-ignore` starts with a digit after the dash, which the view
    // macro may not parse, so it's set on mount instead.
    Effect::new(move |_| {
        if let Some(el) = input.get() {
            let _ = el.set_attribute("data-1p-ignore", "true");
        }
    });

    // Password managers key on type="password"; these hints ask the common
    // ones not to offer to save or autofill the alarm code.
    view! {
        <div class="code-entry">
            <label class="code-title">
                {format!("Enter code to {}", action.verb())}
                <input
                    node_ref=input
                    type="password"
                    autocomplete="one-time-code"
                    autocapitalize="off"
                    spellcheck="false"
                    maxlength="64"
                    data-lpignore="true"
                    data-bwignore="true"
                    data-form-type="other"
                    on:input=move |_| s.touch()
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" {
                            submit();
                        }
                    }
                />
            </label>
            <div class="row">
                <button on:click=move |_| cancel_flow(s)>"Cancel"</button>
                <button class="primary" on:click=move |_| submit()>{action.label()}</button>
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Details (privacy-gated)
// ---------------------------------------------------------------------------

#[component]
fn Details() -> impl IntoView {
    let s = expect_context::<Store>();
    let visible = Memo::new(move |_| {
        !s.config.with(|c| c.privacy())
            || policy::is_revealed(s.revealed_until_ms.get(), s.now_ms.get())
    });
    let privacy = move || s.config.with(|c| c.privacy());
    let reveal = move |_| s.revealed_until_ms.set(js_sys::Date::now() + REVEAL_MS);
    let hide = move |_| s.revealed_until_ms.set(0.0);

    let changed_by = move || {
        s.alarm
            .with(|a| a.as_ref().and_then(|a| a.changed_by.clone()))
    };
    let names = move |pick: fn(&crate::HouseParty) -> Vec<String>| {
        s.alarm.with(|a| {
            a.as_ref()
                .and_then(|a| a.hp.as_ref())
                .map(pick)
                .unwrap_or_default()
        })
    };
    let bypassed = move || names(|h| h.bypassed.iter().map(|b| b.name.clone()).collect());
    let triggered_by = move || names(|h| h.triggered_by.iter().map(|b| b.name.clone()).collect());
    let blocked = move || names(|h| h.open_sensors.iter().map(|b| b.name.clone()).collect());
    let disarmed =
        move || s.alarm.with(|a| a.as_ref().map(|a| a.state)) == Some(AlarmState::Disarmed);

    let section = move |title: &'static str, items: Vec<String>| {
        (!items.is_empty()).then(|| {
            view! {
                <p class="list-title">{title}</p>
                <ul class="blockers">
                    {items.into_iter().map(|n| view! { <li>{n}</li> }).collect_view()}
                </ul>
            }
        })
    };

    view! {
        <Show
            when=move || visible.get()
            fallback=move || view! {
                <button class="reveal" on:click=reveal>"Show details"</button>
            }
        >
            <div class="details">
                {move || changed_by().map(|who| view! { <p class="meta">"Last changed by " {who}</p> })}
                {move || section("Bypassed (not protected until disarmed)", bypassed())}
                {move || section("Triggered by", triggered_by())}
                {move || disarmed().then(|| section("Blocked the last arming attempt", blocked()))}
            </div>
            <Show when=privacy>
                <button class="reveal" on:click=hide>"Hide details"</button>
            </Show>
        </Show>
    }
}

// ---------------------------------------------------------------------------
// Idle screensaver
// ---------------------------------------------------------------------------

fn prefers_reduced_motion() -> bool {
    web_sys::window()
        .and_then(|w| {
            w.match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .is_some_and(|m| m.matches())
}

/// Covers the card while idle. Media is mounted only while showing, so it
/// costs nothing (no download, no decoding) the rest of the time.
#[component]
fn Screensaver() -> impl IntoView {
    let s = expect_context::<Store>();
    let autoplay = !prefers_reduced_motion();
    let src = Memo::new(move |_| {
        let i = s.idle_index.get();
        s.config.with(|c| {
            c.idle
                .as_ref()
                .and_then(|idle| idle.media.get(i % idle.media.len().max(1)).cloned())
        })
    });
    let state = move || {
        s.alarm
            .with(|a| a.as_ref().map(|a| a.state))
            .unwrap_or(AlarmState::Unknown)
    };
    // The chip moves corner to corner with each new clip, against burn-in.
    let corner = move || s.idle_index.get() % 4;

    let video: NodeRef<leptos::html::Video> = NodeRef::new();
    // Set muted/loop as properties before playing: autoplay needs muted
    // *property* set, and `loop` is a Rust keyword in the view macro.
    Effect::new(move |_| {
        if let Some(v) = video.get() {
            v.set_muted(true);
            v.set_loop(true);
            if autoplay {
                let _ = v.play();
            }
        }
    });

    // Wake on click, not pointerdown: the overlay is still there when the
    // click lands, so the tap can't fall through to a button underneath.
    let wake = move || {
        s.woke_at_ms.set(js_sys::Date::now());
        s.activity();
    };

    view! {
        <div
            class="idle"
            role="button"
            tabindex="0"
            aria-label="Alarm panel is idle. Tap to wake."
            on:click=move |ev| {
                ev.prevent_default();
                ev.stop_propagation();
                wake();
            }
            on:keydown=move |ev| {
                if ev.key() == "Enter" || ev.key() == " " {
                    ev.prevent_default();
                    wake();
                }
            }
        >
            {move || src.get().map(|path| {
                if policy::is_video(&path) {
                    view! {
                        <video
                            node_ref=video
                            class="idle-media"
                            src=path
                            playsinline="true"
                            preload="auto"
                            disablepictureinpicture="true"
                            aria-hidden="true"
                        ></video>
                    }.into_any()
                } else {
                    view! {
                        <img class="idle-media" src=path alt="" decoding="async" aria-hidden="true"/>
                    }.into_any()
                }
            })}
            <div class="idle-chip" data-corner=corner data-tone=move || state().tone().as_str()>
                <span class="idle-dot" aria-hidden="true"></span>
                {move || state().label()}
            </div>
        </div>
    }
}
