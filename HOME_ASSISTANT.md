# Connecting Ember to Home Assistant

```
 screens / phones ──LAN──▶  Ember server (Pi)  ──LAN──▶  Home Assistant  ──▶ Govee lights (LAN)
                            holds the token               (separate box)      Apple TV (LAN)
                            only lets through the                             IR blaster ─▶ projector, speakers
                            devices in ember.json                             LG ThinQ ─▶ AC (cloud, see below)
```

Screens and phones only ever talk to the Ember server. The Ember server is the only thing holding the Home Assistant token.

## 1. Where Home Assistant runs
Use a **separate box**: a Raspberry Pi 4/5 or a mini PC running Home Assistant OS. The Pi Zero 2 W behind the screen is too small for Home Assistant, so it only runs the screen and the Ember server.

## 2. Your devices

| Device | Home Assistant integration | Local? | Entity you get |
|---|---|---|---|
| Govee lights | **Govee Light Local** (built in) | yes | `light.*` with brightness and color |
| Apple TV | **Apple TV** (built in) | yes | `media_player.*` and `remote.*` |
| Projector | **HAIR** + IR blaster | yes | `media_player.*` and `button.*` per learned command |
| Edifier R1700BT speakers | **HAIR** + IR blaster | yes | `media_player.*` (power, volume, mute) and `button.*` |
| LG air conditioner | **LG ThinQ** (built in) *or* HAIR | ThinQ: **cloud**. HAIR: yes | `climate.*` |

**Govee:** in the Govee Home app, open each light → settings → turn on **LAN Control**. Then add *Govee Light Local* in Home Assistant; it finds them automatically.

**Apple TV:** Settings → Devices & services → Add → *Apple TV*. Enter the PIN shown on the TV. Home Assistant can't really power an Apple TV off through the media player; Ember handles that by sending `wakeup`/`suspend` through the `remote.*` entity, which is why that device has `"remote"` in `ember.json`.

**IR blaster + HAIR** (for the projector and speakers):
1. Use an IR blaster that Home Assistant supports, either an ESPHome IR transmitter/receiver module or a Broadlink RM4 mini / RM4 pro.
2. Install **HACS**, then install **HAIR** from HACS. HAIR needs Home Assistant 2026.4 or newer.
3. In HAIR, add a device per thing: *Projector* as a media player, *Speakers* as a media player.
4. Learn each remote button by pointing the original remote at the blaster: power, volume up/down, mute, input/source.
5. HAIR creates the `media_player.*` entity and one `button.*` per learned command. Put the button ids into `controls` in `ember.json` (for example the projector's Input button).

IR is one-way, so Home Assistant guesses whether the projector is on. If it gets out of sync, press power once on the screen.

**LG AC:** the built-in *LG ThinQ* integration works through LG's cloud and needs a personal access token from LG's ThinQ developer site. Ember itself stays off the internet either way, but Home Assistant will talk to LG. To keep everything local, learn the AC's IR remote in HAIR as a *climate* device instead. Most LG window and split ACs come with an IR remote.

## 3. A token for Ember
1. In Home Assistant, make a new **non-admin** user called `ember`.
2. Log in as that user → Profile → Security → **Long-lived access tokens** → Create.
3. Copy it once; it's shown only once.

Home Assistant tokens can't be limited to certain devices, so the Ember server does that itself: it only passes on actions for the devices listed in `ember.json`, never locks, alarms or anything else.

## 4. ember.json (on the Pi)
```bash
cp ember.example.json ember.json && chmod 600 ember.json && nano ember.json
```
- `home_assistant.url`: e.g. `http://192.168.1.20:8123`. It must be a local address; the server refuses to send the token anywhere else. Using the IP is most reliable; `homeassistant.local` also works if the Pi resolves `.local` names.
- `home_assistant.token`: the token from step 3.
- `devices`: one entry per device. Change the `entity_id`s to the ones in Settings → Entities.
  - `room`: which Home-panel tab it shows on (`Living`, `Kitchen`, `Bedroom`, `Outside`).
  - `remote` (Apple TV only): its `remote.*` entity.
  - `controls`: extra buttons on its tile, e.g. `{"label":"Input","service":"button.press","entity":"button.projector_input"}`.
- `ember.rooms.<room>.props`: which device each thing in the room controls (the hearth lamp, the bedroom fan, the porch light…). `quick` sets the two bottom-bar shortcuts.
- `ember.scenes`: the four scene buttons. Point them at your `scene.*` or `script.*` entities.
- `trains`: `false` turns off the subway board and the server's only internet use.

Restart the server (`sudo systemctl restart ember`). The Home panel shows **Home Assistant** in green when it's connected. If it says *HA offline*, the reason is at `http://<pi>:8080/api/config`.

## Troubleshooting
| Symptom | Fix |
|---|---|
| `the token was rejected (401)` | Make a new token; check you pasted all of it |
| `isn't on your local network` | `home_assistant.url` points outside your LAN. Use the Home Assistant box's local IP |
| A device shows *Off* and won't respond | Its `entity_id` in `ember.json` doesn't match Home Assistant's |
| Projector or speakers out of sync | IR can't report state. Press power on the tile once to resync |
| `isn't in ember.json` | Add the entity to `devices` (or to a `controls` entry) |
