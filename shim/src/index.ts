/**
 * Thin bridge between Lovelace and the HouseParty card (Rust/WASM).
 *
 * The HouseParty integration is the alarm: it checks codes, locks out
 * repeated wrong guesses (per HA user, server-side, where page scripts can't
 * skip it), decides readiness, and bypasses only approved sensors. This shim
 * runs with full access to the HA connection, so it stays small:
 *  - only HouseParty alarm entities are accepted
 *  - only alarm arm/disarm services, plus `houseparty.arm` for force-arming,
 *    always pinned to the configured entity
 *  - entity data is sanitized (types, ranges, lengths) before WASM sees it
 *  - the link is proven with a heartbeat; unproven data is never shown live
 *  - the WASM binary is hash-checked before it runs
 *  - if WASM can't load or traps, a plain-DOM fallback offers status and
 *    Disarm only
 *  - entity text is only ever set with textContent, never innerHTML
 *
 * Every script on this page (other cards, themes, plugins) shares this
 * origin. A closed shadow root and private fields stop casual reading, not a
 * determined script. Treat every frontend plugin as trusted with your code.
 */
import init, { AlarmCard } from "../../pkg/houseparty_card_ui.js";

declare const __WASM_SHA256__: string; // injected at build time

const TAG = "houseparty-card";
const DOMAIN = "alarm_control_panel";
const WASM_FILE = "houseparty-card_bg.wasm";
const ALLOWED_SERVICES = new Set([
  "alarm_disarm",
  "alarm_arm_home",
  "alarm_arm_away",
  "alarm_arm_night",
  "alarm_arm_vacation",
  "alarm_arm_custom_bypass",
]);
/** alarm service -> HouseParty mode, for force-arming through `houseparty.arm`. */
const HP_MODES: Record<string, string> = {
  alarm_arm_away: "away",
  alarm_arm_home: "home",
  alarm_arm_night: "night",
  alarm_arm_vacation: "vacation",
  alarm_arm_custom_bypass: "custom",
};
const HP_API = 1;
const FORBIDDEN_KEYS = ["code", "pin", "passcode", "password", "token", "access_token"];
// Settings from before HouseParty owned sensors and delays.
const RETIRED_KEYS = ["sensors", "exit_delay", "entry_delay", "sensor_source"];
const ENTITY_RE = /^alarm_control_panel\.[a-z0-9_]+$/;
const SENSOR_RE = /^binary_sensor\.[a-z0-9_]+$/;
const MAX_SENSORS = 128;
const MAX_DELAY_S = 3600;
const MAX_TEXT = 120;
const MAX_STATE = 64;

const HEARTBEAT_MS = 10_000;
const HEARTBEAT_TIMEOUT_MS = 5_000;
const WASM_CACHE = `${TAG}-wasm`;
const IDLE_CLEAR_MS = 20_000;

// Idle screensaver media: files under /local only (served by your own HA),
// never remote URLs, so no third party is put on the alarm panel. Path
// segments must start with a letter, digit, _ or -, which rules out "..".
const IDLE_MEDIA_RE = /^\/local\/(?:[A-Za-z0-9_-][A-Za-z0-9_.-]*\/)*[A-Za-z0-9_-][A-Za-z0-9_.-]*\.(?:gif|webp|png|jpe?g|mp4|webm)$/i;
const IDLE_MAX_MEDIA = 50;
const IDLE_KEYS = new Set(["media", "after", "rotate"]);
const ANIMATION_LEVELS = new Set(["auto", "full", "subtle", "off"]);

/**
 * "auto" goes subtle on low-end hardware (the old phones this card is often
 * run on), full otherwise. The device's own reduce-motion setting is
 * honored separately in CSS and always wins.
 */
function resolveMotion(setting: string): CardConfig["motion"] {
  if (setting === "full" || setting === "subtle" || setting === "off") return setting;
  const nav = navigator as Navigator & { deviceMemory?: number };
  const cores = typeof nav.hardwareConcurrency === "number" ? nav.hardwareConcurrency : 4;
  const memory = typeof nav.deviceMemory === "number" ? nav.deviceMemory : 4;
  return cores <= 2 || memory <= 1 ? "subtle" : "full";
}

interface HassEntity {
  entity_id: string;
  state: string;
  last_changed: string;
  last_updated?: string;
  attributes: Record<string, unknown>;
}

