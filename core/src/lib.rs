//! Ember's brain.
//!
//! One `Brain` holds everything that is *true* about Ember: the four need bars, friendship, grudge,
//! the chaos meter, what Ember is doing, fires in each room, focus sessions and the logbook.
//! It has no I/O and no clock of its own: callers pass the time in.
//!
//! * The Pi server runs one Brain for the whole house; every screen and phone talks to it.
//! * With no server (file or artifact link) the page runs the same code compiled to WebAssembly.
//!
//! Screens send actions (`feed`, `play`, `spray`…) and draw what the brain says: its state, plus a
//! stream of events (`ate`, `ignite`, `mischief_start`…) that they turn into animations and sounds.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::VecDeque;

// ---------------------------------------------------------------------------------------------
// tuning (mirrors the old in-page values so Ember feels the same)
// ---------------------------------------------------------------------------------------------
/// Spots in each room that can catch fire (scene pixel coordinates).
pub const BURNABLES: &[(&str, &[(i32, i32)])] = &[
    ("hearth", &[(56, 68), (104, 69), (24, 51), (7, 40), (41, 40), (132, 47), (80, 72)]),
    ("bedroom", &[(64, 66), (86, 69), (114, 43), (136, 44), (37, 41), (69, 41), (60, 63)]),
    ("porch", &[(43, 57), (78, 47), (14, 30), (40, 62), (60, 56), (25, 56)]),
];
pub const FOODS: &[&str] = &["log", "pinecone", "wax", "oil", "pepper"];
/// (toy, friendship level needed, seconds it lasts at most)
pub const TOYS: &[(&str, u32, f64)] = &[("ball", 0, 25.0), ("bubbles", 0, 25.0), ("music", 1, 7.5), ("feather", 2, 16.0)];
pub const ACTS: &[&str] = &["nap", "work", "study", "exercise", "music"];
const EAT_SECONDS: f64 = 3.0;
const TIDY_SECONDS: f64 = 5.0;
const TIDY_GRUDGE_MIN: f64 = 30.0; // cleaning again within this many minutes is extra annoying
const PLAY_MIN_FUEL: f64 = 25.0;
const PLAY_MIN_REST: f64 = 25.0;
const LOG_MAX: usize = 1000;
const EVENTS_MAX: usize = 120;

