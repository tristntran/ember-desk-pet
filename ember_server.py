#!/usr/bin/env python3
"""Ember desk pet — home server.

Serves the screens (index.html), a phone remote (/remote), relays the MTA subway feeds,
stores Ember's state on disk, and exposes a small JSON API on your local network so other
people and scripts can interact with Ember.

    python3 ember_server.py              # then open http://<pi>:8080/#hearth

Standard library only. See API.md for the endpoints.

Environment:
  EMBER_HOST          bind address (default 0.0.0.0 = whole LAN; use 127.0.0.1 for this machine only)
  EMBER_PORT          port (default 8080)
  EMBER_STATE         state file (default ./ember_state.json)
  EMBER_API_DOMAINS   device domains the API may control (default light,climate,media_player)
  EMBER_API_TOKEN     optional: if set, every POST must send header  X-Ember-Token: <token>
"""
import http.server
import json
import socket
import os
import socketserver
import threading
import time
import urllib.request
import uuid
from urllib.parse import urlparse, parse_qs

HERE = os.path.dirname(os.path.abspath(__file__))
HOST = os.environ.get("EMBER_HOST", "0.0.0.0")
PORT = int(os.environ.get("EMBER_PORT", "8080"))
STATE_FILE = os.environ.get("EMBER_STATE", os.path.join(HERE, "ember_state.json"))
API_DOMAINS = {d.strip() for d in os.environ.get("EMBER_API_DOMAINS", "light,climate,media_player").split(",") if d.strip()}
API_TOKEN = os.environ.get("EMBER_API_TOKEN", "")

MTA_BASE = os.environ.get("MTA_BASE", "https://api-endpoint.mta.info/Dataservice/mtagtfsfeeds/nyct%2F")
FEEDS = {"gtfs", "gtfs-ace", "gtfs-bdfm", "gtfs-g", "gtfs-jz", "gtfs-nqrw", "gtfs-l", "gtfs-si"}
MTA_CACHE_SECONDS = 20

PET_ACTIONS = {"feed", "play", "pet", "tidy", "sleep", "wake", "spray", "mischief"}
TOYS = {"ball", "bubbles", "music", "feather"}
FOODS = {"log", "pinecone", "wax", "oil", "pepper"}
DEVICE_SERVICES = {"toggle", "turn_on", "turn_off", "set_temperature", "play_pause", "play", "pause", "next", "previous", "volume_set"}
STATIC_FILES = {"/index.html", "/remote.html", "/config.local.js", "/vendor/qrcode.js"}
LEASE_SECONDS = 10      # a screen must check in this often to stay the one running Ember
ACTION_TTL = 120         # queued actions expire if no screen picks them up

lock = threading.RLock()
state = {"full": None, "summary": None, "updated": 0, "karaoke": {"current": None, "queue": []}}
queue = []               # [{id, created, action, status, result, by}]
lease = {"id": None, "room": None, "until": 0}
_mta_cache, _mta_locks = {}, {f: threading.Lock() for f in FEEDS}
_last_disk_write = 0


def load_state():
    try:
        with open(STATE_FILE) as f:
            data = json.load(f)
        state.update({k: data.get(k) for k in ("full", "summary", "updated") if k in data})
        if isinstance(data.get("karaoke"), dict):
            state["karaoke"] = data["karaoke"]
    except (OSError, ValueError):
        pass


def save_state(force=False):
    global _last_disk_write
    if not force and time.time() - _last_disk_write < 10:
        return
    _last_disk_write = time.time()
    tmp = STATE_FILE + ".tmp"
    with open(tmp, "w") as f:
        json.dump(state, f)
    os.replace(tmp, STATE_FILE)


def lan_url():
    """Best guess at this machine's address on the LAN (for the QR code on the karaoke screen)."""
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.connect(("10.255.255.255", 1))          # no packets are sent; just picks the outbound interface
            ip = s.getsockname()[0]
    except OSError:
        ip = "127.0.0.1"
    return f"http://{ip}:{PORT}"


def clean(text, n):
    return " ".join(str(text or "").split())[:n]


def leader_alive():
    return lease["id"] and lease["until"] > time.time()


def expire_queue():
    now = time.time()
    for a in queue:
        if a["status"] == "pending" and now - a["created"] > ACTION_TTL:
            a["status"], a["result"] = "expired", {"ok": False, "msg": "No Ember screen picked this up"}
    del queue[:-200]     # keep the last 200


def enqueue(action, by):
    a = {"id": uuid.uuid4().hex[:10], "created": time.time(), "action": action, "status": "pending", "result": None, "by": by[:24]}
    queue.append(a)
    return a


