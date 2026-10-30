// Copy to config.local.js on each device (it's git-ignored) and fill in.
// Anything here overrides CONFIG in index.html.
window.EMBER_LOCAL = {
  ha: {
    enabled: true,
    url: 'ws://homeassistant.local:8123/api/websocket',
    token: 'PASTE_A_LONG_LIVED_ACCESS_TOKEN',
  },
  // trains: { stations: [ ... ] },   // e.g. tweak walk times per device
  // fps: 20,                         // Pi Zero 2 W
};