interface Hass {
  states: Record<string, HassEntity | undefined>;
  connected?: boolean;
  connection?: { ping?: () => Promise<unknown> };
  callService(domain: string, service: string, data: Record<string, unknown>): Promise<unknown>;
}

interface CardConfig {
  type: string;
  entity: string;
  name?: string;
  privacy?: boolean;
  show_entry_countdown?: boolean;
  idle?: { media: string[]; after?: number; rotate?: number };
  /** Resolved motion level passed to the card. */
  motion: "full" | "subtle" | "off";
  haptics?: boolean;
}

type Link = "live" | "stale" | "lost";

// ---------------------------------------------------------------------------
// Module loading: one verified download + compile, shared by every card.
// ---------------------------------------------------------------------------

class InsecureContextError extends Error {}

let wasmReady: Promise<void> | undefined;
let wasmCrashed = false;

const hex = (buf: ArrayBuffer): string =>
  Array.from(new Uint8Array(buf), (b) => b.toString(16).padStart(2, "0")).join("");

/** Bytes from Cache Storage if we've verified this release before, else the network. */
async function fetchWasm(url: URL): Promise<ArrayBuffer> {
  let cache: Cache | undefined;
  try {
    cache = await caches.open(WASM_CACHE);
    const hit = await cache.match(url);
    if (hit) return await hit.arrayBuffer();
  } catch {
    cache = undefined; // Cache Storage unavailable: just use the network
  }
  const res = await fetch(url, { credentials: "same-origin" });
  if (!res.ok) throw new Error(`WASM download failed (${res.status}).`);
  const bytes = await res.arrayBuffer();
  return bytes;
}

async function storeWasm(url: URL, bytes: ArrayBuffer): Promise<void> {
  try {
    const cache = await caches.open(WASM_CACHE);
    // Drop older releases, keep only this one.
    for (const req of await cache.keys()) if (req.url !== url.href) await cache.delete(req);
    await cache.put(url, new Response(bytes, { headers: { "content-type": "application/wasm" } }));
  } catch {
    /* caching is an optimisation only */
  }
}

function loadWasm(): Promise<void> {
  wasmReady ??= (async () => {
    // Hash checks need crypto.subtle, which browsers only offer over HTTPS.
    if (!globalThis.isSecureContext || !globalThis.crypto?.subtle) {
      throw new InsecureContextError("Home Assistant isn't served over HTTPS.");
    }
    // The hash in the URL busts stale caches after an update, so the JS and
    // the WASM can never mismatch.
    const url = new URL(`./${WASM_FILE}?v=${__WASM_SHA256__.slice(0, 16)}`, import.meta.url);
    const bytes = await fetchWasm(url);
    // Verified every time, cached copy included: a tampered cache is refused
    // like a tampered download. Security over speed: no streaming compile.
    if (hex(await crypto.subtle.digest("SHA-256", bytes)) !== __WASM_SHA256__) {
      try {
        await caches.delete(WASM_CACHE);
      } catch {
        /* nothing cached */
      }
      throw new Error("WASM file doesn't match this release. Refusing to run it.");
    }
    await init({ module_or_path: bytes });
    void storeWasm(url, bytes);
  })();
  return wasmReady;
}

// ---------------------------------------------------------------------------
// Heartbeat: one per page, shared by every card. Proves the connection is
// live; a missed answer or a wake from sleep marks every card unproven.
// ---------------------------------------------------------------------------

type PingConn = { ping?: () => Promise<unknown> };

const heartbeat = (() => {
  let conn: PingConn | undefined;
  let ok = false;
  let generation = 0;
  let timer: number | undefined;
  const listeners = new Set<() => void>();
  const visible = new Set<object>();

  const notify = () => listeners.forEach((fn) => fn());
  const set = (v: boolean) => {
    if (v !== ok) {
      ok = v;
      notify();
    }
  };

  async function beat(): Promise<void> {
    const c = conn;
    if (typeof c?.ping !== "function") return;
    const ticket = ++generation;
    const isCurrent = () => c === conn && ticket === generation;
    let to: number | undefined;
    try {
      await Promise.race([
        c.ping.call(c),
        new Promise((_, reject) => {
          to = window.setTimeout(() => reject(new Error("timeout")), HEARTBEAT_TIMEOUT_MS);
        }),
      ]);
      if (isCurrent()) set(true);
    } catch {
      if (isCurrent()) set(false);
    } finally {
      window.clearTimeout(to);
    }
  }

  return {
    /** Latest connection object from hass (it can be replaced on reconnect). */
    setConnection(c: PingConn | undefined) {
      if (c !== conn) {
        conn = c;
        if (visible.size) {
          set(false);
          void beat();
        }
      }
    },
    canPing: () => typeof conn?.ping === "function",
    isOk: () => ok,
    subscribe(fn: () => void) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    /** A card became visible. The first one (a wake) makes the link unproven. */
    show(card: object) {
      const wasIdle = visible.size === 0;
      visible.add(card);
      if (wasIdle) {
        set(false);
        void beat();
        timer = window.setInterval(() => void beat(), HEARTBEAT_MS);
      }
    },
    hide(card: object) {
      visible.delete(card);
      if (visible.size === 0) {
        window.clearInterval(timer);
        timer = undefined;
      }
    },
  };
})();

