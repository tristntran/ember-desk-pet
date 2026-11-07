# Ember — hearth desk pet

A cozy pixel-art desk pet for a small touchscreen (built for 800×480, e.g. Pi Zero 2 W + HyperPixel 4.0 Touch).
Ember is a little fire spirit that lives in your house, controls your Home Assistant devices, and nudges you when it's time to catch the train.

## Run it
```bash
./ember-server                    # serves the screens, stores Ember's state, Home Assistant bridge, LAN API
chromium-browser --kiosk http://localhost:8080/#hearth     # or #bedroom, #porch
```
- Other screens on the network: `http://<pi>:8080/#bedroom`. They mirror the Ember on the main screen.
- Phone remote for anyone on your network: `http://<pi>:8080/remote`
- API for scripts and other apps: see [API.md](API.md).
- Connecting your devices: see [HOME_ASSISTANT.md](HOME_ASSISTANT.md).
- Opening `index.html` straight from disk still works as a demo with pretend devices.

## The server
A small Rust program in [`server/`](server/) (one ~2 MB file, three dependencies). It only talks to your local network:

- Requests from outside your local network are refused.
- Requests to a hostname that isn't local are refused. This blocks "DNS rebinding", a trick that lets a website reach devices on your Wi-Fi.
- Commands sent by other websites' pages are refused, so a page someone opens on your Wi-Fi can't control the house.
- The Home Assistant token stays on the Pi, and is only ever sent to a local address.
- Its only internet use is the MTA train feed. Set `"trains": false` in `ember.json` and it uses none.

### Get the program
- **Easiest:** download `ember-server-<arch>` from the repo's **Actions → Build server** run (or a Release). Use `aarch64` on 64-bit Raspberry Pi OS and `armv7` on 32-bit.
- **Build it yourself** (on a laptop or Pi 4/5; a Pi Zero 2 W has too little memory to compile it comfortably):
  ```bash
  cd server && cargo build --release                       # for the machine you're on
  cross build --release --target aarch64-unknown-linux-gnu  # for a Pi, from a laptop (cargo install cross)
  ```

### Install on the Pi
```bash
mkdir -p ~/ember && cd ~/ember && git clone https://github.com/tristntran/ember-desk-pet .
cp ~/Downloads/ember-server-aarch64 ./ember-server && chmod +x ember-server
cp ember.example.json ember.json   # fill in Home Assistant (see HOME_ASSISTANT.md)
chmod 600 ember.json
sudo cp server/ember.service /etc/systemd/system/ && sudo systemctl enable --now ember
```

## Rooms
| Hash | Ember is… | Tappable props |
|---|---|---|
| `#hearth` | an ember on the logs, plus Mandoo the Maltese in her bed by the fire | lamp, lantern, candle, radio (music), karaoke machine (singer queue), Mandoo |
| `#bedroom` | a little candle | floor lamp, ceiling fan (the AC), fairy lights |
| `#porch` | the porch lantern | porch-light switch, thermometer (+ subway board) |

Getting around: tap the little signs in the bottom corners (bed = bedroom, flame = living room, door = front door), swipe left/right on the scene, or use the arrow keys. The house runs Bedroom ← Living room → Front door. A screen opened with a `#room` drifts back to that room after 3 idle minutes.

## Configure
- Your devices, which prop controls what, and scenes: `ember.json` on the Pi (git-ignored, never served). See [HOME_ASSISTANT.md](HOME_ASSISTANT.md).
- Look and behaviour (trains, focus sessions, mischief): `CONFIG` at the top of `index.html`, or override in `config.local.js` (git-ignored).

## Update a device
```bash
cd ~/ember && git pull      # then refresh the browser; replace ember-server if there's a new build
```
