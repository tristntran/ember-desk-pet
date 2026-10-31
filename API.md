# Ember API

The Ember server serves Ember on your local network and exposes a small JSON API, so people and scripts on the LAN can check on Ember, interact with it, and control the house through it.

Base URL: `http://<pi-address>:8080`. It answers devices on your local network only, and refuses commands sent from other websites' pages (see Access).

## How it works

- The Pi stores Ember's state in `ember_state.json`, which survives restarts and browser resets.
- One screen **runs Ember** at a time. That's whichever screen checked in first. It holds a 10-second lease and renews it every 1.5 seconds. Any other screens **mirror** it.
- **Pet actions are queued:** they're added to a queue, and the screen running Ember carries them out. That way you see the animation, plus a note like "Kat fed Ember". Each request returns an id you can check. Actions expire after 2 minutes if no screen is on.
- **Device actions go straight to Home Assistant** when it's set up in `ember.json`, so they work even with every screen off. The reply is `{"ok":true}` instead of a queue id.

## Reading

| Method | Path | Returns |
|---|---|---|
| GET | `/api` | What's available: pet actions, toys, controllable device domains |
| GET | `/api/ping` | `{ok, screen_online}` |
| GET | `/api/state` | `pet` (name, mood, stats, friendship, asleep, activity, room, misbehaving, fires, session) and `devices` |
| GET | `/api/actions/<id>` | A queued action: `status` is `pending`, `running`, `done` or `expired`, plus a `result` of `{ok, msg}` |

## Interacting with Ember

`POST /api/pet/<action>` with an optional JSON body. Add `"by": "Kat"` to have the screen credit you.

| Action | Body | Notes |
|---|---|---|
| `feed` | `{"food":"log"\|"pinecone"\|"pepper"}` | Refused if Ember is full or busy. A **pepper** sends Ember into a wild frenzy |
| `play` | `{"toy":"ball"\|"bubbles"\|"music"\|"feather"}` | Refused if Ember is hungry, sleepy or misbehaving |
| `pet` | | Hearts. Doesn't work while Ember is sulking |
| `tidy` | | Ember hates it. Refused while a fire is burning |
| `sleep` / `wake` | | `wake` is refused during a focus session |
| `spray` | | Puts out the biggest fire, or squirts Ember if it's misbehaving |
| `mischief` | | Makes Ember misbehave now, if mischief is turned on |

`GET /api/state` also shows `pet.grudge` (0–100) and `pet.chaos`, how close Ember is to its next misbehaving, in %.

## Karaoke queue

The Pi keeps the queue itself, so it works even when no screen is on. Every screen and phone sees the same list.

| Method | Path | Body | |
|---|---|---|---|
| GET | `/api/karaoke` | | `{current, queue}` |
| POST | `/api/karaoke/join` | `{"name":"Kat","song":"optional"}` | Returns your `position` |
| POST | `/api/karaoke/next` | | Moves the next person in line to the mic |
| POST | `/api/karaoke/remove` | `{"id":"…"}` | |
| POST | `/api/karaoke/clear` | | |

`GET /api` also returns `remote`, the remote's address on your network. The karaoke screen turns it into a QR code.

## Controlling the house

`POST /api/devices/<entity_id>/<service>`. Only devices listed in `ember.json` can be controlled.

| Service | Body | For |
|---|---|---|
| `toggle`, `turn_on`, `turn_off` | `{"brightness_pct": 40}` (lights only) | everything. Apple TV power goes through its remote (wake/sleep) |
| `color` | `{"rgb_color":[255,80,0]}` or `{"color_temp_kelvin":2700}` | Govee lights |
| `set_temperature` | `{"temperature": 72}` | the AC |
| `set_hvac_mode` | `{"hvac_mode":"cool"\|"dry"\|"fan_only"\|"auto"\|"off"}` | the AC |
| `set_fan_mode` | `{"fan_mode":"low"}` | the AC |
| `play_pause`, `play`, `pause`, `next`, `previous` | | Apple TV |
| `volume_up`, `volume_down`, `mute` | | speakers, projector (IR) |
| `volume_set` | `{"volume_level": 0.4}` (0–1) | players that support it |
| `select_source` | `{"source":"Netflix"}` | Apple TV apps, projector or speaker inputs |
| `press` | | IR buttons (`button.*`) |

The server only allows the domains in `EMBER_API_DOMAINS`, which defaults to `light,climate,media_player,button,switch`. Locks and alarms can't be added through it.

### For the screens
| Method | Path | |
|---|---|---|
| GET | `/api/config` | `{ha:{configured,status}, devices, ember, trains}`: your device list and room mapping from `ember.json` (never the token) |
| GET | `/api/ha/states` | Live state of every device in `ember.json` |
| POST | `/api/ha/call` | `{"domain":"light","service":"turn_on","data":{"entity_id":"light.govee_kitchen"}}`. Only the domains light, climate, media_player, remote, button, switch, fan, scene, script, input_boolean, and only entities in `ember.json` |

## Examples

```bash
curl http://ember.local:8080/api/state
curl -X POST http://ember.local:8080/api/pet/feed -d '{"by":"Kat"}'
curl -X POST http://ember.local:8080/api/pet/play -d '{"toy":"bubbles","by":"Kat"}'
curl -X POST http://ember.local:8080/api/devices/light.govee_kitchen/toggle
curl -X POST http://ember.local:8080/api/devices/light.govee_living_room/color -d '{"rgb_color":[120,60,255]}'
curl -X POST http://ember.local:8080/api/devices/media_player.living_room_apple_tv/turn_on
curl -X POST http://ember.local:8080/api/devices/media_player.projector/select_source -d '{"source":"HDMI 2"}'
curl -X POST http://ember.local:8080/api/devices/climate.lg_air_conditioner/set_hvac_mode -d '{"hvac_mode":"cool"}'
curl -X POST http://ember.local:8080/api/devices/button.projector_input/press
curl http://ember.local:8080/api/actions/3f9c1a2b7d        # did it work?
```

There's also a phone-friendly remote at **`/remote`**.

## Access

Built in, always on:
- **Local network only:** requests from public internet addresses are refused.
- **No DNS rebinding:** the `Host` must be an IP address, `localhost`, a bare name like `ember`, or a local name (`.local`, `.lan`, `.home.arpa`, `.internal`, `.home`). Add other names you use with `EMBER_HOSTNAMES=ember.mydomain.com`.
- **No drive-by commands:** a `POST` whose `Origin` is another site is refused, so a web page someone opens on your Wi-Fi can't control the house. Scripts and `curl` (no `Origin`) work normally.
- **The token stays on the Pi:** it lives in `ember.json`, which is never served, and is only ever sent to a local address.
- **No internet except the train board:** the MTA feed is the server's only internet traffic; `"trains": false` in `ember.json` turns that off.

Optional:
- **Token:** set `EMBER_API_TOKEN=something` and every `POST` must send the header `X-Ember-Token: something`. (The screens and phone remote don't send it yet, so only use this for script-only setups.)
- **This machine only:** set `EMBER_HOST=127.0.0.1`.
- **Firewall:** DNS rules don't stop someone from using the Pi's IP address directly. A firewall rule or a separate Wi-Fi network does.