// ---------------------------------------------------------------------------
// Validation and sanitized snapshots
// ---------------------------------------------------------------------------

function validateConfig(raw: unknown): CardConfig {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error("Invalid card configuration.");
  }
  const c = raw as Record<string, unknown>;
  for (const key of FORBIDDEN_KEYS) {
    if (key in c) {
      throw new Error(
        `Remove "${key}" from this card's config. Codes belong in HouseParty, ` +
          "never in dashboard config, which everyone who can see the dashboard can read.",
      );
    }
  }
  for (const key of RETIRED_KEYS) {
    if (key in c) {
      throw new Error(`Remove "${key}": sensors and delays are now set in HouseParty's own settings.`);
    }
  }
  if (typeof c.entity !== "string" || !ENTITY_RE.test(c.entity)) {
    throw new Error("Set `entity` to a HouseParty alarm, e.g. alarm_control_panel.house.");
  }
  if (c.animations !== undefined && (typeof c.animations !== "string" || !ANIMATION_LEVELS.has(c.animations))) {
    throw new Error("Set `animations` to auto, full, subtle or off.");
  }
  for (const key of ["privacy", "show_entry_countdown", "haptics"] as const) {
    if (c[key] !== undefined && typeof c[key] !== "boolean") {
      throw new Error(`Set \`${key}\` to true or false.`);
    }
  }
  if (c.name !== undefined && (typeof c.name !== "string" || c.name.length > 100)) {
    throw new Error("Set `name` to text of 100 characters or fewer.");
  }
  const idle = c.idle === undefined ? undefined : validateIdle(c.idle);
  return {
    type: String(c.type ?? ""),
    entity: c.entity,
    name: c.name as string | undefined,
    privacy: c.privacy as boolean | undefined,
    show_entry_countdown: c.show_entry_countdown as boolean | undefined,
    idle,
    motion: resolveMotion((c.animations as string | undefined) ?? "auto"),
    haptics: c.haptics as boolean | undefined,
  };
}

function validateIdle(raw: unknown): CardConfig["idle"] {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error("Set `idle` to a section with a `media` list.");
  }
  const i = raw as Record<string, unknown>;
  for (const key of Object.keys(i)) {
    if (!IDLE_KEYS.has(key)) throw new Error(`Unknown \`idle\` setting "${key}". Use media, after and rotate.`);
  }
  if (
    !Array.isArray(i.media) ||
    i.media.length === 0 ||
    i.media.length > IDLE_MAX_MEDIA ||
    !i.media.every((m) => typeof m === "string" && IDLE_MEDIA_RE.test(m))
  ) {
    throw new Error(
      `Set \`idle.media\` to 1–${IDLE_MAX_MEDIA} files under /local/ (gif, webp, png, jpg, mp4 or webm). ` +
        "Remote URLs aren't allowed on an alarm panel.",
    );
  }
  const seconds = (key: "after" | "rotate", lo: number, hi: number) => {
    const v = i[key];
    if (v === undefined) return undefined;
    if (typeof v !== "number" || !Number.isInteger(v) || v < lo || v > hi) {
      throw new Error(`Set \`idle.${key}\` to whole seconds between ${lo} and ${hi}.`);
    }
    return v;
  };
  return { media: [...(i.media as string[])], after: seconds("after", 30, 3600), rotate: seconds("rotate", 10, 3600) };
}

/** Strings are capped; anything of the wrong type becomes null (fail closed). */
const text = (v: unknown, max = MAX_TEXT): string | null => (typeof v === "string" ? v.slice(0, max) : null);
const u32 = (v: unknown): number | null =>
  typeof v === "number" && Number.isInteger(v) && v >= 0 && v <= 0xffffffff ? v : null;

