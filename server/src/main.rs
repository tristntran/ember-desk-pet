//! Ember desk pet: home server.
//!
//! Serves the screens (index.html) and the phone remote (/remote), keeps Ember's state on disk,
//! exposes a small JSON API, bridges to Home Assistant (the token never leaves this machine),
//! and relays the MTA subway feeds for the porch train board.
//!
//! LAN only by design:
//!   * requests from addresses outside your local network are refused;
//!   * requests whose Host isn't an IP / localhost / a local name are refused (stops DNS rebinding);
//!   * POSTs sent by other websites' pages (Origin doesn't match) are refused (stops drive-by commands);
//!   * Home Assistant must live at a local address, so the token can't be sent off your network;
//!   * the only internet traffic is the MTA feed, and `"trains": false` in ember.json turns that off.
//!
//! Environment: EMBER_HOST (0.0.0.0), EMBER_PORT (8080), EMBER_DIR (folder with index.html; default: current),
//! EMBER_STATE (ember_state.json), EMBER_CONFIG (ember.json), EMBER_API_DOMAINS, EMBER_API_TOKEN,
//! EMBER_HOSTNAMES (extra local names allowed in the Host header, comma separated).

use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Cursor, Read};
use std::net::{IpAddr, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Method, Request, Response, Server};

const PET_ACTIONS: &[&str] = &["feed", "mischief", "pet", "play", "sleep", "spray", "tidy", "wake"];
const TOYS: &[&str] = &["ball", "bubbles", "feather", "music"];
const FOODS: &[&str] = &["log", "oil", "pepper", "pinecone", "wax"];
const DEVICE_SERVICES: &[&str] = &[
    "color", "mute", "next", "pause", "play", "play_pause", "press", "previous", "select_source", "set_fan_mode",
    "set_hvac_mode", "set_temperature", "toggle", "turn_off", "turn_on", "volume_down", "volume_set", "volume_up",
];
const DATA_KEYS: &[&str] = &[
    "brightness_pct", "rgb_color", "color_temp_kelvin", "temperature", "hvac_mode", "fan_mode", "volume_level",
    "source", "is_volume_muted",
];
const CALL_DOMAINS: &[&str] = &[
    "light", "climate", "media_player", "remote", "button", "switch", "fan", "scene", "script", "input_boolean",
];
const FEEDS: &[&str] = &["gtfs", "gtfs-ace", "gtfs-bdfm", "gtfs-g", "gtfs-jz", "gtfs-nqrw", "gtfs-l", "gtfs-si"];
const MTA_BASE: &str = "https://api-endpoint.mta.info/Dataservice/mtagtfsfeeds/nyct%2F";
const STATIC_FILES: &[(&str, &str, &str)] = &[
    ("/", "index.html", "text/html; charset=utf-8"),
    ("/index.html", "index.html", "text/html; charset=utf-8"),
    ("/remote", "remote.html", "text/html; charset=utf-8"),
    ("/remote.html", "remote.html", "text/html; charset=utf-8"),
    ("/config.local.js", "config.local.js", "text/javascript; charset=utf-8"),
    ("/vendor/qrcode.js", "vendor/qrcode.js", "text/javascript; charset=utf-8"),
];
const MTA_CACHE: Duration = Duration::from_secs(20);
const LEASE_SECONDS: f64 = 10.0;
const ACTION_TTL: f64 = 120.0;
const MAX_BODY: u64 = 2_000_000;

// ---------------------------------------------------------------------------------------------
// small helpers
// ---------------------------------------------------------------------------------------------
fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}
fn env(k: &str, d: &str) -> String {
    std::env::var(k).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| d.to_string())
}
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
fn rand_hex(n: usize) -> String {
    let mut b = vec![0u8; n.div_ceil(2)];
    if fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_err() {
        let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(7);
        for (i, x) in b.iter_mut().enumerate() {
            *x = (t >> ((i % 16) * 8)) as u8 ^ (i as u8).wrapping_mul(97);
        }
    }
    b.iter().map(|x| format!("{x:02x}")).collect::<String>()[..n].to_string()
}
fn clean(v: Option<&Value>, n: usize) -> String {
    let s = match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(o) => o.to_string(),
    };
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(n).collect()
}
fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
fn domain(entity: &str) -> &str {
    entity.split('.').next().unwrap_or("")
}

