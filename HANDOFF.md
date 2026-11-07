# Handoff: moving Ember's brain into Rust (work in progress)

Goal: one program owns Ember. Screens and phones are windows that send taps and animate what the brain says.

## Done
- **`core/` (ember-core):** Ember's brain as a pure Rust library with no I/O; the caller passes the time in. It covers stats drift, eating, play (with mini-game "hits" reported by the screen running the game), tidying and grudge, the chaos meter and mischief, fires (ignite, spread, douse, scorch, blaze), sleep and bedtime, focus sessions and breaks, the logbook and ratings, prefs and rename.
  - Loads the old in-page save format, so nothing is lost.
  - `cargo test -p ember-core` runs 13 tests.
  - A WebAssembly entry point (`call(json) -> json`) sits at the bottom of `lib.rs`.
- **`server/`:** runs one Brain in memory.
  - Ticks 4× a second and saves to `ember_state.json` as `{brain, karaoke}`. Older `{full}` saves are migrated.
  - Pushes `{state, events}` to every screen over **`GET /api/stream`** (Server-Sent Events).
  - **`POST /api/act {action}`** for screens; `/api/pet/<action>` for phones and scripts.
  - `/api/brain` returns a snapshot and `/api/log` the logbook (fetch it when `state.logRev` changes).
  - Removed: the lease, the action queue, `/api/actions/*` and `POST /api/state`.
- **`brain.js`:** the brain compiled to WebAssembly and wrapped as one script (`window.EmberBrain.load()`, then `.call({op, now, hour, mday, ...})`). Ops: `init`, `tick`, `act`, `snapshot`, `events`, `log`, `export`. Smoke-tested in Node.

## Not done yet: the page still has its own brain
`index.html` still simulates Ember itself and still uses the old leader/lease sync. With the **new server**, that sync no longer works, because `/api/lease` and `/api/queue/next` are gone. Opened as a file or the artifact link, it works as before.

Next steps:
1. **Connect the page:** if `/api/ping` answers `brain:true`, use `EventSource('/api/stream')` and `POST /api/act`. Otherwise load `brain.js` and run it in-page. To keep tabs in step, one tab runs it and shares `{state, events}` over a BroadcastChannel.
2. **Copy the state:** set the page's `S` from `state` (`stats`, `asleep`, `fires`, `timer`, `grudge`, `fp`…), and map `state.activity.kind` to the local animation state.
3. **Turn events into animations and toasts:** `feed ate play play_end tidy tidy_end mischief_start mischief_end ignite doused squirted blaze sleep wake session_start session_done session_stopped break_start break_done grudge_high pet`. Only process events with `seq` greater than the last one seen.
4. **Swap state changes for actions:** replace every `fx()/addFP()/addGrudge()/S.x=` in the page with an action. Mini-games send `{type:"play_hit", hit:"boop|pop|pop_user|beat|tickle", screen}`, and only the screen that owns the game is counted.
5. **Remove the old sync code:** `LOCAL`, `relay`, `runAction`, `adopt`, `netTick`'s lease and queue, and the in-page decay and chaos.
6. **Update `API.md`** for the endpoints above.

## Building the WebAssembly brain
GitHub Actions is currently blocked (account billing lock), so `brain.yml` won't run until that's fixed. To build locally:
```bash
rustup target add wasm32-unknown-unknown
cargo build --release -p ember-core --lib --target wasm32-unknown-unknown
python3 tools/make_brain_js.py target/wasm32-unknown-unknown/release/ember_core.wasm > brain.js
```
If you can't download the target, put Rust's `library/` source for your exact `rustc` version in `$(rustc --print sysroot)/lib/rustlib/src/rust/library`. Include the `backtrace` submodule. Then run:
`RUSTC_BOOTSTRAP=1 cargo build --release -p ember-core --lib --target wasm32-unknown-unknown -Zbuild-std=std,panic_abort`
