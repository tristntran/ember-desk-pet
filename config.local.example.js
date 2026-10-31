// Copy to config.local.js on each device (it's git-ignored) and fill in.
// Anything here overrides CONFIG in index.html, for this device only.
//
// Your Home Assistant token does NOT go here: anyone who can reach the Pi can read this file.
// Put it in ember.json on the Pi instead (see HOME_ASSISTANT.md); the server keeps it private.
window.EMBER_LOCAL = {
  // trains: { stations: [ ... ] },   // e.g. tweak walk times per device
  // fps: 20,                         // Pi Zero 2 W
};