/// Loopback, private (RFC 1918), link-local, or IPv6 unique-local / link-local.
fn is_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local(),
        IpAddr::V6(v) => {
            if let Some(m) = v.to_ipv4_mapped() {
                return m.is_loopback() || m.is_private() || m.is_link_local();
            }
            let s0 = v.segments()[0];
            v.is_loopback() || (s0 & 0xfe00) == 0xfc00 || (s0 & 0xffc0) == 0xfe80
        }
    }
}

/// Host header must be an IP, localhost, a local-only name (.local/.lan/.home.arpa/.internal/.home)
/// or one listed in EMBER_HOSTNAMES. Anything else is likely DNS rebinding from a website.
fn host_ok(host: &str, extra: &[String]) -> bool {
    let h = host.trim().to_ascii_lowercase();
    let name = if h.starts_with('[') {
        h.split(']').next().unwrap_or("").trim_start_matches('[').to_string()
    } else {
        h.rsplit_once(':').map(|(a, p)| if p.chars().all(|c| c.is_ascii_digit()) { a } else { h.as_str() }).unwrap_or(&h).to_string()
    };
    if name.parse::<IpAddr>().is_ok() || name == "localhost" || !name.contains('.') {
        return true; // bare names like "ember" only resolve on your own network
    }
    [".local", ".lan", ".home.arpa", ".internal", ".home", ".localhost"].iter().any(|t| name.ends_with(t))
        || extra.iter().any(|e| e == &name)
}

fn lan_url(port: u16) -> String {
    let ip = UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("10.255.255.255:1").map(|_| s)) // no packets are sent
        .and_then(|s| s.local_addr())
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into());
    format!("http://{ip}:{port}")
}

// ---------------------------------------------------------------------------------------------
// settings
// ---------------------------------------------------------------------------------------------
struct Settings {
    host: String,
    port: u16,
    dir: PathBuf,
    state_file: PathBuf,
    api_domains: Vec<String>,
    api_token: String,
    hostnames: Vec<String>,
    trains: bool,
}

// ---------------------------------------------------------------------------------------------
// Home Assistant bridge
// ---------------------------------------------------------------------------------------------
struct Ha {
    url: String,
    token: String,
    poll: Duration,
    devices: Vec<Value>,
    ember: Value,
    wanted: HashSet<String>,
    agent: ureq::Agent,
    live: Mutex<(Vec<Value>, String)>, // (entities, status)
    wake: (Mutex<bool>, Condvar),
    /// set only after the address passed the local-network check
    active: std::sync::atomic::AtomicBool,
}

