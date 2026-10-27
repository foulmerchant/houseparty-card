# HouseParty card

> **DO NOT USE — IN TESTING.** Experimental code only. Do not rely on this card for safety, security, or property protection.

A Rust/WebAssembly alarm keypad for the HouseParty Home Assistant integration. It includes accessible motion, reduced-motion behavior, connection monitoring, privacy controls, exact force-arm confirmation, and a local-media idle screensaver.

> **HA test candidate:** the release build, 30 Rust tests, strict Clippy checks, TypeScript checks, and npm audit pass. Real Home Assistant runtime testing is still required. Do not rely on it as your only alarm interface.

## Requirements

- The separate **HouseParty** integration installed and configured.
- Home Assistant served over **HTTPS** for alarm controls. Insecure contexts are deliberately status-only.
- A modern browser with WebAssembly support.

## Install with HACS

Add this repository as a custom HACS repository in the **Dashboard** category, then download **HouseParty card**. HACS downloads both files in `dist/`: the JavaScript entry point and its WASM binary.

Add the card to a dashboard:

```yaml
type: custom:houseparty-card
entity: alarm_control_panel.house
privacy: true
show_entry_countdown: false
animations: auto
haptics: true
```

If HACS does not register the resource automatically, add `/hacsfiles/houseparty-card/houseparty-card.js` as a **JavaScript module** dashboard resource.

## Manual install

Copy both `dist/houseparty-card.js` and `dist/houseparty-card_bg.wasm` to `config/www/houseparty/`, preserving both names. Add `/local/houseparty/houseparty-card.js` as a JavaScript module resource.

## Idle screensaver

Only local media is accepted. Example:

```yaml
idle:
  media:
    - /local/houseparty/idle/otter.mp4
  after: 120
  rotate: 60
```

Prefer hardware-decoded MP4/WebM over animated GIF. Do not place private photos in `config/www`.

## Security notes

- Codes are sent only through Home Assistant services to the configured HouseParty entity.
- The WASM binary is SHA-256 checked before compilation.
- Controls require HTTPS; HTTP status views cannot submit codes.
- State celebrations follow confirmed HA state, never button presses.
- Frontend code shares the browser page with other plugins, so keep wall-tablet dashboards minimal and trusted.

## Build

The exact Rust toolchain and dependency lockfiles are included.

```sh
cargo install wasm-bindgen-cli --version 0.2.129 --locked
./scripts/build.sh
```

The build requires `wasm-opt` for the smallest binary, Node.js 22, and npm. Release checksums are written to `dist/SHA256SUMS`.
