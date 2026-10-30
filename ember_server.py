#!/usr/bin/env python3
"""Ember desk pet — local server.

Serves index.html and relays the MTA subway feeds so the page can read them
(browsers block reading MTA's feed directly from another site).

    python3 ember_server.py            # then open http://localhost:8080

Standard library only. Listens on this machine only (127.0.0.1).
"""
import http.server
import os
import socketserver
import threading
import time
import urllib.request

PORT = int(os.environ.get("EMBER_PORT", "8080"))
MTA_BASE = os.environ.get(
    "MTA_BASE", "https://api-endpoint.mta.info/Dataservice/mtagtfsfeeds/nyct%2F")
FEEDS = {"gtfs", "gtfs-ace", "gtfs-bdfm", "gtfs-g", "gtfs-jz", "gtfs-nqrw", "gtfs-l", "gtfs-si"}
CACHE_SECONDS = 20  # MTA updates roughly every 30s; don't hammer it

_cache = {}
_locks = {f: threading.Lock() for f in FEEDS}


def get_feed(feed):
    with _locks[feed]:
        hit = _cache.get(feed)
        if hit and time.time() - hit[0] < CACHE_SECONDS:
            return hit[1]
        req = urllib.request.Request(MTA_BASE + feed, headers={"User-Agent": "ember-desk-pet"})
        with urllib.request.urlopen(req, timeout=10) as r:
            body = r.read()
        _cache[feed] = (time.time(), body)
        return body


class Handler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/mta/"):
            feed = self.path[5:].split("?")[0]
            if feed not in FEEDS:
                return self.send_error(404, "unknown feed")
            try:
                body = get_feed(feed)
            except Exception as e:  # network down, MTA hiccup, etc.
                return self.send_error(502, str(e))
            self.send_response(200)
            self.send_header("Content-Type", "application/x-protobuf")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        return super().do_GET()

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    os.chdir(os.path.dirname(os.path.abspath(__file__)))
    socketserver.ThreadingTCPServer.allow_reuse_address = True
    with socketserver.ThreadingTCPServer(("127.0.0.1", PORT), Handler) as srv:
        print(f"Ember is running at http://localhost:{PORT}")
        srv.serve_forever()