impl Ha {
    fn new(cfg: &Value) -> Ha {
        let ha = &cfg["home_assistant"];
        let devices = cfg["devices"].as_array().cloned().unwrap_or_default();
        let ember = if cfg["ember"].is_object() { cfg["ember"].clone() } else { json!({}) };
        let mut wanted = HashSet::new();
        for d in &devices {
            for k in ["entity_id", "remote"] {
                if let Some(v) = d[k].as_str() {
                    wanted.insert(v.to_string());
                }
            }
            for c in d["controls"].as_array().into_iter().flatten() {
                if let Some(v) = c["entity"].as_str() {
                    wanted.insert(v.to_string());
                }
            }
        }
        for sc in ember["scenes"].as_array().into_iter().flatten() {
            if let Some(v) = sc["entity"].as_str() {
                wanted.insert(v.to_string());
            }
        }
        let mut props: Vec<&Value> = vec![&ember["props"]];
        for r in ember["rooms"].as_object().into_iter().flat_map(|o| o.values()) {
            props.push(&r["props"]);
        }
        for p in props {
            for v in p.as_object().into_iter().flat_map(|o| o.values()) {
                if let Some(v) = v.as_str() {
                    wanted.insert(v.to_string());
                }
            }
        }
        Ha {
            url: s(&ha["url"]).trim_end_matches('/').to_string(),
            token: s(&ha["token"]).to_string(),
            poll: Duration::from_secs_f64(cfg["poll_seconds"].as_f64().unwrap_or(2.0).clamp(0.5, 60.0)),
            devices,
            ember,
            wanted,
            agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).redirects(0).build(),
            live: Mutex::new((vec![], "not configured".into())),
            wake: (Mutex::new(false), Condvar::new()),
            active: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn filled_in(&self) -> bool {
        !self.url.is_empty() && !self.token.is_empty() && !self.token.starts_with("PASTE")
    }
    /// True only when filled in AND the address is on your network.
    fn configured(&self) -> bool {
        self.active.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Refuse to send the token anywhere but your own network.
    fn check_local(&self) -> Result<(), String> {
        let rest = self
            .url
            .strip_prefix("http://")
            .map(|r| (r, 80))
            .or_else(|| self.url.strip_prefix("https://").map(|r| (r, 443)))
            .ok_or("home_assistant.url must start with http:// or https://")?;
        let hostport = rest.0.split('/').next().unwrap_or("");
        let target = if hostport.contains(':') && !hostport.ends_with(']') { hostport.to_string() } else { format!("{hostport}:{}", rest.1) };
        let addrs: Vec<_> = target.to_socket_addrs().map_err(|e| format!("can't resolve {hostport}: {e}"))?.collect();
        if addrs.is_empty() || addrs.iter().any(|a| !is_local_ip(a.ip())) {
            return Err(format!("{hostport} isn't on your local network; refusing to send the token there"));
        }
        Ok(())
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        self.agent
            .get(&format!("{}{}", self.url, path))
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(ha_err)?
            .into_json()
            .map_err(|e| e.to_string())
    }

    fn poll_loop(self: Arc<Self>) {
        loop {
            let res = self.get("/api/states");
            {
                let mut live = lock(&self.live);
                match res {
                    Ok(Value::Array(all)) => {
                        live.0 = all
                            .into_iter()
                            .filter(|e| self.wanted.contains(s(&e["entity_id"])))
                            .map(|e| json!({"entity_id": e["entity_id"], "state": e["state"], "attributes": e["attributes"]}))
                            .collect();
                        live.1 = "connected".into();
                    }
                    Ok(_) => live.1 = "unexpected reply from Home Assistant".into(),
                    Err(e) => live.1 = format!("can't reach Home Assistant ({e})"),
                }
            }
            let (m, cv) = &self.wake;
            let g = lock(m);
            let (mut g, _) = cv.wait_timeout_while(g, self.poll, |woken| !*woken).unwrap_or_else(|e| e.into_inner());
            *g = false;
        }
    }

    fn call(&self, dom: &str, service: &str, data: &Value) -> Result<(), (u16, String)> {
        if !self.configured() {
            return Err((503, "Home Assistant isn't connected".into()));
        }
        if !CALL_DOMAINS.contains(&dom) {
            return Err((400, format!("'{dom}' actions aren't allowed")));
        }
        if service.is_empty() || !service.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            return Err((400, "bad service name".into()));
        }
        let ids: Vec<&str> = match &data["entity_id"] {
            Value::String(x) => vec![x.as_str()],
            Value::Array(a) => a.iter().map(s).collect(),
            _ => vec![],
        };
        if ids.is_empty() {
            return Err((400, "entity_id is required".into()));
        }
        if let Some(bad) = ids.iter().find(|i| !self.wanted.contains(**i)) {
            return Err((400, format!("{bad} isn't in ember.json")));
        }
        self.agent
            .post(&format!("{}/api/services/{dom}/{service}", self.url))
            .set("Authorization", &format!("Bearer {}", self.token))
            .send_json(data.clone())
            .map_err(|e| (502, format!("Home Assistant: {}", ha_err(e))))?;
        let (m, cv) = &self.wake;
        *lock(m) = true;
        cv.notify_all();
        Ok(())
    }

    fn device(&self, id: &str) -> Option<&Value> {
        self.devices.iter().find(|d| s(&d["entity_id"]) == id)
    }

    fn current(&self, id: &str) -> Value {
        lock(&self.live).0.iter().find(|e| s(&e["entity_id"]) == id).cloned().unwrap_or(Value::Null)
    }

    /// Friendly API verb → Home Assistant action(s).
    fn translate(&self, entity: &str, service: &str, body: &Value) -> Result<Vec<(String, String, Value)>, String> {
        let dom = domain(entity);
        let cur = self.current(entity);
        let st = cur["state"].as_str();
        let pick = |keys: &[&str]| -> Map<String, Value> {
            keys.iter().filter_map(|k| body.get(*k).map(|v| (k.to_string(), v.clone()))).collect()
        };
        let one = |d: &str, sv: &str, extra: Map<String, Value>| {
            let mut m = extra;
            m.insert("entity_id".into(), json!(entity));
            vec![(d.to_string(), sv.to_string(), Value::Object(m))]
        };
        let remote = self.device(entity).and_then(|d| d["remote"].as_str()).map(String::from);
        let off = matches!(st, None | Some("off") | Some("standby"));
        match service {
            "turn_on" | "turn_off" if dom == "media_player" && remote.is_some() => Ok(vec![(
                "remote".into(),
                "send_command".into(),
                json!({"entity_id": remote, "command": if service == "turn_on" { "wakeup" } else { "suspend" }}),
            )]),
            "toggle" => match dom {
                "button" => Ok(one("button", "press", Map::new())),
                "climate" => Ok(one("climate", if off { "turn_on" } else { "turn_off" }, Map::new())),
                "media_player" if remote.is_some() => self.translate(entity, if off { "turn_on" } else { "turn_off" }, body),
                _ => Ok(one(dom, "toggle", Map::new())),
            },
            "turn_on" => Ok(one(dom, "turn_on", if dom == "light" { pick(&["brightness_pct", "rgb_color", "color_temp_kelvin"]) } else { Map::new() })),
            "turn_off" => Ok(one(dom, "turn_off", Map::new())),
            "color" => Ok(one("light", "turn_on", pick(&["rgb_color", "color_temp_kelvin", "brightness_pct"]))),
            "mute" => {
                let muted = cur["attributes"]["is_volume_muted"].as_bool().unwrap_or(false);
                let mut m = Map::new();
                m.insert("is_volume_muted".into(), json!(!muted));
                Ok(one("media_player", "volume_mute", m))
            }
            _ => {
                let (d, sv, keys): (&str, &str, &[&str]) = match service {
                    "set_temperature" => ("climate", "set_temperature", &["temperature"]),
                    "set_hvac_mode" => ("climate", "set_hvac_mode", &["hvac_mode"]),
                    "set_fan_mode" => ("climate", "set_fan_mode", &["fan_mode"]),
                    "play_pause" => ("media_player", "media_play_pause", &[]),
                    "play" => ("media_player", "media_play", &[]),
                    "pause" => ("media_player", "media_pause", &[]),
                    "next" => ("media_player", "media_next_track", &[]),
                    "previous" => ("media_player", "media_previous_track", &[]),
                    "volume_up" => ("media_player", "volume_up", &[]),
                    "volume_down" => ("media_player", "volume_down", &[]),
                    "volume_set" => ("media_player", "volume_set", &["volume_level"]),
                    "select_source" => ("media_player", "select_source", &["source"]),
                    "press" => ("button", "press", &[]),
                    _ => return Err("unknown service".into()),
                };
                let missing: Vec<&str> = keys.iter().copied().filter(|k| body.get(*k).is_none()).collect();
                if !missing.is_empty() {
                    return Err(format!("{} is required", missing.join(", ")));
                }
                Ok(one(d, sv, pick(keys)))
            }
        }
    }

    /// Devices as the phone remote and /api/state expect them.
    fn device_list(&self) -> Vec<Value> {
        let live = lock(&self.live);
        self.devices
            .iter()
            .filter_map(|d| {
                let id = s(&d["entity_id"]);
                let e = live.0.iter().find(|e| s(&e["entity_id"]) == id)?;
                let name = d["name"].as_str().or(e["attributes"]["friendly_name"].as_str()).unwrap_or(id);
                Some(json!({"entity_id": id, "name": name, "room": d["room"], "state": e["state"], "attributes": e["attributes"]}))
            })
            .collect()
    }
}

fn ha_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(401, _) => "the token was rejected (401)".into(),
        ureq::Error::Status(c, r) => format!("HTTP {c}: {}", r.into_string().unwrap_or_default().chars().take(200).collect::<String>()),
        ureq::Error::Transport(t) => t.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// app state
// ---------------------------------------------------------------------------------------------
struct Store {
    full: Value,
    summary: Value,
    updated: f64,
    karaoke: Value,
    queue: Vec<Value>,
    lease_id: String,
    lease_room: Value,
    lease_until: f64,
    last_disk: f64,
}

impl Store {
    fn leader_alive(&self) -> bool {
        !self.lease_id.is_empty() && self.lease_until > now()
    }
    fn expire(&mut self) {
        let t = now();
        for a in self.queue.iter_mut() {
            if a["status"] == "pending" && t - a["created"].as_f64().unwrap_or(t) > ACTION_TTL {
                a["status"] = json!("expired");
                a["result"] = json!({"ok": false, "msg": "No Ember screen picked this up"});
            }
        }
        let n = self.queue.len();
        if n > 200 {
            self.queue.drain(..n - 200);
        }
    }
    fn enqueue(&mut self, action: Value, by: &str) -> String {
        let id = rand_hex(10);
        self.queue.push(json!({"id": id, "created": now(), "action": action, "status": "pending", "result": null,
                               "by": by.chars().take(24).collect::<String>()}));
        id
    }
}

struct App {
    cfg: Settings,
    ha: Arc<Ha>,
    store: Mutex<Store>,
    mta: Mutex<HashMap<String, (Instant, Vec<u8>)>>,
    web: ureq::Agent,
}

impl App {
    fn save(&self, st: &mut Store, force: bool) {
        if !force && now() - st.last_disk < 10.0 {
            return;
        }
        st.last_disk = now();
        let data = json!({"full": st.full, "summary": st.summary, "updated": st.updated, "karaoke": st.karaoke});
        let tmp = self.cfg.state_file.with_extension("json.tmp");
        if fs::write(&tmp, data.to_string()).and_then(|_| fs::rename(&tmp, &self.cfg.state_file)).is_err() {
            eprintln!("!! couldn't write {}", self.cfg.state_file.display());
        }
    }