const nameOf = (hass: Hass, id: string): string =>
  text(hass.states[id]?.attributes?.friendly_name) ?? id;

/** True only for a HouseParty alarm speaking the API version this card knows. */
const isHouseParty = (st: HassEntity | undefined): boolean => st?.attributes?.houseparty_api === HP_API;

const ref = (hass: Hass, id: unknown, state?: unknown) =>
  typeof id === "string" && SENSOR_RE.test(id)
    ? { entity_id: id, name: nameOf(hass, id), state: text(state, MAX_STATE) ?? "" }
    : null;

/** HouseParty's attributes, sanitized. */
function housePartySnap(hass: Hass, a: Record<string, unknown>) {
  const open = a.open_sensors && typeof a.open_sensors === "object" && !Array.isArray(a.open_sensors)
    ? Object.entries(a.open_sensors).slice(0, MAX_SENSORS).map(([id, st]) => ref(hass, id, st))
    : [];
  const bypassed = Array.isArray(a.bypassed_sensors)
    ? a.bypassed_sensors.slice(0, MAX_SENSORS).map((id) => ref(hass, id))
    : [];
  return {
    open_sensors: open.filter((x) => x !== null),
    bypassed: bypassed.filter((x) => x !== null),
    triggered_by: ref(hass, a.triggered_by),
  };
}

/** Copy only the fields the card uses, in the exact shapes Rust expects. */
function snapAlarm(hass: Hass, id: string, st: HassEntity | undefined) {
  if (!st) return null;
  const a = st.attributes && typeof st.attributes === "object" ? st.attributes : {};
  const ms = (v: unknown) => {
    const t = Date.parse(String(v));
    return Number.isFinite(t) ? t : 0;
  };
  const delay = u32(a.delay);
  return {
    entity_id: id,
    state: text(st.state, MAX_STATE) ?? "unknown",
    last_changed_ms: ms(st.last_changed),
    last_updated_ms: ms(st.last_updated ?? st.last_changed),
    friendly_name: text(a.friendly_name),
    code_format: text(a.code_format, 32),
    code_arm_required: typeof a.code_arm_required === "boolean" ? a.code_arm_required : null,
    // A bad value means no arm modes offered, never a crash.
    supported_features: u32(a.supported_features),
    changed_by: text(a.changed_by),
    arm_mode: text(a.arm_mode, 32),
    delay: delay !== null && delay <= MAX_DELAY_S ? delay : null,
    houseparty: isHouseParty(st) ? housePartySnap(hass, a) : null,
  };
}

function errorText(err: unknown): string {
  const msg =
    err && typeof err === "object" && "message" in err ? String((err as { message: unknown }).message) : "";
  return msg.slice(0, 200) || "Home Assistant rejected the command.";
}

// ---------------------------------------------------------------------------
// Fallback: plain DOM, status and Disarm only. Used when WASM can't load or
// has trapped. It deliberately can't arm: arming needs the pre-arm checks.
// ---------------------------------------------------------------------------

class FallbackView {
  #status = document.createElement("p");
  #detail = document.createElement("p");
  #code = document.createElement("input");
  #disarm = document.createElement("button");
  #message = document.createElement("p");
  #idleTimer: number | undefined;
  #send: (service: string, code?: string) => Promise<unknown>;

