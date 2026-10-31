# Ember — hearth desk pet

A cozy pixel-art desk pet for a small touchscreen (built for 800×480, e.g. Pi Zero 2 W + HyperPixel 4.0 Touch).
Ember is a little fire spirit that lives in your house, controls your Home Assistant devices, and nudges you when it's time to catch the train.

## Run it
```bash
python3 ember_server.py          # serves the screens, stores Ember's state, relays MTA feeds, LAN API
chromium-browser --kiosk http://localhost:8080/#hearth     # or #bedroom, #porch
```
- Other screens on the network: `http://<pi>:8080/#bedroom`. They mirror the Ember on the main screen.
- Phone remote for anyone on your network: `http://<pi>:8080/remote`
- API for scripts and other apps: see [API.md](API.md).

## Rooms
| Hash | Ember is… | Tappable props |
|---|---|---|
| `#hearth` | an ember on the logs | lamp, lantern, candle, radio (music) |
| `#bedroom` | a little candle | floor lamp, ceiling fan (the AC), fairy lights |
| `#porch` | the porch lantern | porch-light switch, thermometer (+ subway board) |

## Configure
- Shared settings: `CONFIG` at the top of `index.html` (rooms, entities, trains, focus naps).
- Private per-device settings (HA token etc.): copy `config.local.example.js` → `config.local.js`. It's git-ignored.

## Update a device
```bash
cd ~/ember && git pull      # then refresh the browser (or reboot)
```