    fn mta(&self, feed: &str) -> Result<Vec<u8>, String> {
        let mut cache = lock(&self.mta);
        if let Some((t, body)) = cache.get(feed) {
            if t.elapsed() < MTA_CACHE {
                return Ok(body.clone());
            }
        }
        let resp = self.web.get(&format!("{MTA_BASE}{feed}")).set("User-Agent", "ember-desk-pet").call().map_err(|e| e.to_string())?;
        let mut body = Vec::new();
        resp.into_reader().take(20_000_000).read_to_end(&mut body).map_err(|e| e.to_string())?;
        cache.insert(feed.to_string(), (Instant::now(), body.clone()));
        Ok(body)
    }
}

// ---------------------------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------------------------
type Resp = Response<Cursor<Vec<u8>>>;

fn hdr(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("valid header")
}
fn reply(code: u16, v: Value) -> Resp {
    Response::from_data(v.to_string().into_bytes())
        .with_status_code(code)
        .with_header(hdr("Content-Type", "application/json"))
        .with_header(hdr("Cache-Control", "no-store"))
}
fn err(code: u16, msg: impl Into<String>) -> Resp {
    reply(code, json!({"error": msg.into()}))
}
fn header<'a>(req: &'a Request, name: &'static str) -> Option<&'a str> {
    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str())
}