  constructor(root: ShadowRoot, reason: string, send: (service: string, code?: string) => Promise<unknown>) {
    this.#send = send;
    const card = document.createElement("ha-card");
    const box = document.createElement("div");
    box.style.cssText = "padding:16px;display:grid;gap:10px";

    const note = document.createElement("p");
    note.textContent = `Basic mode: ${reason} Status and Disarm only.`;
    note.style.cssText = "margin:0;font-size:.8rem;color:var(--secondary-text-color)";

    this.#status.style.cssText = "margin:0;font-size:1.3rem;font-weight:600";
    this.#detail.style.cssText = "margin:0;color:var(--secondary-text-color)";

    // Discourage password managers from saving or filling the alarm code.
    const code = this.#code;
    code.type = "password";
    code.autocomplete = "one-time-code";
    code.placeholder = "Code";
    code.setAttribute("aria-label", "Alarm code");
    for (const attr of ["data-1p-ignore", "data-lpignore", "data-bwignore"]) code.setAttribute(attr, "true");
    code.setAttribute("data-form-type", "other");
    code.style.cssText = "min-height:44px;font:inherit;padding:0 10px";
    // Don't leave a typed code behind for the next person.
    code.addEventListener("input", () => this.#armIdleClear());

    this.#disarm.textContent = "Disarm";
    this.#disarm.style.cssText = "min-height:48px;font:inherit";
    this.#disarm.addEventListener("click", () => this.#run("alarm_disarm"));

    this.#message.setAttribute("role", "alert");
    this.#message.style.cssText = "margin:0;color:var(--error-color,#db4437)";

    box.append(note, this.#status, this.#detail, code, this.#disarm, this.#message);
    card.append(box);
    root.replaceChildren(card);
  }

  render(hass: Hass, config: CardConfig, link: Link): void {
    const st = hass.states[config.entity];
    const state = st ? (text(st.state, MAX_STATE) ?? "") : "";
    const live = link === "live" && !!st;

    if (link === "lost") {
      this.#status.textContent = "Connection lost";
      this.#detail.textContent = "The alarm state is unknown until Home Assistant reconnects.";
    } else if (link === "stale") {
      this.#status.textContent = "Checking connection…";
      this.#detail.textContent = "Controls come back once Home Assistant answers.";
    } else if (!st) {
      this.#status.textContent = "Alarm entity not found";
      this.#detail.textContent = config.entity;
    } else if (!isHouseParty(st)) {
      this.#status.textContent = "Not a HouseParty alarm";
      this.#detail.textContent = config.entity;
    } else {
      this.#status.textContent = state.replace(/_/g, " ");
      this.#detail.textContent = text(st.attributes?.friendly_name) ?? config.entity;
    }

    const canDisarm = live && isHouseParty(st) && /^(armed_|arming|pending|triggered)/.test(state);
    this.#code.hidden = !canDisarm || typeof st?.attributes?.code_format !== "string";
    this.#disarm.hidden = !canDisarm;
    if (!canDisarm) this.reset();
  }

  /** Clear any typed code (idle, hidden, or no longer applicable). */
  reset(): void {
    this.#code.value = "";
    window.clearTimeout(this.#idleTimer);
    this.#idleTimer = undefined;
  }

  #armIdleClear(): void {
    window.clearTimeout(this.#idleTimer);
    this.#idleTimer = window.setTimeout(() => this.reset(), IDLE_CLEAR_MS);
  }

  #run(service: string): void {
    const code = this.#code.value;
    this.reset();
    this.#message.textContent = "";
    try {
      // HouseParty checks the code and enforces the lockout; its message
      // (wrong code, locked out) comes back as the rejection.
      this.#send(service, code || undefined).catch((err) => {
        this.#message.textContent = errorText(err);
      });
    } catch (err) {
      this.#message.textContent = errorText(err);
    }
  }
}

// ---------------------------------------------------------------------------
// The custom element
// ---------------------------------------------------------------------------

class RustAlarmCard extends HTMLElement {
  // Closed: page scripts can't reach in through element.shadowRoot.
  #shadow = this.attachShadow({ mode: "closed" });
  #config?: CardConfig;
  #hass?: Hass;
  #card?: AlarmCard;
  #fallback?: FallbackView;
  #started = false;
  #startGeneration = 0;
  #seen: HassEntity | undefined;
  #lastLink?: Link;
  #shown = false;
  #unsubscribe?: () => void;
  #onScreen = true;
  #pageVisible = !document.hidden;
  #stale = false;
  #ticker?: number;
  #io?: IntersectionObserver;

  static getStubConfig(hass: Hass) {
    const entity = Object.keys(hass.states).find((id) => id.startsWith(`${DOMAIN}.`));
    const hp = Object.keys(hass.states).find((id) => id.startsWith(`${DOMAIN}.`) && isHouseParty(hass.states[id]));
    return { entity: hp ?? entity ?? `${DOMAIN}.house` };
  }

  getCardSize(): number {
    return 6;
  }

