# Ember API

`ember_server.py` serves Ember on your local network and exposes a small JSON API, so people and scripts on the LAN can check on Ember, interact with it, and control the house through it.

Base URL: `http://<pi-address>:8080`. CORS is open, so any web page on your network can call it.

## How it works

- The Pi stores Ember's state in `ember_state.json`, which survives restarts and browser resets.
- One screen **runs Ember** at a time. That's whichever screen checked in first. It holds a 10-second lease and renews it every 1.5 seconds. Any other screens **mirror** it.
- **Actions are queued:** `POST` requests are added to a queue, and the screen running Ember carries them out. That way you see the animation, plus a note like "Kat fed Ember". Each request returns an id you can check. Actions expire after 2 minutes if no screen is on.

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
| `feed` | | Refused if Ember is full or busy |
| `play` | `{"toy":"ball"\|"bubbles"\|"music"\|"feather"}` | Refused if Ember is hungry, sleepy or misbehaving |
| `pet` | | Hearts. Doesn't work while Ember is sulking |
| `tidy` | | Ember hates it. Refused while a fire is burning |
| `sleep` / `wake` | | `wake` is refused during a focus session |
| `spray` | | Puts out the biggest fire, or squirts Ember if it's misbehaving |
| `mischief` | | Makes Ember misbehave now, if mischief is turned on |

## Controlling the house

`POST /api/devices/<entity_id>/<service>`

| Service | Body | For |
|---|---|---|
| `toggle`, `turn_on`, `turn_off` | `{"brightness_pct": 40}` (lights only) | lights, the AC, music |
| `set_temperature` | `{"temperature": 72}` | the AC |
| `play_pause`, `play`, `pause`, `next`, `previous` | | music |
| `volume_set` | `{"volume_level": 0.4}` (0–1) | music |

The server only allows the domains in `EMBER_API_DOMAINS`, which defaults to `light,climate,media_player`.

## Examples

```bash
curl http://ember.local:8080/api/state
curl -X POST http://ember.local:8080/api/pet/feed -d '{"by":"Kat"}'
curl -X POST http://ember.local:8080/api/pet/play -d '{"toy":"bubbles","by":"Kat"}'
curl -X POST http://ember.local:8080/api/devices/light.kitchen/toggle
curl -X POST http://ember.local:8080/api/devices/light.living_room/turn_on -d '{"brightness_pct":30}'
curl -X POST http://ember.local:8080/api/devices/media_player.living_room/play_pause
curl -X POST http://ember.local:8080/api/devices/climate.ac/set_temperature -d '{"temperature":71}'
curl http://ember.local:8080/api/actions/3f9c1a2b7d        # did it work?
```

There's also a phone-friendly remote at **`/remote`**.

## Access

The API is open to anything that can reach the Pi's port. To restrict it:
- **Firewall or network:** block the port with a firewall or put the Pi on a separate network. Note that DNS rules alone don't stop someone from using the Pi's IP address directly.
- **Token:** set `EMBER_API_TOKEN=something` and every `POST` must send the header `X-Ember-Token: something`.
- **This machine only:** set `EMBER_HOST=127.0.0.1`.

The screens load `config.local.js` from the server, so **anything in it, including a Home Assistant token, can be read by anyone who can reach the Pi.** Use a separate Home Assistant user with limited permissions for Ember's token.