fn handle(app: &App, req: &mut Request) -> Resp {
    // ---- LAN-only gate ----
    let peer_ok = req.remote_addr().map(|a| is_local_ip(a.ip())).unwrap_or(false);
    if !peer_ok {
        return err(403, "Ember only talks to your local network");
    }
    if !host_ok(header(req, "Host").unwrap_or(""), &app.cfg.hostnames) {
        return err(403, "unknown host name (add it to EMBER_HOSTNAMES if it's yours)");
    }
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let path = path.to_string();
    let method = req.method().clone();

    if method == Method::Options {
        return Response::from_data(vec![]).with_status_code(204);
    }
    if method == Method::Post {
        // Pages from other websites can't drive Ember: a browser always sends Origin on cross-site POSTs.
        if let Some(origin) = header(req, "Origin") {
            let host = header(req, "Host").unwrap_or("");
            let o = origin.trim_end_matches('/');
            if o != format!("http://{host}") && o != format!("https://{host}") {
                return err(403, "requests from other websites aren't allowed");
            }
        }
        if !app.cfg.api_token.is_empty() && header(req, "X-Ember-Token") != Some(app.cfg.api_token.as_str()) {
            return err(401, "missing or wrong X-Ember-Token");
        }
        let mut raw = Vec::new();
        if req.as_reader().take(MAX_BODY).read_to_end(&mut raw).is_err() {
            return err(400, "couldn't read body");
        }
        let body: Value = serde_json::from_slice(&raw).ok().filter(|v: &Value| v.is_object()).unwrap_or_else(|| json!({}));
        return post(app, &path, &body);
    }
    if method != Method::Get && method != Method::Head {
        return err(405, "use GET or POST");
    }
    get(app, &path, query)
}

