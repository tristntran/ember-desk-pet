# Ember — hearth desk pet

A cozy pixel-art desk pet for a small touchscreen (built for 800×480, e.g. Pi Zero 2 W + HyperPixel 4.0 Touch).
Ember is a little fire spirit that lives in your house, controls your Home Assistant devices, and nudges you when it's time to catch the train.

## Run it
```bash
python3 ember_server.py          # serves the page + relays MTA subway feeds
chromium-browser --kiosk http://localhost:8080/#hearth     # or #bedroom, #porch
```

## Rooms
| Hash | Ember is… | Tappable props |
|---|---|---|
| `#hearth` | an ember on the logs | lamp, lantern, candle, curtains |
| `#bedroom` | a little candle | floor lamp, ceiling fan, fairy lights, curtains |
| `#porch` | the porch lantern | front-door lock, porch-light switch, thermometer (+ subway board) |

## Configure
- Shared settings: `CONFIG` at the top of `index.html` (rooms, entities, trains, focus naps).
- Private per-device settings (HA token etc.): copy `config.local.example.js` → `config.local.js`. It's git-ignored.

## Update a device
```bash
cd ~/ember && git pull      # then refresh the browser (or reboot)
```