def get_mta(feed):
    with _mta_locks[feed]:
        hit = _mta_cache.get(feed)
        if hit and time.time() - hit[0] < MTA_CACHE_SECONDS:
            return hit[1]
        req = urllib.request.Request(MTA_BASE + feed, headers={"User-Agent": "ember-desk-pet"})
        with urllib.request.urlopen(req, timeout=10) as r:
            body = r.read()
        _mta_cache[feed] = (time.time(), body)
        return body


class Handler(http.server.SimpleHTTPRequestHandler):
    # ---------- helpers ----------
    def send_json(self, obj, code=200):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def read_json(self):
        n = int(self.headers.get("Content-Length") or 0)
        if not n:
            return {}
        try:
            data = json.loads(self.rfile.read(min(n, 2_000_000)))
            return data if isinstance(data, dict) else {}
        except ValueError:
            return {}

    def end_headers(self):
        if not self.path.startswith("/api/"):
            self.send_header("Cache-Control", "no-cache")
        super().end_headers()

    def log_message(self, *args):
        pass

    def do_OPTIONS(self):
        self.send_response(204)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type, X-Ember-Token")
        self.end_headers()

    # ---------- GET ----------
    def do_GET(self):
        url = urlparse(self.path)
        p, q = url.path, parse_qs(url.query)
        if p.startswith("/mta/"):
            feed = p[5:]
            if feed not in FEEDS:
                return self.send_error(404, "unknown feed")
            try:
                body = get_mta(feed)
            except Exception as e:
                return self.send_error(502, str(e))
            self.send_response(200)
            self.send_header("Content-Type", "application/x-protobuf")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            return self.wfile.write(body)
        if p == "/remote":
            self.path = "/remote.html"
            return super().do_GET()
        if p in ("/api", "/api/"):
            return self.send_json({
                "name": "Ember API", "docs": "See API.md",
                "pet_actions": sorted(PET_ACTIONS), "toys": sorted(TOYS), "foods": sorted(FOODS),
                "device_domains": sorted(API_DOMAINS), "device_services": sorted(DEVICE_SERVICES),
                "token_required_for_post": bool(API_TOKEN), "lan_url": lan_url(), "remote": lan_url() + "/remote"})
        if p == "/api/ping":
            return self.send_json({"ok": True, "screen_online": bool(leader_alive())})
        if p == "/api/state":
            with lock:
                out = {"pet": (state["summary"] or {}).get("pet"), "devices": (state["summary"] or {}).get("devices", []),
                       "updated": state["updated"], "screen_online": bool(leader_alive()), "screen_room": lease["room"]}
                if "full" in q:
                    out["full"] = state["full"]
            return self.send_json(out)
        if p == "/api/karaoke":
            with lock:
                return self.send_json(state["karaoke"])
        if p.startswith("/api/actions/"):
            aid = p.rsplit("/", 1)[-1]
            with lock:
                expire_queue()
                a = next((x for x in queue if x["id"] == aid), None)
            return self.send_json(a) if a else self.send_json({"error": "unknown action id"}, 404)
        if p == "/api/queue/next":                     # used by the screen running Ember
            sid = (q.get("id") or [""])[0]
            with lock:
                if sid != lease["id"]:
                    return self.send_json([])
                expire_queue()
                todo = [a for a in queue if a["status"] == "pending"]
                for a in todo:
                    a["status"] = "running"
            return self.send_json(todo)
        if p.startswith("/api/"):
            return self.send_json({"error": "not found"}, 404)
        if p == "/":
            self.path = "/index.html"
        elif p not in STATIC_FILES:          # never serve the state file, .git, or the server itself
            return self.send_error(404)
        return super().do_GET()

    # ---------- POST ----------
    def do_POST(self):
        p = urlparse(self.path).path
        if API_TOKEN and self.headers.get("X-Ember-Token") != API_TOKEN:
            return self.send_json({"error": "missing or wrong X-Ember-Token"}, 401)
        body = self.read_json()
        by = str(body.get("by") or "Someone")

        # --- screen ↔ server plumbing ---
        if p == "/api/lease":
            sid, room = str(body.get("id", "")), body.get("room")
            with lock:
                if not leader_alive() or lease["id"] == sid:
                    lease.update({"id": sid, "room": room, "until": time.time() + LEASE_SECONDS})
                return self.send_json({"leader": lease["id"], "you": sid == lease["id"]})
        if p == "/api/state":
            with lock:
                if body.get("id") != lease["id"]:
                    return self.send_json({"error": "only the screen running Ember can save state"}, 409)
                state.update({"full": body.get("full"), "summary": body.get("summary"), "updated": time.time()})
                save_state()
            return self.send_json({"ok": True})
        if p.startswith("/api/actions/") and p.endswith("/done"):
            aid = p.split("/")[3]
            with lock:
                a = next((x for x in queue if x["id"] == aid), None)
                if a:
                    a["status"], a["result"] = "done", body.get("result")
            return self.send_json({"ok": bool(a)})

        # --- public: pet interactions ---
        if p.startswith("/api/pet/"):
            action = p.rsplit("/", 1)[-1]
            if action not in PET_ACTIONS:
                return self.send_json({"error": f"unknown action; try one of {sorted(PET_ACTIONS)}"}, 400)
            toy = body.get("toy")
            if action == "play" and toy is not None and toy not in TOYS:
                return self.send_json({"error": f"unknown toy; try one of {sorted(TOYS)}"}, 400)
            food = body.get("food")
            if action == "feed" and food is not None and food not in FOODS:
                return self.send_json({"error": f"unknown food; try one of {sorted(FOODS)}"}, 400)
            with lock:
                a = enqueue({"type": "pet", "action": action, "toy": toy, "food": food}, by)
                online = bool(leader_alive())
            return self.send_json({"queued": a["id"], "screen_online": online, "check": f"/api/actions/{a['id']}"}, 202)

        # --- public: karaoke queue (kept on the server, works even with no screen on) ---
        if p.startswith("/api/karaoke/"):
            op = p.rsplit("/", 1)[-1]
            with lock:
                k = state["karaoke"]
                if op == "join":
                    name, song = clean(body.get("name"), 24), clean(body.get("song"), 60)
                    if not name:
                        return self.send_json({"error": "name is required"}, 400)
                    entry = {"id": uuid.uuid4().hex[:8], "name": name, "song": song, "added": time.time()}
                    k["queue"].append(entry)
                    k["queue"] = k["queue"][-50:]
                    pos = len(k["queue"]) + (1 if k["current"] else 0)
                    out = {"ok": True, "entry": entry, "position": pos}
                elif op == "next":
                    k["current"] = k["queue"].pop(0) if k["queue"] else None
                    if k["current"]:
                        k["current"]["started"] = time.time()
                    out = {"ok": True, "current": k["current"]}
                elif op == "remove":
                    rid = body.get("id")
                    before = len(k["queue"])
                    k["queue"] = [e for e in k["queue"] if e["id"] != rid]
                    if k["current"] and k["current"]["id"] == rid:
                        k["current"] = None
                        before += 1
                    out = {"ok": before != len(k["queue"])}
                elif op == "clear":
                    k.update({"current": None, "queue": []})
                    out = {"ok": True}
                else:
                    return self.send_json({"error": "use join, next, remove or clear"}, 400)
                save_state(force=True)
                out["karaoke"] = k
            return self.send_json(out)

        # --- public: smart-home control ---
        if p.startswith("/api/devices/"):
            parts = p.split("/")             # /api/devices/<entity_id>/<service>
            if len(parts) != 5:
                return self.send_json({"error": "use /api/devices/<entity_id>/<service>"}, 400)
            entity, service = parts[3], parts[4]
            domain = entity.split(".")[0]
            if domain not in API_DOMAINS:
                return self.send_json({"error": f"'{domain}' devices aren't controllable over the API (EMBER_API_DOMAINS)"}, 403)
            if service not in DEVICE_SERVICES:
                return self.send_json({"error": f"unknown service; try one of {sorted(DEVICE_SERVICES)}"}, 400)
            with lock:
                known = {d.get("entity_id") for d in (state["summary"] or {}).get("devices", [])}
                if known and entity not in known:
                    return self.send_json({"error": "unknown entity", "known": sorted(known)}, 404)
                data = {k: v for k, v in body.items() if k in ("brightness_pct", "temperature", "volume_level")}
                a = enqueue({"type": "device", "entity_id": entity, "service": service, "data": data}, by)
                online = bool(leader_alive())
            return self.send_json({"queued": a["id"], "screen_online": online, "check": f"/api/actions/{a['id']}"}, 202)

        return self.send_json({"error": "not found"}, 404)


if __name__ == "__main__":
    os.chdir(HERE)
    load_state()
    socketserver.ThreadingTCPServer.allow_reuse_address = True
    with socketserver.ThreadingTCPServer((HOST, PORT), Handler) as srv:
        where = "this machine only" if HOST.startswith("127.") else "your local network"
        print(f"Ember is running on {where}: http://{'localhost' if HOST in ('0.0.0.0','127.0.0.1') else HOST}:{PORT}")
        print(f"  API: /api   remote: /remote   controllable device domains: {', '.join(sorted(API_DOMAINS))}")
        try:
            srv.serve_forever()
        finally:
            with lock:
                save_state(force=True)