type Fx = [(&'static str, f64); 4];
const fn e1(a: &'static str, x: f64) -> Fx { [(a, x), ("", 0.0), ("", 0.0), ("", 0.0)] }
const EAT_LOG: Fx = [("fuel", 38.0), ("joy", 3.0), ("tidy", -16.0), ("", 0.0)];
const EAT_SMALL: Fx = [("fuel", 22.0), ("joy", 2.0), ("tidy", -10.0), ("", 0.0)];
const PEPPER: Fx = [("fuel", 8.0), ("joy", 8.0), ("tidy", -6.0), ("", 0.0)];
const TIDY: Fx = [("tidy", 100.0), ("rest", -6.0), ("joy", -18.0), ("fuel", -6.0)];
const SWEEPING: Fx = [("joy", -2.5), ("fuel", -1.0), ("", 0.0), ("", 0.0)]; // per second
const GRUDGE_FX: Fx = [("joy", -8.0), ("rest", -3.0), ("", 0.0), ("", 0.0)];
const SQUIRT: Fx = [("fuel", -4.0), ("joy", -5.0), ("", 0.0), ("", 0.0)];
const MISCHIEF: Fx = [("tidy", -2.5), ("joy", 0.6), ("", 0.0), ("", 0.0)]; // per second
const BALL: Fx = [("fuel", -2.0), ("rest", -2.2), ("tidy", -1.4), ("", 0.0)];
const BUBBLES: Fx = [("fuel", -0.8), ("rest", -0.9), ("tidy", -1.5), ("", 0.0)];
const MUSIC: Fx = [("fuel", -0.3), ("rest", -0.5), ("tidy", -0.2), ("", 0.0)];
const FEATHER: Fx = [("fuel", -0.25), ("rest", -0.3), ("tidy", -0.15), ("", 0.0)];

// ---------------------------------------------------------------------------------------------
// state (JSON-compatible with the old in-page save, so nothing is lost when upgrading)
// ---------------------------------------------------------------------------------------------
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Stats { pub fuel: f64, pub joy: f64, pub rest: f64, pub tidy: f64 }
impl Default for Stats { fn default() -> Self { Stats { fuel: 80.0, joy: 75.0, rest: 90.0, tidy: 85.0 } } }
impl Stats {
    fn get(&mut self, k: &str) -> Option<&mut f64> {
        match k { "fuel" => Some(&mut self.fuel), "joy" => Some(&mut self.joy), "rest" => Some(&mut self.rest), "tidy" => Some(&mut self.tidy), _ => None }
    }
    fn clamp(&mut self) { for v in [&mut self.fuel, &mut self.joy, &mut self.rest, &mut self.tidy] { *v = v.clamp(0.0, 100.0); } }
    fn lowest(&self) -> (&'static str, f64) {
        let mut best = ("fuel", self.fuel);
        for (k, v) in [("joy", self.joy), ("rest", self.rest), ("tidy", self.tidy)] { if v < best.1 { best = (k, v); } }
        best
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Fire { pub room: String, pub x: i32, pub y: i32, pub size: f64 }
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Mark { pub room: String, pub x: i32, pub y: i32 }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Timer {
    pub mode: String, // "focus" | "break"
    #[serde(default)] pub act: Option<String>,
    pub start: f64,
    pub end: f64,
    pub total: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LogEntry {
    pub start: f64,
    pub min: i64,
    pub act: String,
    #[serde(default)] pub rating: Option<String>,
    #[serde(default)] pub note: String,
}

/// What Ember is busy with right now.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Activity {
    pub kind: String, // idle | eat | play | tidy | mischief | cowork
    pub since: f64,
    pub until: f64,
    pub toy: Option<String>,
    pub food: Option<String>,
    pub frenzy: bool,
    pub squirts: i32,
    pub owner: String, // the screen that runs the toy mini-game
    pub room: String,
    #[serde(skip)] pub acc: f64,
}
impl Default for Activity {
    fn default() -> Self { Activity { kind: "idle".into(), since: 0.0, until: 0.0, toy: None, food: None, frenzy: false, squirts: 0, owner: String::new(), room: String::new(), acc: 0.0 } }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Prefs { pub mischief: String, pub fires: bool }
impl Default for Prefs { fn default() -> Self { Prefs { mischief: "normal".into(), fires: true } } }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct State {
    pub name: String,
    pub stats: Stats,
    pub fp: f64,
    pub grudge: f64,
    pub chaos: f64,
    pub chaos_at: f64,
    pub asleep: bool,
    pub fires: Vec<Fire>,
    pub scorch: Vec<Mark>,
    pub log: Vec<LogEntry>,
    pub timer: Option<Timer>,
    pub last_tidy: f64,
    pub last_nap: f64,
    pub last_act: String,
    pub last: f64,
    pub activity: Activity,
    pub sulk_until: f64,
    pub room: String,
    pub prefs: Prefs,
    pub last_interact: f64,
    pub log_rev: u64,
    pub blaze: bool,
    pub seq: u64,
}
impl Default for State {
    fn default() -> Self {
        State {
            name: "Ember".into(), stats: Stats::default(), fp: 120.0, grudge: 0.0, chaos: 0.0, chaos_at: 1.0, asleep: false,
            fires: vec![], scorch: vec![], log: vec![], timer: None, last_tidy: 0.0, last_nap: 25.0, last_act: "work".into(),
            last: 0.0, activity: Activity::default(), sulk_until: 0.0, room: "hearth".into(), prefs: Prefs::default(),
            last_interact: 0.0, log_rev: 0, blaze: false, seq: 0,
        }
    }
}

/// Local wall-clock facts the brain can't know by itself.
#[derive(Clone, Copy, Debug)]
pub struct Local { pub hour: f64, pub mday: u32 }
impl Local { fn night(&self) -> bool { self.hour >= 22.0 || self.hour < 6.5 } }

#[derive(Serialize, Clone, Debug)]
pub struct Event {
    pub seq: u64,
    pub t: f64,
    pub kind: String,
    #[serde(flatten)] pub data: Map<String, Value>,
}

pub struct Brain {
    pub s: State,
    events: VecDeque<Event>,
    rng: u64,
    last_tick: f64,
    last_pet: f64,
    last_pet_event: f64,
    hits: (f64, u32),
}

fn clampf(v: f64, a: f64, b: f64) -> f64 { v.max(a).min(b) }
fn str_of<'a>(v: &'a Value, k: &str) -> &'a str { v.get(k).and_then(|x| x.as_str()).unwrap_or("") }

impl Brain {
    /// Start from a saved state (old page saves work too) or a fresh Ember.
    pub fn new(saved: Option<Value>, now: f64) -> Brain {
        let mut s: State = match saved {
            Some(Value::Object(mut m)) => {
                m.retain(|k, v| !v.is_null() || k == "timer"); // old saves may hold nulls
                if let Some(Value::Object(set)) = m.get("settings").cloned() {
                    // migrate old per-screen settings that are really about Ember
                    let mut p = Map::new();
                    if let Some(x) = set.get("mischief") { p.insert("mischief".into(), x.clone()); }
                    if let Some(x) = set.get("fires") { p.insert("fires".into(), json!(x.as_str().map(|f| f != "off").unwrap_or(true))); }
                    m.entry("prefs").or_insert(Value::Object(p));
                }
                serde_json::from_value(Value::Object(m)).unwrap_or_default()
            }
            _ => State::default(),
        };
        if s.last <= 0.0 { s.last = now; }
        if s.chaos_at <= 0.0 { s.chaos_at = 1.0; }
        if s.activity.kind != "idle" && s.activity.kind != "cowork" && s.activity.kind != "mischief" { s.activity = Activity::default(); }
        let seed = (now as u64) ^ 0x9E37_79B9_7F4A_7C15;
        let last = s.last;
        let mut b = Brain { s, events: VecDeque::new(), rng: seed | 1, last_tick: last, last_pet: 0.0, last_pet_event: 0.0, hits: (0.0, 0) };
        b.away(now);
        b
    }

    // ---- small helpers ----
    fn rand(&mut self) -> f64 {
        let mut x = self.rng; x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.rng = x;
        (x >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 { a + self.rand() * (b - a) }
    fn emit(&mut self, now: f64, kind: &str, data: Value) {
        self.s.seq += 1;
        let data = match data { Value::Object(m) => m, _ => Map::new() };
        self.events.push_back(Event { seq: self.s.seq, t: now, kind: kind.into(), data });
        while self.events.len() > EVENTS_MAX { self.events.pop_front(); }
    }
    fn fx(&mut self, f: &Fx, scale: f64) {
        for (k, v) in f.iter() { if let Some(x) = self.s.stats.get(k) { *x += v * scale; } }
        self.s.stats.clamp();
    }
    fn fp(&mut self, n: f64) { self.s.fp = clampf(self.s.fp + n, 0.0, 1000.0); }
    pub fn level(&self) -> u32 { (self.s.fp / 100.0).floor() as u32 }
    fn grudge(&mut self, now: f64, n: f64) {
        let before = self.s.grudge;
        self.s.grudge = clampf(self.s.grudge + n, 0.0, 100.0);
        if n > 0.0 && before < 60.0 && self.s.grudge >= 60.0 { self.emit(now, "grudge_high", json!({})); }
    }
    fn busy(&self) -> bool { !matches!(self.s.activity.kind.as_str(), "idle") }
    fn focus(&self) -> Option<&Timer> { self.s.timer.as_ref().filter(|t| t.mode == "focus") }
    fn idle(&mut self) { self.s.activity = Activity::default(); }
    fn room_fires(&self, room: &str) -> usize { self.s.fires.iter().filter(|f| f.room == room).count() }
    fn cant_play(&self, scale: f64) -> Option<(&'static str, String)> {
        if self.s.stats.fuel < PLAY_MIN_FUEL * scale { return Some(("hungry", format!("{} is too hungry to play. Feed it first", self.s.name))); }
        if self.s.stats.rest < PLAY_MIN_REST * scale { return Some(("sleepy", format!("{} is too sleepy to play. Let it nap", self.s.name))); }
        None
    }
    pub fn fav_toy(local: Local) -> &'static str { TOYS[(local.mday as usize) % TOYS.len()].0 }
    fn schedule(&mut self, mult: f64) { self.s.chaos = 0.0; self.s.chaos_at = self.range(0.7, 1.3) * mult; }

    /// Catch up after being switched off (gentler rates, capped at 12 hours).
    fn away(&mut self, now: f64) {
        let mins = clampf((now - self.s.last) / 60000.0, 0.0, 720.0);
        if mins < 2.0 { self.last_tick = now; return; }
        let st = &mut self.s.stats;
        if self.s.asleep { st.rest += mins * 4.0; st.fuel -= mins * 0.3; }
        else { st.fuel -= mins * 0.35; st.joy -= mins * 0.4; st.rest -= mins * 0.25; }
        st.tidy -= mins * 0.15;
        st.clamp();
        self.s.grudge = (self.s.grudge - mins * 0.1).max(0.0);
        if self.s.activity.kind != "cowork" { self.idle(); }
        self.last_tick = now;
        self.s.last = now;
    }

    // ---- state changes shared by actions and the clock ----
    fn go_sleep(&mut self, now: f64, why: &str) {
        if self.s.asleep { return; }
        if self.s.timer.as_ref().is_some_and(|t| t.mode == "break") { self.s.timer = None; }
        self.idle();
        self.s.asleep = true;
        self.emit(now, "sleep", json!({"why": why}));
    }
    fn wake(&mut self, now: f64, by_user: bool) {
        if !self.s.asleep { return; }
        self.s.asleep = false;
        if self.s.activity.kind != "cowork" { self.idle(); }
        self.emit(now, "wake", json!({"byUser": by_user, "groggy": by_user && self.s.stats.rest < 50.0}));
    }
    fn start_mischief(&mut self, now: f64, frenzy: bool) {
        let extra = if self.rand() < 0.4 { 1 } else { 0 } + if self.s.grudge >= 50.0 { 1 } else { 0 };
        let room = self.s.room.clone();
        self.s.activity = Activity {
            kind: "mischief".into(), since: now, until: now + if frenzy { 35_000.0 } else { 25_000.0 },
            frenzy, squirts: if frenzy { 2 } else { (1 + extra).min(3) }, room, ..Activity::default()
        };
        self.emit(now, "mischief_start", json!({"frenzy": frenzy}));
    }
    fn end_mischief(&mut self, now: f64, how: &str) {
        let frenzy = self.s.activity.frenzy;
        self.idle();
        if frenzy { self.schedule(1.5); }
        else if how == "squirted" { self.s.sulk_until = now + 2600.0; self.schedule(1.5); }
        else { self.fp(-3.0); self.grudge(now, -10.0); self.schedule(1.0); }
        self.emit(now, "mischief_end", json!({"how": how, "frenzy": frenzy}));
    }
    fn can_misbehave(&self, now: f64) -> bool {
        self.s.prefs.mischief != "off" && !self.s.asleep && self.s.activity.kind == "idle" && self.focus().is_none() && now >= self.s.sulk_until
    }
    /// Average minutes between episodes: more grudge or boredom = sooner; more friendship = later.
    pub fn chaos_minutes(&self) -> f64 {
        let base = match self.s.prefs.mischief.as_str() { "rare" => 30.0, "normal" => 14.0, "often" => 2.0, _ => return f64::INFINITY };
        let lv = self.level() as f64;
        let f = (1.0 + self.s.grudge / 35.0) * (1.0 / (0.6 + lv * 0.16)) * if self.s.stats.joy < 40.0 { 2.0 } else { 1.0 };
        base / f
    }
    fn ignite(&mut self, now: f64, room: &str, at: Option<(i32, i32)>) {
        if !self.s.prefs.fires { return; }
        let spots = BURNABLES.iter().find(|(r, _)| *r == room).map(|(_, s)| *s).unwrap_or(&[]);
        let free: Vec<(i32, i32)> = spots.iter().copied().filter(|(x, y)| !self.s.fires.iter().any(|f| f.room == room && f.x == *x && f.y == *y)).collect();
        if free.is_empty() { return; }
        let (x, y) = match at { Some(p) if free.contains(&p) => p, _ => free[(self.rand() * free.len() as f64) as usize % free.len()] };
        self.s.fires.push(Fire { room: room.into(), x, y, size: 0.6 });
        let first = self.room_fires(room) == 1;
        self.emit(now, "ignite", json!({"room": room, "x": x, "y": y, "first": first}));
    }
    fn douse_fire(&mut self, now: f64, i: usize) {
        let f = &mut self.s.fires[i];
        f.size -= 1.3;
        let (room, x, y, out) = (f.room.clone(), f.x, f.y, f.size <= 0.3);
        if out {
            self.s.fires.remove(i);
            self.s.scorch.push(Mark { room: room.clone(), x, y });
            self.fp(0.5);
        }
        let all_out = self.room_fires(&room) == 0;
        if out && all_out { self.fp(2.0); }
        self.emit(now, "doused", json!({"room": room, "x": x, "y": y, "out": out, "allOut": out && all_out}));
    }
    fn squirt_pet(&mut self, now: f64) {
        self.fx(&SQUIRT, 1.0);
        let frenzy = self.s.activity.frenzy;
        if !frenzy { self.grudge(now, 12.0); }
        if self.s.activity.kind == "mischief" {
            self.s.activity.squirts -= 1;
            let left = self.s.activity.squirts;
            self.emit(now, "squirted", json!({"left": left.max(0)}));
            if left <= 0 { self.end_mischief(now, "squirted"); }
        } else {
            self.emit(now, "squirted", json!({"left": 0}));
        }
    }
    fn end_play(&mut self, now: f64, how: &str, msg: &str) {
        let toy = self.s.activity.toy.clone();
        self.idle();
        if how == "done" { self.grudge(now, -8.0); }
        self.emit(now, "play_end", json!({"how": how, "toy": toy, "msg": msg}));
    }
    fn log_session(&mut self, start: f64, minutes: f64, act: &str, rating: Option<&str>) {
        if minutes < 1.0 { return; }
        self.s.log.insert(0, LogEntry { start, min: minutes.round() as i64, act: act.into(), rating: rating.map(String::from), note: String::new() });
        self.s.log.truncate(LOG_MAX);
        self.s.log_rev += 1;
    }

    // -----------------------------------------------------------------------------------------
    // the clock
    // -----------------------------------------------------------------------------------------
    /// Advance time. Call a few times a second (server) or every frame-ish (browser).
    pub fn tick(&mut self, now: f64, local: Local) {
        let gap = now - self.last_tick;
        if gap > 120_000.0 { self.s.last = self.last_tick; self.away(now); }
        let dt = clampf((now - self.last_tick) / 1000.0, 0.0, 5.0);
        self.last_tick = now;
        self.s.last = now;
        if dt <= 0.0 { return; }

        // sessions
        if let Some(t) = self.s.timer.clone() {
            if now >= t.end {
                self.s.timer = None;
                if t.mode == "focus" {
                    self.fp(5.0); self.fx(&e1("joy", 6.0), 1.0); self.grudge(now, -5.0);
                    let act = t.act.clone().unwrap_or_else(|| "nap".into());
                    self.log_session(t.start, t.total / 60000.0, &act, None);
                    if self.s.asleep { self.wake(now, false); }
                    if self.s.activity.kind == "cowork" { self.idle(); }
                    self.emit(now, "session_done", json!({"act": act, "min": (t.total / 60000.0).round()}));
                } else {
                    self.emit(now, "break_done", json!({}));
                }
            }
        }

        // needs drift
        let m = dt / 60.0;
        {
            let s = &mut self.s.stats;
            if self.s.asleep { s.rest += 4.0 * m; s.fuel -= 0.3 * m; }
            else {
                let hungry = s.fuel < 20.0;
                s.fuel -= 0.7 * m;
                s.joy -= 0.9 * (1.0 + if hungry { 1.0 } else { 0.0 } + if s.tidy < 20.0 { 0.5 } else { 0.0 }) * m;
                s.rest -= if hungry { 0.8 } else { 0.45 } * m;
            }
            s.tidy -= 0.25 * m;
            s.clamp();
        }
        if self.s.stats.lowest().1 <= 0.0 { self.fp(-0.6 * m); }
        self.s.grudge = (self.s.grudge - dt * (6.0 / 3600.0)).max(0.0);

        // what Ember is doing
        let kind = self.s.activity.kind.clone();
        match kind.as_str() {
            "eat" if now >= self.s.activity.until => {
                let food = self.s.activity.food.clone().unwrap_or_else(|| "log".into());
                self.idle();
                if food == "pepper" {
                    self.fx(&PEPPER, 1.0);
                    self.emit(now, "ate", json!({"food": food}));
                    self.start_mischief(now, true);
                } else {
                    self.grudge(now, -4.0);
                    if self.s.stats.fuel < 85.0 { self.fp(6.0); }
                    self.fx(if food == "pinecone" { &EAT_SMALL } else { &EAT_LOG }, 1.0);
                    self.emit(now, "ate", json!({"food": food}));
                }
            }
            "play" => {
                if let Some((icon, msg)) = self.cant_play(0.5) {
                    let msg = msg.replace("to play", "to keep playing");
                    self.end_play(now, icon, &msg);
                } else if now >= self.s.activity.until { self.end_play(now, "done", ""); }
            }
            "tidy" => {
                self.s.activity.acc += dt;
                while self.s.activity.acc >= 1.0 { self.s.activity.acc -= 1.0; self.fx(&SWEEPING, 1.0); }
                if now >= self.s.activity.until {
                    let room = self.s.activity.room.clone();
                    self.idle();
                    self.s.scorch.retain(|m| m.room != room);
                    if self.s.stats.tidy < 70.0 { self.fp(5.0); }
                    self.fx(&TIDY, 1.0);
                    self.grudge(now, 22.0);
                    let again = self.s.last_tidy > 0.0 && now - self.s.last_tidy < TIDY_GRUDGE_MIN * 60000.0;
                    if again { self.grudge(now, 18.0); self.fx(&GRUDGE_FX, 1.0); }
                    self.s.sulk_until = now + if again { 9000.0 } else { 6000.0 };
                    self.s.last_tidy = now;
                    self.emit(now, "tidy_end", json!({"room": room, "again": again}));
                }
            }
            "mischief" => {
                self.s.activity.acc += dt;
                while self.s.activity.acc >= 1.0 { self.s.activity.acc -= 1.0; self.fx(&MISCHIEF, 1.0); }
                let p = dt * 0.12 * (1.0 + self.s.grudge / 50.0) * if self.s.activity.frenzy { 2.5 } else { 1.0 };
                if self.rand() < p { let room = self.s.room.clone(); self.ignite(now, &room, None); }
                if now >= self.s.activity.until { self.end_mischief(now, "ignored"); }
            }
            "cowork" => {
                if self.focus().is_none() { self.idle(); }
                else if self.focus().and_then(|t| t.act.as_deref()) == Some("exercise") {
                    self.s.activity.acc += dt;
                    if self.s.activity.acc >= 60.0 { self.s.activity.acc = 0.0; self.fx(&[("rest", -2.0), ("fuel", -1.0), ("", 0.0), ("", 0.0)], 1.0); }
                }
            }
            _ => {}
        }

        // chaos meter
        if self.can_misbehave(now) {
            let mins = self.chaos_minutes();
            if mins.is_finite() {
                self.s.chaos += dt / (mins * 60.0);
                if self.s.chaos >= self.s.chaos_at { self.start_mischief(now, false); }
            }
        }

        // fires grow, spread and make a mess
        let mut spread: Vec<(String, (i32, i32))> = vec![];
        let mut room_total: Vec<(String, f64)> = vec![];
        for i in 0..self.s.fires.len() {
            let f = &mut self.s.fires[i];
            f.size = (f.size + dt * 0.07).min(3.0);
            let (room, x, y, size) = (f.room.clone(), f.x, f.y, f.size);
            match room_total.iter_mut().find(|(r, _)| *r == room) { Some(e) => e.1 += size, None => room_total.push((room.clone(), size)) }
            if size >= 2.5 && self.rand() < dt * 0.04 {
                let spots = BURNABLES.iter().find(|(r, _)| *r == room).map(|(_, s)| *s).unwrap_or(&[]);
                let near = spots.iter().copied()
                    .filter(|(sx, sy)| !self.s.fires.iter().any(|g| g.room == room && g.x == *sx && g.y == *sy))
                    .min_by(|a, b| {
                        let da = ((a.0 - x).pow(2) + (a.1 - y).pow(2)) as f64;
                        let db = ((b.0 - x).pow(2) + (b.1 - y).pow(2)) as f64;
                        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                    });
                if let Some(p) = near { spread.push((room, p)); }
            }
        }
        for (room, p) in spread { self.ignite(now, &room, Some(p)); }
        let total: f64 = room_total.iter().map(|(_, t)| t).sum();
        if total > 0.0 { self.fx(&e1("tidy", -0.35 * total), dt); }
        let worst = room_total.iter().cloned().fold((String::new(), 0.0), |a, b| if b.1 > a.1 { b } else { a });
        let blaze = worst.1 >= 8.0;
        if blaze && !self.s.blaze { self.fp(-5.0); self.emit(now, "blaze", json!({"room": worst.0})); }
        self.s.blaze = blaze;

        // sleepy-time and waking up
        if !self.s.asleep && self.s.activity.kind == "idle" && self.focus().is_none() {
            if self.s.stats.rest < 6.0 { self.go_sleep(now, "tired"); }
            else if local.night() && now - self.s.last_interact > 120_000.0 { self.go_sleep(now, "bedtime"); }
        }
        if self.s.asleep && self.s.stats.rest >= 100.0 && !local.night() && self.s.timer.is_none() { self.wake(now, false); }
    }

    // -----------------------------------------------------------------------------------------
    // actions from screens, phones and the API
    // -----------------------------------------------------------------------------------------
    /// Apply an action like `{"type":"feed","food":"log","room":"hearth","by":"Kat","screen":"abc"}`.
    /// Returns `{"ok":bool,"msg":..., "code":...}`.
    pub fn act(&mut self, a: &Value, now: f64, local: Local) -> Value {
        self.tick(now, local);
        let kind = str_of(a, "type").to_string();
        let by = str_of(a, "by").chars().take(40).collect::<String>();
        let screen = str_of(a, "screen").chars().take(40).collect::<String>();
        let room = str_of(a, "room");
        if BURNABLES.iter().any(|(r, _)| *r == room) { self.s.room = room.to_string(); }
        let room = self.s.room.clone();
        if !matches!(kind.as_str(), "play_hit" | "rate" | "prefs" | "rename") { self.s.last_interact = now; }
        let name = self.s.name.clone();
        let no = |code: &str, msg: String| json!({"ok": false, "code": code, "msg": msg});
        let yes = || json!({"ok": true});

        match kind.as_str() {
            "feed" => {
                let food = str_of(a, "food");
                let food = if FOODS.contains(&food) { food.to_string() } else { "log".to_string() };
                if self.s.activity.kind == "cowork" { return no("busy", format!("{name} is busy working with you")); }
                if self.s.asleep { return no("asleep", format!("{name} is asleep… shh")); }
                if self.busy() { return no("busy", format!("{name} is busy")); }
                if self.s.stats.fuel >= 97.0 && food != "pepper" { return no("full", format!("{name} is full!")); }
                self.s.activity = Activity { kind: "eat".into(), since: now, until: now + EAT_SECONDS * 1000.0, food: Some(food.clone()), owner: screen, room: room.clone(), ..Activity::default() };
                self.emit(now, "feed", json!({"food": food, "by": by, "room": room}));
                yes()
            }
            "play" => {
                if self.s.asleep { return no("asleep", format!("{name} is asleep… shh")); }
                if self.s.activity.kind == "mischief" { return no("mischief", format!("{name} is misbehaving. Spray it instead")); }
                if self.busy() { return no("busy", format!("{name} is busy")); }
                if let Some((code, msg)) = self.cant_play(1.0) { return no(code, msg); }
                let want = str_of(a, "toy");
                let (toy, lv, secs) = *TOYS.iter().find(|t| t.0 == want).unwrap_or(&TOYS[0]);
                if self.level() < lv { return no("locked", format!("Reach friendship {lv} to unlock that toy")); }
                self.s.activity = Activity { kind: "play".into(), since: now, until: now + secs * 1000.0, toy: Some(toy.into()), owner: screen, room: room.clone(), ..Activity::default() };
                let fav = Brain::fav_toy(local) == toy;
                self.emit(now, "play", json!({"toy": toy, "by": by, "fav": fav}));
                yes()
            }
            "play_hit" => {
                // the screen running the mini-game reports each boop/pop/beat/tickle
                if self.s.activity.kind != "play" { return no("not_playing", "Not playing".into()); }
                if !self.s.activity.owner.is_empty() && self.s.activity.owner != screen { return no("not_owner", "Another screen is running this game".into()); }
                if now - self.hits.0 > 1000.0 { self.hits = (now, 0); }
                self.hits.1 += 1;
                if self.hits.1 > 6 { return no("too_fast", "Too fast".into()); }
                let tired = if self.s.stats.rest < 20.0 { 0.5 } else { 1.0 };
                let fav = if self.s.activity.toy.as_deref() == Some(Brain::fav_toy(local)) { 1.5 } else { 1.0 };
                let (joy, fxv, fp): (f64, &Fx, f64) = match str_of(a, "hit") {
                    "boop" => (7.0, &BALL, 2.5),
                    "pop" => (5.0, &BUBBLES, 1.5),
                    "pop_user" => (3.0, &BUBBLES, 0.8),
                    "beat" => (1.6, &MUSIC, 0.0),
                    "tickle" => (3.0, &FEATHER, 1.0),
                    _ => return no("bad_hit", "Unknown hit".into()),
                };
                self.fx(&e1("joy", joy * fav * tired), 1.0);
                self.fx(fxv, 1.0);
                self.fp(fp);
                yes()
            }
            "play_end" => {
                if self.s.activity.kind == "play" && (self.s.activity.owner.is_empty() || self.s.activity.owner == screen || screen.is_empty()) { self.end_play(now, "done", ""); }
                yes()
            }
            "pet" => {
                if now < self.s.sulk_until - 1000.0 { return no("sulking", format!("{name} is still sulking")); }
                if self.s.asleep { return yes(); }
                if now - self.last_pet < 450.0 { return yes(); }
                self.last_pet = now;
                self.fx(&e1("joy", 1.5), 1.0); self.fp(1.0); self.grudge(now, -0.6);
                if now - self.last_pet_event > 3000.0 { self.last_pet_event = now; self.emit(now, "pet", json!({"by": by, "screen": screen})); }
                yes()
            }
            "boop" => {
                // a tap on an awake Ember
                if self.s.asleep || now < self.s.sulk_until { return yes(); }
                self.fx(&e1("joy", 1.0), 1.0); self.fp(0.5);
                yes()
            }
            "nice" => {
                // being kind to Mandoo cheers Ember up too
                self.fx(&e1("joy", 1.0), 1.0); self.grudge(now, -1.0);
                yes()
            }
            "tidy" => {
                if self.s.activity.kind == "cowork" { return no("busy", "Not now, you're in a session".into()); }
                if self.room_fires(&room) > 0 { return no("fire", "Put the fire out first!".into()); }
                if self.busy() { return no("busy", format!("{name} is busy")); }
                if self.s.stats.tidy >= 97.0 { return no("clean", "Already spotless".into()); }
                if let Some(t) = self.focus() { let left = ((t.end - now) / 60000.0).ceil(); return no("focus", format!("Let {name} nap, {left} min left")); }
                let woke = self.s.asleep;
                if woke { self.s.asleep = false; }
                self.s.activity = Activity { kind: "tidy".into(), since: now, until: now + TIDY_SECONDS * 1000.0, room: room.clone(), ..Activity::default() };
                self.emit(now, "tidy", json!({"by": by, "room": room, "woke": woke}));
                yes()
            }
            "spray" => {
                // {target:"fire", x, y}  |  {target:"pet"}  |  nothing: the biggest fire here, else Ember if misbehaving
                let target = str_of(a, "target");
                if target == "fire" || target.is_empty() {
                    let x = a.get("x").and_then(|v| v.as_i64()).map(|v| v as i32);
                    let y = a.get("y").and_then(|v| v.as_i64()).map(|v| v as i32);
                    let pick = match (x, y) {
                        (Some(x), Some(y)) => self.s.fires.iter().position(|f| f.room == room && f.x == x && f.y == y),
                        _ => self.s.fires.iter().enumerate().filter(|(_, f)| f.room == room)
                            .max_by(|a, b| a.1.size.partial_cmp(&b.1.size).unwrap_or(std::cmp::Ordering::Equal)).map(|(i, _)| i),
                    };
                    if let Some(i) = pick { self.douse_fire(now, i); return yes(); }
                    if target == "fire" { return no("missed", "Missed".into()); }
                }
                if target == "pet" || self.s.activity.kind == "mischief" { self.squirt_pet(now); return yes(); }
                no("nothing", "Nothing to spray".into())
            }
            "sleep" => {
                if self.s.asleep { return yes(); }
                if self.focus().is_some() { return no("focus", format!("{name} is in a focus session")); }
                if self.busy() { return no("busy", format!("{name} is busy")); }
                self.s.timer = None;
                self.go_sleep(now, "tucked_in");
                yes()
            }
            "wake" => {
                if self.focus().is_some_and(|t| t.act.as_deref() == Some("nap")) { return no("focus", format!("{name} is in a focus session")); }
                self.wake(now, true);
                yes()
            }
            "session_start" => {
                let m = a.get("minutes").and_then(|v| v.as_f64()).unwrap_or(25.0).round().clamp(1.0, 240.0);
                let act = str_of(a, "act");
                let act = if ACTS.contains(&act) { act.to_string() } else { "work".to_string() };
                if self.s.activity.kind != "idle" && self.s.activity.kind != "cowork" { self.idle(); }
                self.s.last_nap = m; self.s.last_act = act.clone();
                self.s.timer = Some(Timer { mode: "focus".into(), act: Some(act.clone()), start: now, end: now + m * 60000.0, total: m * 60000.0 });
                if act == "nap" { self.go_sleep(now, "focus_nap"); }
                else { if self.s.asleep { self.s.asleep = false; } self.s.activity = Activity { kind: "cowork".into(), since: now, ..Activity::default() }; }
                self.emit(now, "session_start", json!({"act": act, "min": m, "by": by}));
                yes()
            }
            "rest" => {
                // nap until rested (no timer)
                if self.busy() && self.s.activity.kind != "cowork" { return no("busy", format!("{name} is busy")); }
                self.s.timer = None;
                self.go_sleep(now, "until_rested");
                yes()
            }
            "session_stop" => {
                let Some(t) = self.s.timer.clone() else { return yes() };
                self.s.timer = None;
                if t.mode == "focus" {
                    let done = (now - t.start) / 60000.0;
                    let act = t.act.clone().unwrap_or_else(|| "nap".into());
                    self.log_session(t.start, done, &act, Some("stopped"));
                    if act == "nap" || self.s.asleep { self.wake(now, true); }
                    if self.s.activity.kind == "cowork" { self.idle(); }
                    self.emit(now, "session_stopped", json!({"min": done.floor(), "by": by}));
                } else {
                    self.emit(now, "break_end", json!({}));
                }
                yes()
            }
            "break_start" => {
                let m = a.get("minutes").and_then(|v| v.as_f64()).unwrap_or(5.0).round().clamp(1.0, 60.0);
                self.s.timer = Some(Timer { mode: "break".into(), act: None, start: now, end: now + m * 60000.0, total: m * 60000.0 });
                if self.s.asleep { self.wake(now, false); }
                if self.s.activity.kind == "cowork" { self.idle(); }
                self.emit(now, "break_start", json!({"min": m}));
                yes()
            }
            "rate" => {
                // rate the newest session that hasn't been rated yet
                let rating = str_of(a, "rating");
                if !["great", "ok", "meh"].contains(&rating) { return no("bad_rating", "Unknown rating".into()); }
                let note: String = str_of(a, "note").trim().chars().take(140).collect();
                if let Some(e) = self.s.log.iter_mut().take(3).find(|e| e.rating.is_none()) {
                    e.rating = Some(rating.into()); e.note = note; self.s.log_rev += 1;
                }
                yes()
            }
            "mischief" => {
                if self.s.prefs.mischief == "off" { return no("off", "Mischief is turned off".into()); }
                if !self.can_misbehave(now) { return no("busy", format!("{name} is busy")); }
                self.start_mischief(now, false);
                yes()
            }
            "prefs" => {
                if let Some(m) = a.get("mischief").and_then(|v| v.as_str()) {
                    if ["off", "rare", "normal", "often"].contains(&m) { self.s.prefs.mischief = m.into(); self.schedule(1.0); }
                }
                if let Some(f) = a.get("fires").and_then(|v| v.as_bool()) { self.s.prefs.fires = f; if !f { self.s.fires.clear(); } }
                yes()
            }
            "rename" => {
                let n: String = str_of(a, "name").split_whitespace().collect::<Vec<_>>().join(" ").chars().take(24).collect();
                self.s.name = if n.is_empty() { "Ember".into() } else { n };
                yes()
            }
            "ignite" => {
                // dev/testing hook; also lets Home Assistant automations stir things up
                let r = str_of(a, "at_room"); let r = if r.is_empty() { room.clone() } else { r.to_string() };
                self.ignite(now, &r, None);
                yes()
            }
            _ => no("unknown", format!("Unknown action '{kind}'")),
        }
    }

    // -----------------------------------------------------------------------------------------
    // reading
    // -----------------------------------------------------------------------------------------
    /// Everything screens need to draw Ember, minus the logbook (fetch that when `logRev` changes).
    pub fn snapshot(&self, local: Local) -> Value {
        let mut v = serde_json::to_value(&self.s).unwrap_or_default();
        if let Value::Object(m) = &mut v {
            m.remove("log");
            m.insert("level".into(), json!(self.level()));
            m.insert("favToy".into(), json!(Brain::fav_toy(local)));
            m.insert("chaosPct".into(), json!(((self.s.chaos / self.s.chaos_at).min(1.0) * 100.0).round()));
            m.insert("mood".into(), json!(self.mood()));
        }
        v
    }
    pub fn log(&self) -> Value { json!({"rev": self.s.log_rev, "log": self.s.log}) }
    /// Events after `seq` (oldest first).
    pub fn events_since(&self, seq: u64) -> Vec<Event> { self.events.iter().filter(|e| e.seq > seq).cloned().collect() }
    /// The full state, for saving to disk / localStorage.
    pub fn export(&self) -> Value { serde_json::to_value(&self.s).unwrap_or_default() }
    pub fn mood(&self) -> &'static str {
        if self.s.asleep { return "sleeping"; }
        if self.s.activity.kind == "mischief" { return "mischievous"; }
        let (k, v) = self.s.stats.lowest();
        if self.s.grudge >= 60.0 && v >= 20.0 { return "resentful"; }
        if v < 20.0 { return match k { "fuel" => "hungry", "joy" => "bored", "rest" => "sleepy", _ => "messy" }; }
        let s = &self.s.stats;
        let avg = (s.fuel + s.joy + s.rest + s.tidy) / 4.0;
        if avg > 82.0 { "glowing" } else if avg > 62.0 { "cheerful" } else if avg > 42.0 { "content" } else { "grumpy" }
    }
    /// Short public summary for the API and phone remote.
    pub fn summary(&self) -> Value {
        let s = &self.s;
        json!({
            "name": s.name, "mood": self.mood(), "friendship": self.level(),
            "stats": {"fuel": s.stats.fuel.round(), "joy": s.stats.joy.round(), "rest": s.stats.rest.round(), "tidy": s.stats.tidy.round()},
            "asleep": s.asleep, "activity": if s.asleep { "sleep" } else { s.activity.kind.as_str() }, "toy": s.activity.toy,
            "room": s.room, "misbehaving": s.activity.kind == "mischief", "grudge": s.grudge.round(),
            "chaos": ((s.chaos / s.chaos_at).min(1.0) * 100.0).round(), "fires": s.fires.len(),
            "session": s.timer.as_ref().map(|t| json!({"mode": t.mode, "activity": t.act, "ends": t.end})),
        })
    }
}

// =============================================================================================
// WebAssembly interface (used by the page when there's no server): JSON in, JSON out.
// =============================================================================================
#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use std::cell::RefCell;
    thread_local! { static BRAIN: RefCell<Option<Brain>> = const { RefCell::new(None) }; static OUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) }; }

    #[no_mangle] pub extern "C" fn alloc(len: usize) -> *mut u8 { let mut v = Vec::<u8>::with_capacity(len); let p = v.as_mut_ptr(); std::mem::forget(v); p }
    #[no_mangle] pub unsafe extern "C" fn dealloc(p: *mut u8, len: usize) { drop(Vec::from_raw_parts(p, 0, len)); }
    #[no_mangle] pub extern "C" fn out_ptr() -> *const u8 { OUT.with(|o| o.borrow().as_ptr()) }
    #[no_mangle] pub extern "C" fn out_len() -> usize { OUT.with(|o| o.borrow().len()) }

    /// One entry point: `{"op":"init"|"tick"|"act"|"snapshot"|"events"|"log"|"export", "now":ms, "hour":h, "mday":d, ...}`.
    #[no_mangle] pub unsafe extern "C" fn call(p: *const u8, len: usize) {
        let input = std::slice::from_raw_parts(p, len);
        let req: Value = serde_json::from_slice(input).unwrap_or_default();
        let now = req["now"].as_f64().unwrap_or(0.0);
        let local = Local { hour: req["hour"].as_f64().unwrap_or(12.0), mday: req["mday"].as_u64().unwrap_or(1) as u32 };
        let res = BRAIN.with(|b| {
            let mut b = b.borrow_mut();
            if req["op"] == "init" { *b = Some(Brain::new(req.get("state").cloned(), now)); return json!({"ok": true}); }
            let Some(br) = b.as_mut() else { return json!({"error": "not initialised"}) };
            match req["op"].as_str().unwrap_or("") {
                "tick" => { br.tick(now, local); json!({"ok": true}) }
                "act" => br.act(&req["action"], now, local),
                "snapshot" => br.snapshot(local),
                "events" => serde_json::to_value(br.events_since(req["since"].as_u64().unwrap_or(0))).unwrap_or_default(),
                "log" => br.log(),
                "export" => br.export(),
                _ => json!({"error": "unknown op"}),
            }
        });
        OUT.with(|o| *o.borrow_mut() = serde_json::to_vec(&res).unwrap_or_default());
    }
}

// =============================================================================================
#[cfg(test)]
mod tests {
    use super::*;
    const DAY: Local = Local { hour: 12.0, mday: 3 };
    const NIGHT: Local = Local { hour: 23.0, mday: 3 };
    fn act(b: &mut Brain, now: f64, v: Value) -> Value { b.act(&v, now, DAY) }
    fn run(b: &mut Brain, from: f64, secs: f64, local: Local) -> f64 { let mut t = from; while t < from + secs * 1000.0 { t += 250.0; b.tick(t, local); } t }
    fn kinds(b: &Brain) -> Vec<String> { b.events_since(0).into_iter().map(|e| e.kind).collect() }

    #[test]
    fn migrates_old_page_save() {
        let old = json!({"name":"Sparky","stats":{"fuel":50,"joy":60,"rest":70,"tidy":80},"fp":250,"asleep":false,
            "settings":{"mischief":"off","fires":"off","room":"porch"},"ents":[{"x":1}],"last":1000.0,"lastTidy":null,
            "log":[{"start":5.0,"min":25,"act":"work","rating":"great","note":"hi"}],"timer":null,"fires":[]});
        let b = Brain::new(Some(old), 1000.0);
        assert_eq!(b.s.name, "Sparky");
        assert_eq!(b.s.stats.fuel, 50.0);
        assert_eq!(b.level(), 2);
        assert_eq!(b.s.prefs.mischief, "off");
        assert!(!b.s.prefs.fires);
        assert_eq!(b.s.log.len(), 1);
    }

    #[test]
    fn feeding_takes_time_then_fills_fuel() {
        let mut b = Brain::new(None, 0.0);
        b.s.stats.fuel = 40.0;
        assert_eq!(act(&mut b, 1000.0, json!({"type":"feed","food":"pinecone"}))["ok"], true);
        assert_eq!(b.s.activity.kind, "eat");
        assert_eq!(act(&mut b, 1100.0, json!({"type":"feed"}))["code"], "busy");
        run(&mut b, 1100.0, 4.0, DAY);
        assert_eq!(b.s.activity.kind, "idle");
        assert!(b.s.stats.fuel > 60.0, "fuel {}", b.s.stats.fuel);
        assert!(kinds(&b).contains(&"ate".to_string()));
    }

    #[test]
    fn full_ember_refuses_food_but_not_pepper() {
        let mut b = Brain::new(None, 0.0);
        b.s.stats.fuel = 99.0;
        assert_eq!(act(&mut b, 10.0, json!({"type":"feed"}))["code"], "full");
        assert_eq!(act(&mut b, 20.0, json!({"type":"feed","food":"pepper"}))["ok"], true);
        run(&mut b, 20.0, 4.0, DAY);
        assert_eq!(b.s.activity.kind, "mischief");
        assert!(b.s.activity.frenzy);
    }

    #[test]
    fn cannot_play_when_hungry_or_sleepy() {
        let mut b = Brain::new(None, 0.0);
        b.s.stats.fuel = 10.0;
        assert_eq!(act(&mut b, 10.0, json!({"type":"play","toy":"ball"}))["code"], "hungry");
        b.s.stats.fuel = 90.0; b.s.stats.rest = 10.0;
        assert_eq!(act(&mut b, 20.0, json!({"type":"play","toy":"ball"}))["code"], "sleepy");
        b.s.stats.rest = 90.0; b.s.fp = 50.0;
        assert_eq!(act(&mut b, 30.0, json!({"type":"play","toy":"feather"}))["code"], "locked");
    }

    #[test]
    fn only_the_game_screen_scores_hits() {
        let mut b = Brain::new(None, 0.0);
        act(&mut b, 10.0, json!({"type":"play","toy":"ball","screen":"A"}));
        let joy = b.s.stats.joy;
        assert_eq!(act(&mut b, 20.0, json!({"type":"play_hit","hit":"boop","screen":"B"}))["ok"], false);
        assert_eq!(act(&mut b, 30.0, json!({"type":"play_hit","hit":"boop","screen":"A"}))["ok"], true);
        assert!(b.s.stats.joy > joy);
        act(&mut b, 40.0, json!({"type":"play_end","screen":"A"}));
        assert_eq!(b.s.activity.kind, "idle");
    }

    #[test]
    fn mischief_spreads_fire_and_spray_stops_it() {
        let mut b = Brain::new(None, 0.0);
        b.s.room = "porch".into();
        assert_eq!(act(&mut b, 10.0, json!({"type":"mischief","room":"porch"}))["ok"], true);
        let mut t = 10.0;
        for _ in 0..200 { t += 250.0; b.tick(t, DAY); if !b.s.fires.is_empty() { break; } }
        b.s.fires.push(Fire { room: "porch".into(), x: 43, y: 57, size: 1.0 });
        b.s.fires.dedup_by(|a, b| a.x == b.x && a.y == b.y);
        assert!(b.s.fires.iter().all(|f| f.room == "porch"));
        // put every fire out
        while let Some(f) = b.s.fires.first().cloned() { t += 10.0; act(&mut b, t, json!({"type":"spray","target":"fire","x":f.x,"y":f.y,"room":"porch"})); }
        assert!(b.s.fires.is_empty());
        assert!(!b.s.scorch.is_empty());
        // squirt Ember until it stops
        while b.s.activity.kind == "mischief" { t += 10.0; act(&mut b, t, json!({"type":"spray","target":"pet"})); }
        assert!(b.s.grudge > 0.0);
        assert!(kinds(&b).contains(&"mischief_end".to_string()));
    }

    #[test]
    fn no_mischief_while_asleep_even_with_full_meter() {
        let mut b = Brain::new(None, 0.0);
        act(&mut b, 10.0, json!({"type":"sleep"}));
        b.s.chaos = 99.0; b.s.chaos_at = 0.1;
        run(&mut b, 10.0, 5.0, DAY);
        assert!(b.s.asleep);
        assert_eq!(b.s.activity.kind, "idle");
    }

    #[test]
    fn chaos_meter_triggers_mischief_when_awake() {
        let mut b = Brain::new(None, 0.0);
        b.s.prefs.mischief = "often".into(); b.s.chaos_at = 0.01;
        run(&mut b, 0.0, 10.0, DAY);
        assert!(kinds(&b).contains(&"mischief_start".to_string()));
    }

    #[test]
    fn tidying_is_grumpy_and_cleans_scorch_in_that_room() {
        let mut b = Brain::new(None, 0.0);
        b.s.stats.tidy = 40.0;
        b.s.scorch.push(Mark { room: "hearth".into(), x: 1, y: 1 });
        b.s.scorch.push(Mark { room: "porch".into(), x: 1, y: 1 });
        assert_eq!(act(&mut b, 10.0, json!({"type":"tidy","room":"hearth"}))["ok"], true);
        let t = run(&mut b, 10.0, 6.0, DAY);
        assert!(b.s.stats.tidy > 99.0);
        assert!(b.s.grudge >= 20.0);
        assert_eq!(b.s.scorch.len(), 1);
        assert!(b.s.sulk_until > t);
        assert_eq!(act(&mut b, t + 10.0, json!({"type":"pet"}))["code"], "sulking");
    }

    #[test]
    fn focus_session_logs_and_rates() {
        let mut b = Brain::new(None, 0.0);
        act(&mut b, 0.0, json!({"type":"session_start","minutes":1,"act":"study"}));
        assert_eq!(b.s.activity.kind, "cowork");
        run(&mut b, 0.0, 61.0, DAY);
        assert!(b.s.timer.is_none());
        assert_eq!(b.s.log[0].act, "study");
        assert!(b.s.log[0].rating.is_none());
        act(&mut b, 62_000.0, json!({"type":"rate","rating":"great","note":"chapter 3"}));
        assert_eq!(b.s.log[0].rating.as_deref(), Some("great"));
        assert_eq!(b.s.log[0].note, "chapter 3");
    }

    #[test]
    fn falls_asleep_at_night_when_left_alone_and_wakes_rested_in_the_morning() {
        let mut b = Brain::new(None, 0.0);
        b.s.prefs.mischief = "off".into();
        run(&mut b, 0.0, 130.0, NIGHT);
        assert!(b.s.asleep);
        b.s.stats.rest = 100.0;
        run(&mut b, 130_000.0, 1.0, DAY);
        assert!(!b.s.asleep);
    }

    #[test]
    fn time_away_is_caught_up_gently() {
        let mut b = Brain::new(None, 0.0);
        let fuel = b.s.stats.fuel;
        b.tick(3_600_000.0, DAY);
        assert!(b.s.stats.fuel < fuel && b.s.stats.fuel > fuel - 30.0);
    }

    #[test]
    fn export_round_trips() {
        let mut b = Brain::new(None, 0.0);
        act(&mut b, 10.0, json!({"type":"rename","name":"  Cinder  "}));
        let b2 = Brain::new(Some(b.export()), 20.0);
        assert_eq!(b2.s.name, "Cinder");
        assert_eq!(b2.s.stats, b.s.stats);
    }
}