fn get(app: &App, p: &str, query: &str) -> Resp {
    let ha = &app.ha;
    if let Some(feed) = p.strip_prefix("/mta/") {
        if !app.cfg.trains {
            return err(404, "trains are turned off in ember.json");
        }
        if !FEEDS.contains(&feed) {
            return err(404, "unknown feed");
        }
        return match app.mta(feed) {
            Ok(b) => Response::from_data(b).with_header(hdr("Content-Type", "application/x-protobuf")),
            Err(e) => err(502, e),
        };
    }
    match p {
        "/api" | "/api/" => {
            let lan = lan_url(app.cfg.port);
            return reply(200, json!({
                "name": "Ember API", "docs": "See API.md", "server": "rust",
                "pet_actions": PET_ACTIONS, "toys": TOYS, "foods": FOODS,
                "device_domains": app.cfg.api_domains, "device_services": DEVICE_SERVICES,
                "token_required_for_post": !app.cfg.api_token.is_empty(), "trains": app.cfg.trains,
                "lan_url": lan, "remote": format!("{lan}/remote")}));
        }
        "/api/config" => {
            let status = lock(&ha.live).1.clone();
            return reply(200, json!({"ha": {"configured": ha.configured(), "status": status},
                                     "devices": ha.devices, "ember": ha.ember, "trains": app.cfg.trains}));
        }
        "/api/ha/states" => {
            let live = lock(&ha.live);
            return reply(200, json!({"configured": ha.configured(), "status": live.1, "entities": live.0}));
        }
        "/api/ping" => return reply(200, json!({"ok": true, "screen_online": lock(&app.store).leader_alive()})),
        "/api/state" => {
            let devices = if ha.configured() { Some(ha.device_list()) } else { None };
            let st = lock(&app.store);
            let mut out = json!({
                "pet": st.summary["pet"], "devices": devices.map(Value::from).unwrap_or_else(|| st.summary["devices"].clone()),
                "updated": st.updated, "screen_online": st.leader_alive(), "screen_room": st.lease_room});
            if out["devices"].is_null() {
                out["devices"] = json!([]);
            }
            if query.split('&').any(|kv| kv == "full" || kv.starts_with("full=")) {
                out["full"] = st.full.clone();
            }
            return reply(200, out);
        }
        "/api/karaoke" => return reply(200, lock(&app.store).karaoke.clone()),
        "/api/queue/next" => {
            let sid = query.split('&').find_map(|kv| kv.strip_prefix("id=")).unwrap_or("");
            let mut st = lock(&app.store);
            if sid.is_empty() || sid != st.lease_id {
                return reply(200, json!([]));
            }
            st.expire();
            let mut todo = vec![];
            for a in st.queue.iter_mut().filter(|a| a["status"] == "pending") {
                a["status"] = json!("running");
                todo.push(a.clone());
            }
            return reply(200, Value::Array(todo));
        }
        _ => {}
    }
    if let Some(aid) = p.strip_prefix("/api/actions/") {
        let mut st = lock(&app.store);
        st.expire();
        return match st.queue.iter().find(|a| a["id"] == aid) {
            Some(a) => reply(200, a.clone()),
            None => err(404, "unknown action id"),
        };
    }
    if p.starts_with("/api/") {
        return err(404, "not found");
    }
    // ---- static files: only the page files, never state/config/source ----
    if let Some((_, file, ctype)) = STATIC_FILES.iter().find(|(u, _, _)| *u == p) {
        return match fs::read(app.cfg.dir.join(file)) {
            Ok(b) => Response::from_data(b).with_header(hdr("Content-Type", ctype)).with_header(hdr("Cache-Control", "no-cache")),
            Err(_) => err(404, "not found"),
        };
    }
    err(404, "not found")
}