  setConfig(config: unknown): void {
    this.#config = validateConfig(config); // throws: HA shows its own error card
    this.#seen = undefined;
    if (this.#card) this.#guard(() => this.#card!.set_config(this.#config));
    if (this.isConnected) void this.#start();
    this.#push(true);
  }

  set hass(hass: Hass) {
    this.#hass = hass;
    heartbeat.setConnection(hass.connection);
    this.#push(false);
  }

  connectedCallback(): void {
    if (this.#config) void this.#start();
    this.#unsubscribe = heartbeat.subscribe(() => this.#push(heartbeat.isOk()));
    this.#io = new IntersectionObserver((entries) => {
      this.#onScreen = entries.some((e) => e.isIntersecting);
      this.#visibilityChanged();
    });
    this.#io.observe(this);
    document.addEventListener("visibilitychange", this.#onPageVisibility);
    this.#visibilityChanged();
  }

  disconnectedCallback(): void {
    this.#unsubscribe?.();
    this.#unsubscribe = undefined;
    this.#io?.disconnect();
    this.#io = undefined;
    document.removeEventListener("visibilitychange", this.#onPageVisibility);
    this.#stopTimers();
    this.#fallback?.reset();

    // A disconnected element may be inserted again later. Fully dispose the
    // Leptos owner and instance registry now, then allow a fresh mount.
    const card = this.#card;
    this.#card = undefined;
    if (card) {
      try {
        card.destroy();
      } catch {
        // The DOM is still removed below; a trapped WASM instance is already
        // globally excluded from further use by #guard when applicable.
      }
    }
    this.#fallback = undefined;
    this.#started = false;
    this.#startGeneration += 1;
    this.#seen = undefined;
    this.#lastLink = undefined;
    this.#shadow.replaceChildren();
  }

  // ---- lifecycle ----

  async #start(): Promise<void> {
    if (this.#started) return;
    this.#started = true;
    const generation = ++this.#startGeneration;
    this.#renderShell("Loading alarm…");
    try {
      if (wasmCrashed) throw new Error("the alarm card crashed earlier in this session.");
      await loadWasm();
      // A slow WASM load must not mount twice after disconnect/reconnect.
      if (generation !== this.#startGeneration || !this.isConnected) return;
      const mount = this.#renderShell();
      this.#card = new AlarmCard(mount, (service: unknown, data: unknown) => this.#fromWasm(service, data));
      this.#card.set_config(this.#config);
      this.#lastLink = undefined;
      this.#seen = undefined;
      this.#push(true);
      this.#visibilityChanged();
    } catch (err) {
      if (generation === this.#startGeneration) this.#fail(err);
    }
  }

  /** The placeholder never shows an alarm state, only that we're loading. */
  #renderShell(placeholder?: string): HTMLElement {
    const haCard = document.createElement("ha-card");
    const mount = document.createElement("div");
    if (placeholder) {
      const p = document.createElement("p");
      p.textContent = placeholder;
      p.style.cssText = "margin:0;padding:16px;color:var(--secondary-text-color)";
      mount.append(p);
    }
    haCard.append(mount);
    this.#shadow.replaceChildren(haCard);
    return mount;
  }

  /**
   * Calls into Rust. `update` never throws by design, so anything thrown
   * here is a real fault (a trap, or a bug), and we stop trusting the module.
   */
  #guard(fn: () => void): void {
    try {
      fn();
    } catch (err) {
      if (err instanceof WebAssembly.RuntimeError) wasmCrashed = true;
      this.#fail(err);
    }
  }

  #fail(err: unknown): void {
    console.error(`[${TAG}] switching to basic mode:`, err);
    this.#stopTimers();
    const card = this.#card;
    this.#card = undefined;
    // Never call back into a module that has trapped.
    if (card && !wasmCrashed) {
      try {
        card.destroy();
      } catch {
        /* already broken */
      }
    }
    const reason =
      err instanceof InsecureContextError
        ? "the full card needs Home Assistant served over HTTPS."
        : "the full card didn't load.";
    this.#fallback ??= new FallbackView(this.#shadow, reason, (service, code) => this.#send(service, code));
    this.#lastLink = undefined;
    this.#push(true);
    this.#visibilityChanged();
  }

  // ---- service calls: the only path to HA ----

  /** Called synchronously from Rust. Throws on anything not allowed. */
  #fromWasm(service: unknown, data: unknown): void {
    const d = data && typeof data === "object" ? (data as { code?: unknown; force?: unknown; bypass?: unknown }) : {};
    const code = typeof d.code === "string" ? d.code : undefined;
    const bypass = d.force === true ? d.bypass : undefined;
    this.#send(service, code, bypass).catch((err) => this.#guard(() => this.#card?.report_error(errorText(err))));
  }