fn post(app: &App, p: &str, body: &Value) -> Resp {
    let by = clean(body.get("by"), 24);
    let by = if by.is_empty() { "Someone".to_string() } else { by };
    let ha = &app.ha;

    // ---- screen <-> server plumbing ----
    if p == "/api/lease" {
        let sid = clean(body.get("id"), 64);
        let mut st = lock(&app.store);
        if !sid.is_empty() && (!st.leader_alive() || st.lease_id == sid) {
            st.lease_id = sid.clone();
            st.lease_room = body["room"].clone();
            st.lease_until = now() + LEASE_SECONDS;
        }
        return reply(200, json!({"leader": st.lease_id, "you": st.lease_id == sid}));
    }
    if p == "/api/state" {
        let mut st = lock(&app.store);
        if s(&body["id"]) != st.lease_id || st.lease_id.is_empty() {
            return err(409, "only the screen running Ember can save state");
        }
        st.full = body["full"].clone();
        st.summary = body["summary"].clone();
        st.updated = now();
        app.save(&mut st, false);
        return reply(200, json!({"ok": true}));
    }
    if let Some(aid) = p.strip_prefix("/api/actions/").and_then(|r| r.strip_suffix("/done")) {
        let mut st = lock(&app.store);
        let found = st.queue.iter_mut().find(|a| a["id"] == aid).map(|a| {
            a["status"] = json!("done");
            a["result"] = body["result"].clone();
        });
        return reply(200, json!({"ok": found.is_some()}));
    }

    // ---- pet interactions ----
    if let Some(action) = p.strip_prefix("/api/pet/") {
        if !PET_ACTIONS.contains(&action) {
            return err(400, format!("unknown action; try one of {PET_ACTIONS:?}"));
        }
        let toy = &body["toy"];
        if action == "play" && !toy.is_null() && !TOYS.contains(&s(toy)) {
            return err(400, format!("unknown toy; try one of {TOYS:?}"));
        }
        let food = &body["food"];
        if action == "feed" && !food.is_null() && !FOODS.contains(&s(food)) {
            return err(400, format!("unknown food; try one of {FOODS:?}"));
        }
        let mut st = lock(&app.store);
        let id = st.enqueue(json!({"type": "pet", "action": action, "toy": toy, "food": food}), &by);
        return reply(202, json!({"queued": id, "screen_online": st.leader_alive(), "check": format!("/api/actions/{id}")}));
    }

    // ---- karaoke queue (kept here; works with no screen on) ----
    if let Some(op) = p.strip_prefix("/api/karaoke/") {
        let mut st = lock(&app.store);
        let mut k = st.karaoke.clone();
        if !k["queue"].is_array() {
            k = json!({"current": null, "queue": []});
        }
        let mut out = match op {
            "join" => {
                let name = clean(body.get("name"), 24);
                if name.is_empty() {
                    return err(400, "name is required");
                }
                let entry = json!({"id": rand_hex(8), "name": name, "song": clean(body.get("song"), 60), "added": now()});
                let q = k["queue"].as_array_mut().expect("array");
                q.push(entry.clone());
                let n = q.len();
                if n > 50 {
                    q.drain(..n - 50);
                }
                let pos = k["queue"].as_array().map_or(0, |q| q.len()) + usize::from(!k["current"].is_null());
                json!({"ok": true, "entry": entry, "position": pos})
            }
            "next" => {
                let q = k["queue"].as_array_mut().expect("array");
                let mut cur = if q.is_empty() { Value::Null } else { q.remove(0) };
                if cur.is_object() {
                    cur["started"] = json!(now());
                }
                k["current"] = cur.clone();
                json!({"ok": true, "current": cur})
            }
            "remove" => {
                let rid = body["id"].clone();
                let q = k["queue"].as_array_mut().expect("array");
                let before = q.len();
                q.retain(|e| e["id"] != rid);
                let mut changed = before != q.len();
                if !k["current"].is_null() && k["current"]["id"] == rid {
                    k["current"] = Value::Null;
                    changed = true;
                }
                json!({"ok": changed})
            }
            "clear" => {
                k = json!({"current": null, "queue": []});
                json!({"ok": true})
            }
            _ => return err(400, "use join, next, remove or clear"),
        };
        st.karaoke = k.clone();
        app.save(&mut st, true);
        out["karaoke"] = k;
        return reply(200, out);
    }

    // ---- Home Assistant (screens use this) ----
    if p == "/api/ha/call" {
        if !ha.configured() {
            return err(503, "Home Assistant isn't set up in ember.json");
        }
        let data = if body["data"].is_object() { body["data"].clone() } else { json!({}) };
        return match ha.call(s(&body["domain"]), s(&body["service"]), &data) {
            Ok(()) => reply(200, json!({"ok": true})),
            Err((c, m)) => err(c, m),
        };
    }

    // ---- smart-home control (friendly verbs) ----
    if let Some(rest) = p.strip_prefix("/api/devices/") {
        let Some((entity, service)) = rest.split_once('/').filter(|(_, sv)| !sv.contains('/')) else {
            return err(400, "use /api/devices/<entity_id>/<service>");
        };
        let dom = domain(entity);
        if !app.cfg.api_domains.iter().any(|d| d == dom) {
            return err(403, format!("'{dom}' devices aren't controllable over the API (EMBER_API_DOMAINS)"));
        }
        if !DEVICE_SERVICES.contains(&service) {
            return err(400, format!("unknown service; try one of {DEVICE_SERVICES:?}"));
        }
        if ha.configured() {
            if ha.device(entity).is_none() {
                let known: Vec<&str> = ha.devices.iter().map(|d| s(&d["entity_id"])).collect();
                return reply(404, json!({"error": "unknown device", "known": known}));
            }
            let calls = match ha.translate(entity, service, body) {
                Ok(c) => c,
                Err(e) => return err(400, e),
            };
            for (d, sv, data) in calls {
                if let Err((c, m)) = ha.call(&d, &sv, &data) {
                    return err(c, m);
                }
            }
            return reply(200, json!({"ok": true, "by": by}));
        }
        // demo mode: hand it to the screen running Ember
        let mut st = lock(&app.store);
        let known: Vec<String> = st.summary["devices"].as_array().into_iter().flatten().map(|d| s(&d["entity_id"]).to_string()).collect();
        if !known.is_empty() && !known.iter().any(|k| k == entity) {
            return reply(404, json!({"error": "unknown entity", "known": known}));
        }
        let data: Map<String, Value> = DATA_KEYS.iter().filter_map(|k| body.get(*k).map(|v| (k.to_string(), v.clone()))).collect();
        let id = st.enqueue(json!({"type": "device", "entity_id": entity, "service": service, "data": data}), &by);
        return reply(202, json!({"queued": id, "screen_online": st.leader_alive(), "check": format!("/api/actions/{id}")}));
    }
    err(404, "not found")
}

// ---------------------------------------------------------------------------------------------
fn main() {
    let dir = PathBuf::from(env("EMBER_DIR", "."));
    let config_file = PathBuf::from(env("EMBER_CONFIG", &dir.join("ember.json").to_string_lossy()));
    let cfg_json: Value = match fs::read_to_string(&config_file) {
        Ok(t) => serde_json::from_str(&t).unwrap_or_else(|e| {
            eprintln!("!! {} isn't valid JSON: {e}", config_file.display());
            json!({})
        }),
        Err(_) => json!({}),
    };
    let settings = Settings {
        host: env("EMBER_HOST", "0.0.0.0"),
        port: env("EMBER_PORT", "8080").parse().unwrap_or(8080),
        state_file: PathBuf::from(env("EMBER_STATE", &dir.join("ember_state.json").to_string_lossy())),
        api_domains: env("EMBER_API_DOMAINS", "light,climate,media_player,button,switch")
            .split(',').map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect(),
        api_token: env("EMBER_API_TOKEN", ""),
        hostnames: env("EMBER_HOSTNAMES", "").split(',').map(|d| d.trim().to_ascii_lowercase()).filter(|d| !d.is_empty()).collect(),
        trains: cfg_json["trains"].as_bool().unwrap_or(true),
        dir,
    };

    let ha = Arc::new(Ha::new(&cfg_json));
    if ha.filled_in() {
        match ha.check_local() {
            Ok(()) => {
                ha.active.store(true, std::sync::atomic::Ordering::SeqCst);
                let h = ha.clone();
                thread::spawn(move || h.poll_loop());
                println!("Home Assistant: {}  ({} devices from {})", ha.url, ha.devices.len(), config_file.display());
            }
            Err(e) => {
                eprintln!("!! Home Assistant bridge disabled: {e}");
                lock(&ha.live).1 = e;
            }
        }
    } else {
        println!("Home Assistant: not set up (demo devices). Copy ember.example.json to ember.json to connect.");
    }

    let mut store = Store {
        full: Value::Null, summary: Value::Null, updated: 0.0, karaoke: json!({"current": null, "queue": []}),
        queue: vec![], lease_id: String::new(), lease_room: Value::Null, lease_until: 0.0, last_disk: 0.0,
    };
    if let Ok(saved) = fs::read_to_string(&settings.state_file).map(|t| serde_json::from_str::<Value>(&t).unwrap_or_default()) {
        store.full = saved["full"].clone();
        store.summary = saved["summary"].clone();
        store.updated = saved["updated"].as_f64().unwrap_or(0.0);
        if saved["karaoke"].is_object() {
            store.karaoke = saved["karaoke"].clone();
        }
    }

    let addr = format!("{}:{}", settings.host, settings.port);
    let server = Arc::new(Server::http(&addr).unwrap_or_else(|e| {
        eprintln!("!! can't listen on {addr}: {e}");
        std::process::exit(1)
    }));
    println!("Ember is running on your local network: {}", lan_url(settings.port));
    println!("  API: /api   remote: /remote   controllable device domains: {}", settings.api_domains.join(", "));
    println!(
        "  Internet: {}",
        if settings.trains { "only the MTA train feed (api-endpoint.mta.info). Set \"trains\": false in ember.json for none." } else { "none (trains off)" }
    );

    let app = Arc::new(App {
        cfg: settings,
        ha,
        store: Mutex::new(store),
        mta: Mutex::new(HashMap::new()),
        web: ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build(),
    });

    let workers: Vec<_> = (0..6)
        .map(|_| {
            let (server, app) = (server.clone(), app.clone());
            thread::spawn(move || {
                for mut req in server.incoming_requests() {
                    let resp = handle(&app, &mut req);
                    let _ = req.respond(resp);
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
}