  /**
   * The only path to HA. `bypass` (an array of approved sensor ids) makes it
   * a force-arm; HouseParty refuses if anything else is blocking.
   */
  #send(service: unknown, code?: string, bypass?: unknown): Promise<unknown> {
    if (!globalThis.isSecureContext) {
      throw new Error("Alarm controls require HTTPS. Status remains available in basic mode.");
    }
    if (typeof service !== "string" || !ALLOWED_SERVICES.has(service)) {
      throw new Error(`Blocked service call: ${String(service)}`);
    }
    const hass = this.#hass;
    const config = this.#config;
    if (!hass || !config) throw new Error("Home Assistant isn't ready yet.");
    if (!isHouseParty(hass.states[config.entity])) throw new Error("This isn't a HouseParty alarm.");
    if (this.#link() !== "live") throw new Error("Not sent: the connection isn't confirmed.");

    if (bypass !== undefined) {
      const mode = HP_MODES[service];
      if (!mode) throw new Error("Only arming can be forced.");
      if (!Array.isArray(bypass) || bypass.length > MAX_SENSORS || !bypass.every((id) => typeof id === "string" && SENSOR_RE.test(id))) {
        throw new Error("Invalid bypass list.");
      }
      const payload: Record<string, unknown> = { entity_id: config.entity, mode, force: true, bypass };
      if (code) payload.code = code;
      return hass.callService("houseparty", "arm", payload);
    }
    const payload: Record<string, unknown> = { entity_id: config.entity };
    if (code) payload.code = code;
    return hass.callService(DOMAIN, service, payload);
  }

  // ---- connection: never show unproven data as live ----

  #link(): Link {
    const hass = this.#hass;
    if (!hass || hass.connected === false) return "lost";
    // No ping available (older frontend): trust the connected flag.
    if (!heartbeat.canPing()) return "live";
    return heartbeat.isOk() ? "live" : "stale";
  }

  // ---- state updates ----

  #push(force: boolean): void {
    const hass = this.#hass;
    const config = this.#config;
    if (!hass || !config) return;
    const link = this.#link();

    if (this.#fallback) {
      this.#fallback.render(hass, config, link);
      return;
    }
    if (!this.#card) return;

    // Link changes always go through, even off-screen: a stale "Disarmed"
    // costs more than one wasted update.
    if (link !== this.#lastLink) {
      this.#lastLink = link;
      this.#guard(() => this.#card!.set_link(link));
      if (!this.#card) return;
    }

    if (!force && !this.#visible) {
      this.#stale = true;
      return;
    }

    // HA replaces an entity's state object whenever it changes, so an
    // identity check is exact and nearly free.
    const st = hass.states[config.entity];
    if (!force && this.#seen === st) return;
    this.#seen = st;
    this.#stale = false;

    const snapshot = { alarm: snapAlarm(hass, config.entity, hass.states[config.entity]), now_ms: Date.now() };
    this.#guard(() => this.#card!.update(snapshot));
  }

  // ---- visibility: pause work nobody can see, re-prove the link on wake ----

  #onPageVisibility = (): void => {
    this.#pageVisible = !document.hidden;
    this.#visibilityChanged();
  };

  get #visible(): boolean {
    return this.#onScreen && this.#pageVisible && this.isConnected;
  }

  #visibilityChanged(): void {
    if (this.#visible) {
      if (!this.#shown) {
        this.#shown = true;
        heartbeat.show(this); // a wake makes the link unproven until answered
      }
      if (this.#stale) this.#push(true);
      if (this.#ticker === undefined && this.#card) {
        this.#ticker = window.setInterval(() => this.#guard(() => this.#card?.tick(Date.now())), 1000);
      }
    } else {
      this.#stopTimers();
      // Screen off, tab hidden, or scrolled away: drop any half-entered code.
      this.#guard(() => this.#card?.reset_flow());
      this.#fallback?.reset();
    }
  }

  #stopTimers(): void {
    window.clearInterval(this.#ticker);
    this.#ticker = undefined;
    if (this.#shown) {
      this.#shown = false;
      heartbeat.hide(this);
    }
  }
}

if (!customElements.get(TAG)) customElements.define(TAG, RustAlarmCard);

const w = window as unknown as { customCards?: Array<Record<string, unknown>> };
w.customCards ??= [];
if (!w.customCards.some((c) => c.type === TAG)) {
  w.customCards.push({
    type: TAG,
    name: "HouseParty",
    description: "Front end for the HouseParty alarm.",
    preview: false,
  });
}
