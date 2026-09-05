// pyblockMiner — PyBLØCK LOTTO BLAKE2b miner (Bitcoin BLAKE2b, solo lottery). Rust + ratatui TUI.
// Tabbed app: MINE dashboard · DATA (session analytics) · STRATUMS (switch pools live) · LEARN · NETWORK · SETUP · HELP.
// Pool MODES (same chain, different coinbase): LOTTO solo · CHIRP syndicate (MINE lists EVERY miner in the coinbase
// draw + your slice) · CAROUSEL rotating supplier templates (shows the live template). Detected from the stratum port.
// `p` pauses/resumes mining from any tab (GPU/CPU idle, pool stays connected).
// Native SV1 stratum client + N-GPU/CPU BLAKE2b grinding via persistent gpu_grind daemons.
// You mine to YOUR address → keep 99.1% of every block · PyBLØCK pool fee 0.9%. Non-custodial.
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use serde::{Serialize, Deserialize};
use serde_json::{json, Value};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, List, ListItem, Paragraph, Sparkline, Wrap};

// ── PyBLØCK hashrate donation (like xmrig, but paid in HASH not satoshis; mainnet only) ──
const DONATE_POOL: &str = "pool.pyblock.xyz:4445";
const DEV_DONATION_ADDR: &str = "1PyBLoCKdiaC46vD9CWcmxa3ey2VzSc5Q2";
const DONATE_MIN: f64 = 2.0;
const VERSION: &str = env!("CARGO_PKG_VERSION");   // from Cargo.toml — shown in TUI footer, --version, and the stratum UA

// PyBLØCK palette
const GRN: Color = Color::Rgb(0, 255, 65);
const YLW: Color = Color::Rgb(255, 255, 0);
const CYN: Color = Color::Rgb(0, 255, 255);
const MUT: Color = Color::Rgb(130, 154, 130);
const AMB: Color = Color::Rgb(224, 176, 53);
const PNK: Color = Color::Rgb(255, 92, 200);
const BRD: Color = Color::Rgb(35, 60, 35);
const PUR: Color = Color::Rgb(185, 107, 255);   // CHIRP accent — the pool site's violet
const WHT: Color = Color::Rgb(236, 236, 244);   // CAROUSEL accent + primary values (soft white)
const DIM: Color = Color::Rgb(58, 70, 58);      // quiet card borders — the frame recedes, the numbers speak

// ── network config: mainnet | testnet4 | regtest ──
struct NetCfg { name: &'static str, donate: bool }
fn net_cfg(net: &str) -> NetCfg {
    match net {
        "testnet4" | "testnet" | "t4" => NetCfg { name: "testnet4", donate: false },
        "regtest"  | "reg"            => NetCfg { name: "regtest",  donate: false },
        _                             => NetCfg { name: "mainnet",  donate: true },
    }
}
fn addr_ok(net: &str, a: &str) -> bool {
    match net {
        "testnet4" => a.starts_with("tb1")   || a.starts_with('m') || a.starts_with('n') || a.starts_with('2'),
        "regtest"  => a.starts_with("bcrt1") || a.starts_with('m') || a.starts_with('n') || a.starts_with('2'),
        _          => a.starts_with("bc1")   || a.starts_with('1') || a.starts_with('3'),
    }
}
// public network-stats endpoint per network (regtest → the :23110 demo pool's stats, same as mainnet)
fn net_stats_url(net: &str) -> Option<&'static str> {
    match net {
        "testnet4" => Some("https://pool.pyblock.xyz:8443/api/blake_stats_t4.php"),
        _          => Some("https://pool.pyblock.xyz:8443/api/blake_stats.php"),
    }
}

// ── a stratum (pool) entry: PyBLØCK defaults + user's custom ones ──
#[derive(Serialize, Deserialize, Clone)]
struct Stratum { name: String, url: String, network: String, #[serde(default)] custom: bool }

fn default_stratums() -> Vec<Stratum> {
    vec![
        // 3 pools mainnet BLAKE2b (misma cadena, distinto puerto/modo — se elige en la tab STRATUMS):
        Stratum { name: "PyBLØCK · LOTTO".into(),    url: "pool.pyblock.xyz:4445".into(),  network: "mainnet".into(),  custom: false },
        Stratum { name: "PyBLØCK · CHIRP".into(),    url: "pool.pyblock.xyz:5574".into(),  network: "mainnet".into(),  custom: false },
        Stratum { name: "PyBLØCK · CAROUSEL".into(), url: "pool.pyblock.xyz:30110".into(), network: "mainnet".into(),  custom: false },
        Stratum { name: "PyBLØCK · testnet4".into(), url: "pool.pyblock.xyz:23111".into(), network: "testnet4".into(), custom: false },
        Stratum { name: "PyBLØCK · regtest".into(),  url: "pool.pyblock.xyz:23110".into(), network: "regtest".into(),  custom: false },
    ]
}

// ── pool MODE: what the coinbase does on this stratum. Same BLAKE2b chain, different payout rules.
//    Detected from the port (PyBLØCK convention: 4445 LOTTO · 5574 CHIRP · 30110 CAROUSEL) or the name. ──
#[derive(Clone, Copy, PartialEq, Default, Debug)]
enum PoolMode { #[default] Lotto, Chirp, Carousel, Custom }
fn pool_mode(url: &str, name: &str) -> PoolMode {
    let port = url.rsplit(':').next().unwrap_or("");
    let n = name.to_ascii_uppercase();
    match port {
        "5574" | "5554"            => PoolMode::Chirp,
        "30110" | "30000"          => PoolMode::Carousel,
        "4445" | "23111" | "23110" => PoolMode::Lotto,
        _ if n.contains("CHIRP")   => PoolMode::Chirp,
        _ if n.contains("CAROUSEL") || n.contains("CARROUSEL") => PoolMode::Carousel,
        _ if n.contains("LOTTO")   => PoolMode::Lotto,
        _                          => PoolMode::Custom,
    }
}
impl PoolMode {
    fn label(self) -> &'static str { match self { PoolMode::Lotto => "LOTTO", PoolMode::Chirp => "CHIRP", PoolMode::Carousel => "CAROUSEL", PoolMode::Custom => "CUSTOM" } }
    fn icon(self) -> &'static str { match self { PoolMode::Lotto => "🎰", PoolMode::Chirp => "🌌", PoolMode::Carousel => "🎠", PoolMode::Custom => "⛏" } }
    fn accent(self) -> Color { match self { PoolMode::Lotto => YLW, PoolMode::Chirp => PUR, PoolMode::Carousel => WHT, PoolMode::Custom => CYN } }
    fn tagline(self) -> &'static str {
        match self { PoolMode::Lotto => "solo lottery", PoolMode::Chirp => "syndicate · weighted split", PoolMode::Carousel => "rotating clean templates", PoolMode::Custom => "custom stratum" }
    }
    // who gets paid — one honest line, shown in STRATUMS and in the MINE header
    fn payout(self) -> &'static str {
        match self {
            PoolMode::Lotto    => "every block you find pays YOUR address · you keep 99.1% · PyBLØCK fee 0.9%",
            PoolMode::Chirp    => "every block is split on-chain among ALL eligible miners by weight · 7-day loyalty · fee 0.9%",
            PoolMode::Carousel => "you mine independent suppliers' clean templates · finder keeps 98% · supplier 1% · PyBLØCK 1%",
            PoolMode::Custom   => "payout rules are the pool operator's — check their site",
        }
    }
    fn payout_short(self) -> &'static str {
        match self {
            PoolMode::Lotto => "keep 99.1% · fee 0.9%", PoolMode::Chirp => "weighted split · fee 0.9%",
            PoolMode::Carousel => "keep 98% · supplier 1% · fee 1%", PoolMode::Custom => "operator's rules",
        }
    }
}

// ── CHIRP syndicate: everyone in the coinbase draw. Source: pool.pyblock.xyz chirp_api.php (chain=blake2b) ──
#[derive(Clone, Default)]
struct ChirpMember { addr: String, days: f64, power: f64, weight: f64, eligible: bool, last_seen: u64 }
#[derive(Clone, Default)]
struct ChirpInfo {
    members: Vec<ChirpMember>,   // sorted: eligible by weight desc, then the rest by tenure desc
    candidates: u64, workers: u64, blocks: u64, hashrate_ths: f64, min_days: f64, min_power: f64,
    reward_sats: u64, height: u64, fee_bps: u64, fetched: u64,
}
impl ChirpInfo {
    fn sum_weight(&self) -> f64 { self.members.iter().filter(|m| m.eligible).map(|m| m.weight).sum() }
    fn me(&self, addr: &str) -> Option<&ChirpMember> { if addr.is_empty() { None } else { self.members.iter().find(|m| m.addr == addr) } }
    // your share of the next coinbase (0..100) — only if you're eligible
    fn my_pct(&self, addr: &str) -> Option<f64> {
        let m = self.me(addr)?; let s = self.sum_weight();
        if m.eligible && s > 0.0 { Some(m.weight / s * 100.0) } else { None }
    }
}
// ── CAROUSEL: independent suppliers' clean templates in rotation. Source: b.pyblock.xyz carousel.php?carrousel=1 ──
#[derive(Clone, Default)]
struct CarouselInfo { suppliers: Vec<String>, current: String, recent: Vec<String>, miners: u64, hashrate_ths: f64, live: bool, fetched: u64 }

// ── persisted config ──
#[derive(Serialize, Deserialize)]
struct Config {
    #[serde(default = "default_stratums")] stratums: Vec<Stratum>,
    #[serde(default)] selected: usize,                 // index into stratums
    #[serde(default)] addrs: HashMap<String, String>,  // network -> address
    #[serde(default = "d_donate")] donate: f64,
    #[serde(default)] gpus: Option<u32>,
    #[serde(default)] cpu: bool,
    #[serde(default)] worker: String,                  // stratum worker suffix → login "addr.worker" (tells your rigs apart on the pool)
    #[serde(default = "d_true")] log_file: bool,       // append every log line to <config dir>/miner.log
    #[serde(default)] api_port: u16,                   // 0 = off · else JSON at http://127.0.0.1:<port>/ and Prometheus at /metrics
    #[serde(default)] alerts: AlertCfg,
}
// ── alerts: what a miner wants to know without watching the screen ──
#[derive(Serialize, Deserialize, Clone)]
struct AlertCfg {
    #[serde(default = "d_true")] bell: bool,           // terminal bell
    #[serde(default = "d_true")] desktop: bool,        // notify-send (Linux) / osascript (macOS)
    #[serde(default)] telegram_token: String,          // Bot API token · with telegram_chat → sendMessage
    #[serde(default)] telegram_chat: String,
    #[serde(default)] webhook_url: String,             // POST {source,title,body,ts} as JSON
}
impl Default for AlertCfg { fn default() -> Self { AlertCfg { bell: true, desktop: true, telegram_token: String::new(), telegram_chat: String::new(), webhook_url: String::new() } } }
fn d_donate() -> f64 { DONATE_MIN }
fn d_true() -> bool { true }
impl Default for Config {
    fn default() -> Self {
        Config { stratums: default_stratums(), selected: 0, addrs: HashMap::new(), donate: DONATE_MIN, gpus: None, cpu: false,
                 worker: String::new(), log_file: true, api_port: 0, alerts: AlertCfg::default() }
    }
}
// worker names travel inside the stratum username → keep them plain: [A-Za-z0-9-_], max 24
fn clean_worker(s: &str) -> String { s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(24).collect() }
fn login(addr: &str, worker: &str) -> String { if worker.is_empty() { addr.to_string() } else { format!("{}.{}", addr, worker) } }
fn config_path() -> PathBuf {
    // Cross-platform config dir. On Windows HOME/XDG are usually unset → the old code fell back to "." (CWD),
    // so saves landed next to wherever the exe was launched and looked like they "didn't save". Prefer the
    // native Windows locations (%APPDATA%, then %USERPROFILE%\.config) before the CWD fallback.
    let base = std::env::var("XDG_CONFIG_HOME").ok()
        .or_else(|| std::env::var("APPDATA").ok())                                        // Windows: %APPDATA%\Roaming
        .or_else(|| std::env::var("HOME").ok().map(|h| format!("{}/.config", h)))         // Linux / macOS
        .or_else(|| std::env::var("USERPROFILE").ok().map(|h| format!("{}/.config", h)))  // Windows fallback (no APPDATA)
        .unwrap_or_else(|| ".".into());
    PathBuf::from(base).join("pyblockminer").join("config.json")
}
fn load_config() -> Config {
    let mut c: Config = std::fs::read_to_string(config_path()).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    reconcile_defaults(&mut c);
    c
}
// Refresca los stratums DEFAULT (custom=false) contra default_stratums() para que un cambio de
// puerto/URL por versión (p.ej. mainnet :23110→:4445) llegue a configs ya guardados. Preserva
// los stratums custom, el índice seleccionado, addresses y donate.
fn reconcile_defaults(c: &mut Config) {
    let defs = default_stratums();
    let mut changed = false;
    // Clave = URL (identidad estable del pool). Antes era por NETWORK, pero ahora hay 3 pools mainnet
    // (LOTTO/CHIRP/CAROUSEL) con la MISMA network → habría que distinguirlos por URL. Actualiza el nombre
    // de los defaults guardados (p.ej. "PyBLØCK · mainnet" :4445 → "PyBLØCK · LOTTO") sin tocar los custom.
    for s in c.stratums.iter_mut() {
        if s.custom { continue; }
        if let Some(d) = defs.iter().find(|d| d.url == s.url && !d.custom) {
            if s.name != d.name || s.network != d.network { s.name = d.name.clone(); s.network = d.network.clone(); changed = true; }
        }
    }
    // Agrega los pools default que falten (por URL) → CHIRP/CAROUSEL llegan a configs ya guardados.
    for d in defs.iter() {
        if !c.stratums.iter().any(|s| !s.custom && s.url == d.url) { c.stratums.push(d.clone()); changed = true; }
    }
    if changed { save_config(c); }
}
fn save_config(c: &Config) {
    let p = config_path();
    if let Some(dir) = p.parent() { let _ = std::fs::create_dir_all(dir); }
    if let Ok(s) = serde_json::to_string_pretty(c) { let _ = std::fs::write(&p, s); }
}

// ── live target shared engine↔UI: switch pools/network without restarting ──
struct Target { pool: String, addr: String, worker: String, network: String, donate: f64 }

// ── locate the GPU grinder + kernel (Linux: gpu_grind + blake2b.cl · macOS: metal_grind + blake2b.metal) ──
fn gpu_dir() -> String {
    if let Ok(d) = std::env::var("PYBLOCK_GPU_DIR") { return d; }
    let has_kernel = |c: &Path| c.join("blake2b.cl").exists() || c.join("blake2b.metal").exists();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            for cand in [p.join("gpu"), p.join("../../gpu"), p.to_path_buf()] {
                if has_kernel(&cand) { return cand.to_string_lossy().into_owned(); }
            }
        }
    }
    if has_kernel(Path::new("gpu")) { return "gpu".into(); }
    ".".into()
}
fn gpu_bin() -> String {
    if let Ok(b) = std::env::var("PYBLOCK_GPU_BIN") { return b; }
    let base = if cfg!(target_os = "macos") { "metal_grind" } else { "gpu_grind" };
    let name = format!("{}{}", base, std::env::consts::EXE_SUFFIX);   // ".exe" on Windows, "" on Linux/macOS
    let cand = format!("{}/{}", gpu_dir(), name);
    if Path::new(&cand).exists() { cand } else { name }
}
// Run `<bin> <args…>` and collect stdout lines, but KILL it if it doesn't finish in `secs`.
// std has no wait-with-timeout, so: read stdout on a thread, recv_timeout, kill on timeout.
// Why this matters: a broken OpenCL stack (e.g. a ROCm ICD that doesn't support the card, or a
// rocm+mesa ICD conflict) makes clGetPlatformIDs/clGetDeviceIDs HANG — a plain .output() would then
// block forever at startup and the miner "never starts" (both GPU and CPU), with no error shown.
fn run_lines_timeout(bin: &str, args: &[&str], dir: &str, secs: u64) -> Option<Vec<String>> {
    let mut child = Command::new(bin).args(args).current_dir(dir)
        .stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || { let mut s = String::new(); let _ = out.read_to_string(&mut s); let _ = tx.send(s); });
    match rx.recv_timeout(Duration::from_secs(secs)) {
        Ok(s) => {
            let _ = child.wait();
            Some(s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        }
        Err(_) => { let _ = child.kill(); let _ = child.wait(); None }   // hung → kill it, caller falls back (no GPU) instead of freezing
    }
}
fn gpu_names() -> Vec<String> {
    #[cfg(target_os = "macos")]
    { vec!["Apple GPU (Metal)".to_string()] }   // Apple Silicon = one integrated GPU; metal_grind's READY has the exact name
    #[cfg(not(target_os = "macos"))]
    {
        // Ask the grinder to enumerate OpenCL GPUs (NVIDIA / AMD / Intel) — the SAME global list it will drive,
        // so detection matches what actually mines. Timeout-guarded so a hung OpenCL stack can't freeze startup.
        // Falls back to nvidia-smi if the grinder isn't built yet (also timeout-guarded).
        if let Some(names) = run_lines_timeout(&gpu_bin(), &["list"], &gpu_dir(), 6) {
            if !names.is_empty() { return names; }
        }
        run_lines_timeout("nvidia-smi", &["--query-gpu=name", "--format=csv,noheader"], ".", 6).unwrap_or_default()
    }
}

#[derive(Default)]
struct Stats {
    connected: bool,
    endpoint: String,
    addr: String,
    donate: f64,
    donated: u64,
    diff: f64,
    best_diff: f64,
    bits: u32,
    gpu_ghs: Vec<f64>,
    gpu_names: Vec<String>,
    blocks: u64,
    accepted: u64,
    rejected: u64,
    hr_total: f64,
    hr_hist: VecDeque<u64>,
    log: VecDeque<String>,
    started: Option<Instant>,
    net_ok: bool,
    net_miners: u64,
    net_ghs: f64,
    net_height: u64,
    net_nbits: u32,     // network block target (compact nbits) — for the solo-lottery ETA in the DATA tab
    network: String,
    balance_ok: bool,
    balance_btc: f64,
    paused: bool,       // mirror of the shared pause flag → shown in the UI
    // BLAKE2b activation gating: None=unknown (grind, safe default), Some(false)=chain is still SHA-256d
    // (DON'T grind → save power), Some(true)=blake2b live. Set from the pool network-stats endpoint.
    blake2b_active: Option<bool>,
    activation_height: u64,
    blocks_until_act: u64,
    latest_version: String,   // newest published miner version (from the pool) → drives the update label
    update_available: bool,
    gpu_dead: Vec<bool>,               // per worker: grinder down, auto-respawning (shown in WORKERS)
    last_tick: Option<Instant>,        // engine heartbeat — the UI flags ENGINE STALLED if it stops while connected
    mode: PoolMode,                    // what the coinbase does on the active stratum (LOTTO / CHIRP / CAROUSEL)
    chirp: Option<ChirpInfo>,          // CHIRP: everyone in the coinbase draw (kept while polling, cleared on switch)
    carousel: Option<CarouselInfo>,    // CAROUSEL: templates in rotation + the one being mined right now
    worker: String,                    // worker suffix in use (header shows addr.worker)
    alerts: AlertCfg,                  // live alert settings (SETUP edits them; alert() reads them)
    ring_bell: bool,                   // set by alert(), consumed by the UI loop → \x07
    alerts_sent: u64,
    log_file: Option<std::fs::File>,   // miner.log (timestamped copy of every log line)
    api_port: u16,
}
impl Stats {
    fn logline(&mut self, s: String) {
        if let Some(f) = self.log_file.as_mut() { let _ = writeln!(f, "{}  {}", fmt_ts(now_unix()), s); }
        self.log.push_back(s);
        while self.log.len() > 300 { self.log.pop_front(); }
    }
}
// "2026-09-05 14:03:21" from unix seconds (civil-from-days, H. Hinnant) — no chrono for one timestamp
fn fmt_ts(t: u64) -> String {
    let (days, rem) = ((t / 86_400) as i64, t % 86_400);
    let z = days + 719_468; let era = z.div_euclid(146_097); let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1; let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}
// <config dir>/miner.log, append; rotated to miner.log.1 past 5 MB so it can never eat the disk
fn open_log_file() -> Option<std::fs::File> {
    let p = config_path().parent()?.join("miner.log");
    if let Some(dir) = p.parent() { let _ = std::fs::create_dir_all(dir); }
    if std::fs::metadata(&p).map(|m| m.len() > 5_000_000).unwrap_or(false) { let _ = std::fs::rename(&p, p.with_extension("log.1")); }
    std::fs::OpenOptions::new().create(true).append(true).open(&p).ok()
}
// Fire an alert: log line (🔔) + bell flag for the UI + desktop / Telegram / webhook on a throwaway thread so the
// engine never waits on the network. Takes the already-locked Stats so callers holding the lock can't deadlock.
fn alert(st: &mut Stats, title: &str, body: &str) {
    st.logline(format!("🔔 {} — {}", title, body));
    st.alerts_sent += 1;
    if st.alerts.bell { st.ring_bell = true; }
    let (cfg, title, body) = (st.alerts.clone(), title.to_string(), body.to_string());
    std::thread::spawn(move || {
        if cfg.desktop {
            #[cfg(target_os = "linux")]
            { let _ = Command::new("notify-send").args(["-a", "pyblockMiner", &format!("⛏ {}", title), &body]).stdout(Stdio::null()).stderr(Stdio::null()).status(); }
            #[cfg(target_os = "macos")]
            { let script = format!("display notification \"{}\" with title \"pyblockMiner · {}\"", body.replace('"', "'"), title.replace('"', "'"));
              let _ = Command::new("osascript").args(["-e", &script]).stdout(Stdio::null()).stderr(Stdio::null()).status(); }
        }
        if !cfg.telegram_token.is_empty() && !cfg.telegram_chat.is_empty() {
            let url = format!("https://api.telegram.org/bot{}/sendMessage", cfg.telegram_token);
            let payload = json!({"chat_id": cfg.telegram_chat, "text": format!("⛏ pyblockMiner · {}\n{}", title, body)}).to_string();
            let _ = ureq::post(&url).timeout(Duration::from_secs(10)).set("Content-Type", "application/json").send_string(&payload);
        }
        if !cfg.webhook_url.is_empty() {
            let payload = json!({"source": "pyblockMiner", "version": VERSION, "title": title, "body": body, "ts": now_unix()}).to_string();
            let _ = ureq::post(&cfg.webhook_url).timeout(Duration::from_secs(10)).set("Content-Type", "application/json").send_string(&payload);
        }
    });
}

fn floor_pot(diff: f64) -> u32 {
    let d = if diff >= 1.0 { diff as u64 } else { 1 };
    63u32.saturating_sub(d.leading_zeros())
}
fn b2b(data: &[u8]) -> [u8; 32] {
    let h = blake2b_simd::Params::new().hash_length(32).hash(data);
    let mut o = [0u8; 32];
    o.copy_from_slice(h.as_bytes());
    o
}
fn target_be(bits: u32) -> [u8; 32] {
    let mut base = [0u8; 32];
    for b in base.iter_mut().take(32).skip(4) { *b = 0xff; }
    let shb = (bits / 8) as usize;
    let sbit = bits % 8;
    let mut out = [0u8; 32];
    for i in 0..32 {
        let src = i as isize - shb as isize;
        let mut acc: u16 = 0;
        if src >= 0 { acc |= (base[src as usize] as u16) >> sbit; }
        if src - 1 >= 0 && sbit > 0 { acc |= ((base[(src - 1) as usize] as u16) << (8 - sbit)) & 0xff; }
        out[i] = acc as u8;
    }
    out
}
// network block target from the job's compact nbits (Bitcoin standard mantissa·256^(exp-3))
fn nbits_to_target(nbits: u32) -> [u8; 32] {
    let exp = (nbits >> 24) as i32;
    let mant = (nbits & 0x007fffff) as u64;
    let mut t = [0u8; 32];
    for i in 0..3i32 {
        let byte = ((mant >> (8 * (2 - i))) & 0xff) as u8;
        let pos = 32 - exp + i;
        if pos >= 0 && pos < 32 { t[pos as usize] = byte; }
    }
    t
}
// recompute the BLAKE2b PoW hash for an accepted nonce (to classify it: real block? best difficulty?)
fn nonce_hash(prevhash_hex: &str, ntime_hex: &str, work_root_hex: &str, nonce_hex: &str) -> Option<[u8; 32]> {
    let prevhash = hex::decode(prevhash_hex).ok()?;
    let mut ntime8 = hex::decode(ntime_hex).unwrap_or_default(); ntime8.resize(8, 0);
    let work_root = hex::decode(work_root_hex).ok()?;
    let nonce = u32::from_str_radix(nonce_hex, 16).ok()?;
    if prevhash.len() < 32 || work_root.len() < 32 { return None; }
    let mut work = [0u8; 80];
    work[..32].copy_from_slice(&prevhash[..32]);
    work[32..36].copy_from_slice(&nonce.to_le_bytes());
    work[40..48].copy_from_slice(&ntime8[..8]);
    work[48..80].copy_from_slice(&work_root[..32]);
    Some(b2b(&work))
}
fn hash_le_target(h: &[u8; 32], target: &[u8; 32]) -> bool {
    for i in 0..32 { if h[i] < target[i] { return true; } if h[i] > target[i] { return false; } }
    true
}
// difficulty of a hash vs the BLAKE2b diff-1 target (2^224-1): diff ≈ 2^96 / top128(hash)
fn hash_diff(h: &[u8; 32]) -> f64 {
    let mut top: u128 = 0;
    for i in 0..16 { top = (top << 8) | h[i] as u128; }
    if top == 0 { return f64::INFINITY; }
    (2f64).powi(96) / (top as f64)
}
// human-readable difficulty (best-share monitor)
fn fmt_diff(d: f64) -> String {
    if d <= 0.0 { "—".into() }
    else if d >= 1e12 { format!("{:.2}T", d / 1e12) }
    else if d >= 1e9 { format!("{:.2}G", d / 1e9) }
    else if d >= 1e6 { format!("{:.2}M", d / 1e6) }
    else if d >= 1e3 { format!("{:.2}K", d / 1e3) }
    else { format!("{:.2}", d) }
}

fn cpu_grind(prevhash_hex: &str, ntime_hex: &str, work_root_hex: &str, bits: u32, threads: usize, nstart: u64, span: u64) -> Vec<String> {
    let prevhash = hex::decode(prevhash_hex).unwrap_or_default();
    let mut ntime8 = hex::decode(ntime_hex).unwrap_or_default(); ntime8.resize(8, 0);
    let work_root = hex::decode(work_root_hex).unwrap_or_default();
    if prevhash.len() < 32 || work_root.len() < 32 { return vec![]; }
    let target = target_be(bits);
    let threads = threads.max(1);
    let per = span / threads as u64;
    let out = Mutex::new(Vec::<String>::new());
    std::thread::scope(|s| {
        for t in 0..threads {
            let a = nstart + t as u64 * per;
            let b = if t == threads - 1 { nstart + span } else { a + per };
            let (prevhash, ntime8, work_root, target, out) = (&prevhash, &ntime8, &work_root, &target, &out);
            s.spawn(move || {
                let mut local = vec![];
                let mut work = [0u8; 80];
                work[..32].copy_from_slice(&prevhash[..32]);
                work[40..48].copy_from_slice(&ntime8[..8]);
                work[48..80].copy_from_slice(&work_root[..32]);
                let mut n = a;
                while n < b && n < (1u64 << 32) {
                    work[32..36].copy_from_slice(&(n as u32).to_le_bytes());
                    let h = b2b(&work);
                    let mut win = true;
                    for i in 0..32 { if h[i] < target[i] { break; } if h[i] > target[i] { win = false; break; } }
                    if win { local.push(format!("{:08x}", n)); }
                    n += 1;
                }
                if !local.is_empty() { out.lock().unwrap().extend(local); }
            });
        }
    });
    out.into_inner().unwrap()
}

// One GPU grinder process. Its stdout is drained by a reader thread into `rx`, so the engine waits for results with a
// TIMEOUT — a hung OpenCL/Metal driver used to block read_line forever, freezing the whole engine while the UI kept
// saying LIVE. A daemon that dies or hangs is killed, marked dead, and RESPAWNED with backoff (10s → 5 min); before,
// a dead GPU stayed dead until the user restarted the miner.
struct Daemon {
    child: Child, stdin: ChildStdin, rx: std::sync::mpsc::Receiver<String>,
    name: String, dev: u32, weight: f64, dead: bool, died_at: Instant, since: Instant, fails: u32,
}
impl Daemon {
    fn kill(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); self.dead = true; self.weight = 0.0; self.died_at = Instant::now(); }
    fn retry_in(&self) -> Duration { Duration::from_secs((10u64 << self.fails.saturating_sub(1).min(5)).min(300)) }   // 10 · 20 · 40 · 80 · 160 · 300s
}
fn spawn_daemon(dev: u32, name: String) -> Option<Daemon> {
    let mut child = Command::new(gpu_bin())
        .args(["daemon", &dev.to_string()])
        .current_dir(gpu_dir())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().ok()?;
    let stdin = child.stdin.take()?;
    let stdout = BufReader::new(child.stdout.take()?);
    // Wait for the grinder's "READY <device>" on stderr before trusting it. Both grinders do all device/
    // kernel setup AFTER exec and exit on failure, so a spawn success alone doesn't mean a working GPU.
    // If it never signals READY (died/hung), drop it → the engine falls back to CPU instead of hashing zero.
    let stderr = child.stderr.take()?;
    let (tx, rx) = std::sync::mpsc::channel::<Option<String>>();
    std::thread::spawn(move || {
        let mut line = String::new();
        let got = BufReader::new(stderr).read_line(&mut line).ok().filter(|&n| n > 0).map(|_| line.trim().to_string());
        let _ = tx.send(got);
    });
    match rx.recv_timeout(Duration::from_secs(8)).ok().flatten() {
        Some(l) if l.starts_with("READY") => {
            let dn = l.strip_prefix("READY").unwrap_or("").trim();
            let name = if dn.is_empty() { name } else { dn.to_string() };
            let (tx, rx) = std::sync::mpsc::channel::<String>();
            std::thread::spawn(move || { for l in stdout.lines() { match l { Ok(l) => { if tx.send(l).is_err() { break; } } Err(_) => break } } });
            // start with a SMALL assumed rate (50 MH/s) so the first sweep is short on any GPU — the real rate
            // arrives with the first END and sizes the next sweep. (1 GH/s assumed on a slow iGPU = a multi-second
            // first sweep, which the hang watchdog would mistake for a hung driver.)
            Some(Daemon { child, stdin, rx, name, dev, weight: 0.05, dead: false, died_at: Instant::now(), since: Instant::now(), fails: 0 })
        }
        _ => { let _ = child.kill(); let _ = child.wait(); None }
    }
}
// the header fields a sweep needs (one struct instead of 4 loose args)
struct Work<'a> { prevhash: &'a str, ntime: &'a str, work_root: &'a str, bits: u32 }
// Returns (winning nonces, per-GPU GH/s, CPU GH/s, events to log). A GPU that exits or doesn't answer within the
// deadline is killed + marked dead here; the engine respawns it later (see Daemon).
fn grind_all(ds: &mut [Daemon], cpu_threads: usize, cpu_rate: &mut f64, w: &Work, secs: f64) -> (Vec<String>, Vec<f64>, f64, Vec<String>) {
    let space: u64 = 1u64 << 32;
    let gpu_caps: Vec<u64> = ds.iter().map(|d| if d.dead { 0 } else { ((d.weight * 1e9 * secs) as u64).max(1 << 22) }).collect();
    let cpu_cap: u64 = if cpu_threads > 0 { ((*cpu_rate * 1e9 * secs) as u64).max(2_000_000) } else { 0 };
    let total: u64 = gpu_caps.iter().sum::<u64>() + cpu_cap;
    let sweep: u64 = if total == 0 || total >= space { space } else { total };
    let mut cursor: u64 = 0;
    for (i, d) in ds.iter_mut().enumerate() {
        if d.dead || gpu_caps[i] == 0 { continue; }   // dead daemons get no job; their nonce share went to live workers via `total`
        let span = if total > 0 { (sweep as u128 * gpu_caps[i] as u128 / total as u128) as u64 } else { 0 };
        let _ = writeln!(d.stdin, "{} {} {} {} {} {}", w.prevhash, w.ntime, w.work_root, w.bits, cursor, span.max(1));
        let _ = d.stdin.flush();
        cursor += span;
    }
    let mut nonces: Vec<String> = vec![];
    let mut events: Vec<String> = vec![];
    let mut cpu_ghs = 0.0f64;
    if cpu_threads > 0 && cursor < sweep {
        let cpu_span = sweep - cursor;
        let t0 = Instant::now();
        let won = cpu_grind(w.prevhash, w.ntime, w.work_root, w.bits, cpu_threads, cursor, cpu_span);
        let dt = t0.elapsed().as_secs_f64();
        cpu_ghs = if dt > 0.0 { cpu_span as f64 / dt / 1e9 } else { 0.0 };
        if cpu_ghs > 0.0 { *cpu_rate = cpu_ghs; }
        nonces.extend(won);
    }
    let mut gpu_ghs = vec![0.0f64; ds.len()];
    // a sweep sized to `secs` that takes 6× longer (+5s slack) is a hung driver, not a slow GPU
    let deadline = Duration::from_secs_f64(secs * 6.0 + 5.0);
    for (i, d) in ds.iter_mut().enumerate() {
        if d.dead { continue; }   // no job was sent to a dead daemon
        let t0 = Instant::now();
        loop {
            let left = deadline.checked_sub(t0.elapsed()).unwrap_or(Duration::ZERO);
            match d.rx.recv_timeout(left) {
                Ok(line) => {
                    let t = line.trim();
                    if let Some(rest) = t.strip_prefix("END ") {
                        gpu_ghs[i] = rest.parse().unwrap_or(0.0);
                        if gpu_ghs[i] > 0.0 { d.weight = gpu_ghs[i]; }
                        if d.since.elapsed() > Duration::from_secs(300) { d.fails = 0; }   // stable for 5 min → forget old crashes (backoff resets)
                        break;
                    } else if !t.is_empty() { nonces.push(t.to_string()); }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    d.kill(); d.fails += 1;
                    events.push(format!("⚠ {} unresponsive for {:.0}s — killed · respawning in {}s", d.name, deadline.as_secs_f64(), d.retry_in().as_secs()));
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    d.kill(); d.fails += 1;
                    events.push(format!("⚠ {} grinder exited — respawning in {}s", d.name, d.retry_in().as_secs()));
                    break;
                }
            }
        }
    }
    (nonces, gpu_ghs, cpu_ghs, events)
}

fn send(stream: &mut TcpStream, v: &Value) {
    let mut s = serde_json::to_string(v).unwrap();
    s.push('\n');
    // the socket is non-blocking (so reads can poll); a plain write_all would DROP a mining.submit on
    // WouldBlock. Loop until the whole line is sent, retrying briefly on WouldBlock (bounded) so submits
    // never partial-write.
    let buf = s.as_bytes(); let mut off = 0; let start = Instant::now();
    while off < buf.len() {
        match stream.write(&buf[off..]) {
            Ok(0) => break,
            Ok(n) => off += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() > Duration::from_secs(5) { break; }
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

// TCP connect with a real timeout. Plain TcpStream::connect uses the OS default (minutes against a blackholed
// host), which made a stratum switch to a dead pool look like the miner hung.
fn tcp_connect(pool: &str, secs: u64) -> Option<TcpStream> {
    use std::net::ToSocketAddrs;
    let addrs: Vec<std::net::SocketAddr> = pool.to_socket_addrs().ok()?.collect();
    addrs.iter().find_map(|a| TcpStream::connect_timeout(a, Duration::from_secs(secs)).ok())
}
// sleep `secs`, but return at once if the user switched stratum/address meanwhile (backoff must not delay a switch)
fn sleep_unless_switched(tgt: &Arc<Mutex<Target>>, pool: &str, addr: &str, secs: u64) {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(secs) {
        { let t = tgt.lock().unwrap(); if t.pool != pool || t.addr != addr { return; } }
        std::thread::sleep(Duration::from_millis(200));
    }
}

struct Conn {
    stream: TcpStream, buf: Vec<u8>, en1: Option<String>, en2size: usize, diff: f64,
    job: Option<Vec<Value>>, en2ctr: u64, pending: HashMap<u64, (String, bool, u64, Instant)>, subid: u64, addr: String, is_dev: bool, idle: Instant, last_notify: Instant,
}
impl Conn {
    fn connect(pool: &str, addr: &str, is_dev: bool) -> Option<Conn> {
        let mut stream = tcp_connect(pool, 8)?;
        stream.set_nonblocking(true).ok();
        let sub_ua = if is_dev { format!("PyBLOCK-GPU/BLAKE2b-donate/{}", VERSION) } else { format!("PyBLOCK-GPU/BLAKE2b/{}", VERSION) };
        send(&mut stream, &json!({"id":1,"method":"mining.subscribe","params":[sub_ua]}));
        send(&mut stream, &json!({"id":2,"method":"mining.authorize","params":[addr,"x"]}));
        // ask the pool for a LOW starting difficulty. Without this, a datum whose starting/min share-diff is
        // too high for a GPU (e.g. diff 1024 ≈ 13 min/share at 5 GH/s) yields ~no shares, so vardiff never has
        // data to adjust DOWN → the miner "hashes but never submits". Pools that honor it start low + vardiff up.
        send(&mut stream, &json!({"id":3,"method":"mining.suggest_difficulty","params":[1]}));
        Some(Conn { stream, buf: Vec::new(), en1: None, en2size: 8, diff: 1.0, job: None,
            en2ctr: 0, pending: HashMap::new(), subid: 100, addr: addr.to_string(), is_dev, idle: Instant::now(), last_notify: Instant::now() })
    }
    fn pump(&mut self, stats: &Arc<Mutex<Stats>>) -> bool {
        let mut tmp = [0u8; 8192];
        match self.stream.read(&mut tmp) {
            Ok(0) => return false,
            Ok(n) => { self.buf.extend_from_slice(&tmp[..n]); self.idle = Instant::now(); }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return false,
        }
        let mut msgs = vec![];
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            if let Ok(v) = serde_json::from_slice::<Value>(&line[..line.len().saturating_sub(1)]) { msgs.push(v); }
        }
        for m in msgs {
            let id = m.get("id").and_then(|v| v.as_u64());
            let meth = m.get("method").and_then(|v| v.as_str());
            if id == Some(1) && m.get("result").map_or(false, |r| !r.is_null()) {
                let r = &m["result"];
                self.en1 = r.get(1).and_then(|v| v.as_str()).map(|s| s.to_string());
                self.en2size = r.get(2).and_then(|v| v.as_u64()).unwrap_or(8) as usize;
                if !self.is_dev {
                    let e1 = self.en1.clone().unwrap_or_default();
                    stats.lock().unwrap().logline(format!("subscribed · extranonce1={}", e1));
                }
            } else if meth == Some("mining.set_difficulty") {
                self.diff = m.get("params").and_then(|p| p.get(0)).and_then(|v| v.as_f64()).unwrap_or(1.0);
                if !self.is_dev {
                    let bits = floor_pot(self.diff);
                    let mut st = stats.lock().unwrap();
                    st.diff = self.diff; st.bits = bits;
                    st.logline(format!("difficulty set · bits={}", bits));
                }
            } else if meth == Some("mining.notify") {
                self.job = m.get("params").and_then(|v| v.as_array()).cloned();
                self.last_notify = Instant::now();   // fresh work arrived → feeds the job-freshness watchdog
            } else if let Some(idv) = id {
                if let Some((nonce, is_block, height, _)) = self.pending.remove(&idv) {
                    let hs = if height > 0 { format!("height {} ", height) } else { String::new() };
                    let mut st = stats.lock().unwrap();
                    if m.get("result") == Some(&Value::Bool(true)) {
                        if self.is_dev {
                            if is_block {
                                st.donated += 1; let n = st.donated;
                                st.logline(format!("💚 donation BLOCK {}(#{}) → developer · thank you! · nonce {}", hs, n, nonce));
                            }
                        } else {
                            st.accepted += 1;
                            if is_block {
                                st.blocks += 1; let n = st.blocks;
                                st.logline(format!("🎉 BLOCK FOUND {}(#{})  paid to your address · nonce {}", hs, n, nonce));
                                let body = format!("{}block #{} this session · paid to {}", hs, n, st.addr);
                                alert(&mut st, "🎉 BLOCK FOUND", &body);
                            } else {
                                let a = st.accepted;
                                st.logline(format!("✓ share accepted (#{}) · nonce {}", a, nonce));
                            }
                        }
                    } else if m.get("error").map_or(false, |e| !e.is_null()) {
                        st.rejected += 1;
                        let e = m.get("error").map(|v| v.to_string()).unwrap_or_default();
                        let who = if self.is_dev { "donation " } else { "" };
                        st.logline(format!("✗ {}rejected ({})  nonce {}", who, e, nonce));
                    }
                }
            }
        }
        // submits the pool never answered would otherwise pile up forever (weeks-long sessions on a flaky pool)
        self.pending.retain(|_, v| v.3.elapsed() < Duration::from_secs(120));
        true
    }
    fn ready(&self) -> Option<(String, Vec<Value>)> {
        match (&self.en1, &self.job) {
            (Some(a), Some(b)) if b.len() >= 8 => Some((a.clone(), b.clone())),
            _ => None,
        }
    }
}

fn build_work(conn: &mut Conn, en1v: &str, jobv: &[Value]) -> (String, String, String, String, String, String) {
    let prevhash = jobv[1].as_str().unwrap_or("").to_string();
    let job_id = jobv[0].as_str().unwrap_or("").to_string();
    let version = jobv[5].as_str().unwrap_or("").to_string();
    let ntime = jobv[7].as_str().unwrap_or("").to_string();
    let coinb1 = hex::decode(jobv[2].as_str().unwrap_or("")).unwrap_or_default();
    let en2_full = conn.en2ctr.to_le_bytes();
    conn.en2ctr += 1;
    let mut en2 = en2_full[..conn.en2size.min(8)].to_vec();
    en2.resize(conn.en2size.max(1), 0);
    let mut extranonce = hex::decode(en1v).unwrap_or_default();
    extranonce.extend_from_slice(&en2);
    extranonce.resize(12, 0);
    let mut leaf = vec![0u8];
    leaf.extend_from_slice(&coinb1);
    leaf.extend_from_slice(&extranonce);
    let work_root = b2b(&leaf);
    (prevhash, ntime, version, job_id, hex::encode(&en2), hex::encode(work_root))
}

// engine reads the shared Target; when pool/addr changes (live stratum switch) it reconnects.
fn engine(stats: Arc<Mutex<Stats>>, tgt: Arc<Mutex<Target>>, ngpu: u32, cpu_threads: usize, paused: Arc<AtomicBool>) {
    let names = gpu_names();
    let mut daemons: Vec<Daemon> = Vec::new();
    for dev in 0..ngpu {
        let nm = names.get(dev as usize).cloned().unwrap_or_else(|| format!("GPU {}", dev));
        if let Some(d) = spawn_daemon(dev, nm) { daemons.push(d); }
    }
    // if the GPU grinder couldn't start (not built / no device), fall back to CPU so we still mine
    let mut cpu_threads = cpu_threads;
    if daemons.is_empty() && cpu_threads == 0 {
        cpu_threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    }
    let mut cpu_rate = 0.05f64;
    { let mut st = stats.lock().unwrap();
      st.gpu_names = daemons.iter().map(|d| d.name.clone()).collect();
      if cpu_threads > 0 { st.gpu_names.push(format!("CPU ({} threads)", cpu_threads)); }
      st.gpu_ghs = vec![0.0; st.gpu_names.len()];
      let nworkers = st.gpu_names.len();
      let names_str = st.gpu_names.join(", ");
      st.logline(format!("{} worker(s) ready: {}", nworkers, names_str)); }

    let mut donate_credit = 0.0f64;
    let mut dev_retry = Instant::now();
    let mut conn_fails = 0u32;   // consecutive connect failures / instant drops → exponential backoff 3s…60s
    // adaptive sweep length: track the observed block cadence (from the user's prevhash changes) so sweeps
    // stay short on fast chains (switch to new work sooner → far fewer stale shares + less wasted hashrate)
    // but keep the efficient default on normal-speed chains.
    let mut last_prevhash = String::new();
    let mut last_block_at = Instant::now();
    let mut block_interval_ema = 30.0f64;   // conservative start → sweep capped at MAX until real cadence is seen

    loop {
        // read current live target (pool/addr/donate change when the user switches stratum)
        let (pool, addr, worker, donate, network) = { let t = tgt.lock().unwrap(); (t.pool.clone(), t.addr.clone(), t.worker.clone(), t.donate, t.network.clone()) };
        if addr.is_empty() {
            { let mut st = stats.lock().unwrap(); st.connected = false;
              st.logline("no address set — go to SETUP [5] to set/generate one".into()); }
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        stats.lock().unwrap().last_tick = Some(Instant::now());
        let mut user = match Conn::connect(&pool, &login(&addr, &worker), false) {
            Some(c) => c,
            None => {
                conn_fails += 1;
                let wait = (3u64 << (conn_fails - 1).min(4)).min(60);   // 3 · 6 · 12 · 24 · 48 · 60s
                { let mut st = stats.lock().unwrap(); st.connected = false; st.logline(format!("connection failed to {} — retry in {}s (attempt {})", pool, wait, conn_fails));
                  if conn_fails == 3 { alert(&mut st, "pool unreachable", &format!("{} — 3 failed attempts, retrying with backoff · not mining", pool)); } }
                sleep_unless_switched(&tgt, &pool, &addr, wait);
                continue;
            }
        };
        let mut dev: Option<Conn> = if donate > 0.0 { Conn::connect(DONATE_POOL, DEV_DONATION_ADDR, true) } else { None };
        { let mut st = stats.lock().unwrap(); st.connected = true; st.started.get_or_insert(Instant::now());
          st.logline(format!("connected to {} as {}", pool, login(&addr, &worker)));
          if conn_fails >= 3 { alert(&mut st, "pool reachable again", &format!("connected to {} after {} attempts · mining", pool, conn_fails)); }
          if donate > 0.0 { st.logline(format!("hashrate donation {:.1}% → PyBLØCK", donate)); } }
        let session_start = Instant::now();
        let mut switched = false;

        loop {
            stats.lock().unwrap().last_tick = Some(Instant::now());   // heartbeat: every path through this loop ticks
            // live switch: if the shared target's pool/addr changed, drop + reconnect
            { let t = tgt.lock().unwrap();
              if t.pool != pool || t.addr != addr || t.worker != worker || t.network != network || t.donate != donate {
                stats.lock().unwrap().logline(format!("switching stratum → {}", t.pool)); switched = true; break;
              } }
            // dead grinders: respawn once their backoff has elapsed (keeps index/name so the WORKERS rows stay put)
            let mut respawn_tried = false;
            for d in daemons.iter_mut() {
                if d.dead && d.died_at.elapsed() >= d.retry_in() {
                    respawn_tried = true;
                    match spawn_daemon(d.dev, d.name.clone()) {
                        Some(mut nd) => { nd.fails = d.fails; *d = nd; let mut st = stats.lock().unwrap(); alert(&mut st, "GPU back online", &format!("{} is hashing again", d.name)); }
                        None => { d.died_at = Instant::now(); d.fails += 1;
                                  stats.lock().unwrap().logline(format!("✗ {} still not starting — next try in {}s", d.name, d.retry_in().as_secs())); }
                    }
                }
            }
            if respawn_tried {
                let mut st = stats.lock().unwrap();
                st.gpu_dead = daemons.iter().map(|d| d.dead).chain(std::iter::once(false).take(usize::from(cpu_threads > 0))).collect();
            }
            if !user.pump(&stats) { { let mut st = stats.lock().unwrap(); st.connected = false; st.logline("disconnected — reconnecting…".into()); } break; }
            if let Some(d) = dev.as_mut() { if !d.pump(&stats) { dev = None; dev_retry = Instant::now(); } }
            if dev.is_none() && donate > 0.0 && dev_retry.elapsed() > Duration::from_secs(20) {
                dev = Conn::connect(DONATE_POOL, DEV_DONATION_ADDR, true); dev_retry = Instant::now();
            }
            if user.idle.elapsed() > Duration::from_secs(90) { { let mut st = stats.lock().unwrap(); st.connected = false; st.logline("no data for 90s — reconnecting…".into()); } break; }
            // Job-freshness watchdog: force a clean resubscribe if no NEW work (mining.notify) has arrived for a
            // while, scaled to the observed block cadence. `idle` only tracks ANY bytes (set_difficulty pings keep
            // it alive), so a connection that stays open but stops delivering jobs — exactly what happens at a
            // chain transition (BLAKE2b activation / pool re-point / node reorg) — would otherwise leave the miner
            // grinding dead work until a manual restart. This makes it pick up the new template ON ITS OWN.
            let stall = Duration::from_secs_f64((block_interval_ema * 6.0).clamp(90.0, 600.0));
            if user.last_notify.elapsed() > stall { { let mut st = stats.lock().unwrap(); st.connected = false; st.logline(format!("no new job in {}s — resubscribing…", stall.as_secs())); } break; }

            // PAUSE: user hit `p`. Stop feeding the grinders (GPU/CPU go idle → no work sent, no shares submitted)
            // but keep the pool connection alive by pumping it, so resume is instant with no reconnect.
            if paused.load(Ordering::Relaxed) {
                { let mut st = stats.lock().unwrap();
                  if !st.paused { st.paused = true; st.logline("⏸ mining paused".into()); }
                  st.hr_total = 0.0; for g in st.gpu_ghs.iter_mut() { *g = 0.0; } }
                user.pump(&stats);
                if let Some(d) = dev.as_mut() { d.pump(&stats); }
                std::thread::sleep(Duration::from_millis(120));
                continue;
            } else {
                let mut st = stats.lock().unwrap();
                if st.paused { st.paused = false; st.logline("▶ mining resumed".into()); }
            }

            // ALGO GATE: pyblockMiner only hashes BLAKE2b. While the chain is still SHA-256d (pre-activation),
            // grinding is pure wasted electricity + guaranteed rejects — so idle the grinders and wait. The pool
            // reports blake2b_active=false until the flag-day height; we keep the connection pumped so mining
            // resumes ON ITS OWN the moment BLAKE2b activates. (None/unknown → grind — safe default.)
            if stats.lock().unwrap().blake2b_active == Some(false) {
                { let mut st = stats.lock().unwrap(); st.hr_total = 0.0; for g in st.gpu_ghs.iter_mut() { *g = 0.0; } }
                user.pump(&stats);
                if let Some(d) = dev.as_mut() { d.pump(&stats); }
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }

            donate_credit += donate / 100.0;
            let dev_ready = dev.as_ref().and_then(|d| d.ready()).is_some();
            let do_donate = donate_credit >= 1.0 && dev_ready;
            let (en1v, jobv, is_dev) = if do_donate {
                let d = dev.as_ref().unwrap(); let (a, b) = d.ready().unwrap(); (a, b, true)
            } else {
                match user.ready() { Some((a, b)) => (a, b, false), None => { std::thread::sleep(Duration::from_millis(60)); continue; } }
            };
            if is_dev { donate_credit -= 1.0; }

            let diff = if is_dev { dev.as_ref().unwrap().diff } else { user.diff };
            let bits = floor_pot(diff);
            let (prevhash, ntime, version, job_id, en2hex, work_root) = {
                let c = if is_dev { dev.as_mut().unwrap() } else { &mut user };
                build_work(c, &en1v, &jobv)
            };
            // update the observed block interval from the user's prevhash changes, then size the sweep to a
            // small fraction of it (clamped): fast chain → short sweep → quick switch; slow chain → 0.35s cap.
            if !is_dev && prevhash != last_prevhash {
                if !last_prevhash.is_empty() {
                    let dt = last_block_at.elapsed().as_secs_f64();
                    if dt > 0.02 && dt < 600.0 { block_interval_ema = block_interval_ema * 0.7 + dt * 0.3; }
                }
                last_prevhash = prevhash.clone();
                last_block_at = Instant::now();
            }
            let sweep_secs = (block_interval_ema * 0.15).clamp(0.06, 0.35);
            let work = Work { prevhash: &prevhash, ntime: &ntime, work_root: &work_root, bits };
            let (nonces, gpu_ghs, cpu_ghs, events) = grind_all(&mut daemons, cpu_threads, &mut cpu_rate, &work, sweep_secs);
            {
                let mut st = stats.lock().unwrap();
                for e in events { alert(&mut st, "GPU down", &e); }
                st.gpu_dead = daemons.iter().map(|d| d.dead).chain(std::iter::once(false).take(usize::from(cpu_threads > 0))).collect();
                let mut all = gpu_ghs.clone();
                if cpu_threads > 0 { all.push(cpu_ghs); }
                st.gpu_ghs = all;
                st.hr_total = st.gpu_ghs.iter().sum();
                let hv = (st.hr_total * 100.0) as u64;
                st.hr_hist.push_back(hv);
                while st.hr_hist.len() > 512 { st.hr_hist.pop_front(); }   // keep enough samples so the Sparkline fills wide terminals (it shows the last <width> points)
            }
            user.pump(&stats);
            if let Some(d) = dev.as_mut() { d.pump(&stats); }
            // Only submit winners if this sweep's job is still the tip. If a block landed DURING the grind,
            // its winners are for the old prevhash → worthless (the pool rejects them as stale-prevblk), so we
            // skip them, exactly like a normal miner does. Safe now because the adaptive sweep keeps each grind
            // well under the block interval → stale sweeps are a minority (this is NOT the old v0.2.1 bug that
            // dropped everything with fixed 0.35s sweeps ≈ the block time on a fast chain).
            let nbits = jobv.get(6).and_then(|v| v.as_str()).and_then(|s| u32::from_str_radix(s, 16).ok()).unwrap_or(0);
            if !is_dev && nbits != 0 { stats.lock().unwrap().net_nbits = nbits; }   // network target → DATA-tab solo-lottery ETA
            let net_target = nbits_to_target(nbits);
            // found-block height. The BLAKE2b stratum notify carries NO coinbase (coinb2 empty, merkle []),
            // so coinb1 has no BIP34 height — the pool's reported mining height is the only source. blake_stats
            // returns the *template* height (tip+1) = exactly the block being mined. Captured per-sweep and
            // carried in `pending` so BLOCK FOUND logs it even if the tip moves before the pool replies.
            let job_height = stats.lock().unwrap().net_height;
            // did the tip move during this grind? compare the ground prevhash to the latest job's prevhash
            let still_current = {
                let latest = if is_dev { dev.as_ref().and_then(|d| d.job.as_ref()) } else { user.job.as_ref() };
                match latest.and_then(|j| j.get(1)).and_then(|v| v.as_str()) {
                    Some(cur) => cur == prevhash,
                    None => true,   // unknown → submit rather than silently drop
                }
            };
            let mut sweep_best = 0.0f64;
            let conn = if is_dev { dev.as_mut().unwrap() } else { &mut user };
            for nh in nonces {
                // The grinders filter at floor_pot(diff) — a power of two. On a pool diff that ISN'T a power of two
                // (vardiff 3000 → kernel bits 11 = 2048) some winners are below the real share target and the pool
                // would reject them → check the exact difficulty here and only send what can be accepted.
                let (is_block, meets_diff) = match nonce_hash(&prevhash, &ntime, &work_root, &nh) {
                    Some(h) => { let d = hash_diff(&h); if d > sweep_best { sweep_best = d; }
                                 (nbits != 0 && hash_le_target(&h, &net_target), d >= diff * 0.999) }
                    None => (false, true),
                };
                if !still_current || !meets_diff { continue; }   // stale sweep (a block landed mid-grind) or sub-target → don't submit
                conn.subid += 1; let sid = conn.subid;
                conn.pending.insert(sid, (nh.clone(), is_block, job_height, Instant::now()));
                send(&mut conn.stream, &json!({"id":sid,"method":"mining.submit","params":[conn.addr, job_id, en2hex, ntime, nh, version]}));
            }
            if sweep_best > 0.0 && !is_dev { let mut st = stats.lock().unwrap(); if sweep_best > st.best_diff { st.best_diff = sweep_best; } }
        }
        // a session that died within 10s of connecting (pool accepts TCP then drops us) counts as a failure → back off,
        // instead of hammering the pool in a tight reconnect loop. A user-driven switch never waits.
        if !switched && session_start.elapsed() < Duration::from_secs(10) {
            conn_fails += 1;
            let wait = (3u64 << (conn_fails - 1).min(4)).min(60);
            stats.lock().unwrap().logline(format!("session dropped early — reconnecting in {}s", wait));
            sleep_unless_switched(&tgt, &pool, &addr, wait);
        } else { conn_fails = 0; }
    }
}

struct NetStats { miners: u64, ghs: f64, height: u64, blake2b_active: Option<bool>, activation_height: u64, blocks_until: u64, latest: String }
// true if `latest` is a strictly newer semver than `cur` (both "x.y.z", optional leading 'v')
fn is_newer(latest: &str, cur: &str) -> bool {
    let t = |s: &str| { let mut i = s.trim().trim_start_matches('v').split('.').map(|x| x.parse::<u32>().unwrap_or(0));
        (i.next().unwrap_or(0), i.next().unwrap_or(0), i.next().unwrap_or(0)) };
    t(latest) > t(cur)
}
fn poll_network_stats(url: &str) -> Option<NetStats> {
    let body = ureq::get(url).timeout(Duration::from_secs(8)).call().ok()?.into_string().ok()?;
    let v: Value = serde_json::from_str(&body).ok()?;
    if !v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false) { return None; }
    Some(NetStats {
        miners: v.get("miners").and_then(|x| x.as_u64()).unwrap_or(0),
        ghs: v.get("network_hashrate_ghs").and_then(|x| x.as_f64()).unwrap_or(0.0),
        height: v.get("block_height").and_then(|x| x.as_u64()).unwrap_or(0),
        blake2b_active: v.get("blake2b_active").and_then(|x| x.as_bool()),   // null/absent → None (unknown → grind)
        activation_height: v.get("activation_height").and_then(|x| x.as_u64()).unwrap_or(0),
        blocks_until: v.get("blocks_until_activation").and_then(|x| x.as_u64()).unwrap_or(0),
        latest: v.get("miner_latest").and_then(|x| x.as_str()).unwrap_or("").to_string(),
    })
}
// The balance ALWAYS comes from the PyBLØCK blake2b node for the SELECTED network — pyblockMiner mines
// BLAKE2b, so the balance lives on the blake2b chain, NOT on public SHA-256 explorers (mempool.space doesn't
// follow the blake2b chain → it would report 0/garbage). The network is auto-detected from the active stratum;
// each network has its own pool endpoint that reads that network's blake2b node. When the stable mainnet
// BLAKE2b version ships, "mainnet" transparently reads the mainnet blake2b node — no code change, no hardcode.
fn net_balance_url(net: &str) -> &'static str {
    match net {
        "testnet4" => "https://pool.pyblock.xyz:8443/api/blake_balance_t4.php",
        _          => "https://pool.pyblock.xyz:8443/api/blake_balance.php",   // mainnet + regtest → PyBLØCK blake2b node
    }
}
fn poll_balance(net: &str, addr: &str) -> Option<f64> {
    if addr.is_empty() { return None; }
    let url = format!("{}?addr={}", net_balance_url(net), addr);
    let body = ureq::get(&url).timeout(Duration::from_secs(12)).call().ok()?.into_string().ok()?;
    let v: Value = serde_json::from_str(&body).ok()?;
    if !v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false) { return None; }
    v.get("balance_btc").and_then(|x| x.as_f64())
}

// ── mode data: CHIRP coinbase members · CAROUSEL rotation (the pool's own pages poll these every 15s) ──
const CHIRP_API: &str = "https://pool.pyblock.xyz:8443/chirp_api.php";
const CAROUSEL_API: &str = "https://b.pyblock.xyz:8443/carousel.php?carrousel=1";
fn get_json(url: &str, secs: u64) -> Option<Value> {
    let body = ureq::get(url).timeout(Duration::from_secs(secs)).call().ok()?.into_string().ok()?;
    serde_json::from_str(&body).ok()
}
fn now_unix() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }
fn poll_chirp() -> Option<ChirpInfo> {
    let list = get_json(&format!("{}?mode=miners&chain=blake2b", CHIRP_API), 10)?;
    let mut members: Vec<ChirpMember> = list.as_array()?.iter().map(|m| ChirpMember {
        // addresses are base58/bech32 → anything else is noise (or an attempt to draw on the terminal)
        addr: m.get("address").and_then(|x| x.as_str()).unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric()).take(90).collect(),
        days: m.get("days").and_then(|x| x.as_f64()).unwrap_or(0.0),
        power: m.get("power").and_then(|x| x.as_f64()).unwrap_or(0.0),
        weight: m.get("weight").and_then(|x| x.as_f64()).unwrap_or(0.0),
        eligible: m.get("eligible").and_then(|x| x.as_bool()).unwrap_or(false),
        last_seen: m.get("last_seen").and_then(|x| x.as_u64()).unwrap_or(0),
    }).collect();
    let f = |a: f64, b: f64| b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal);
    members.sort_by(|a, b| b.eligible.cmp(&a.eligible).then(f(a.weight, b.weight)).then(f(a.days, b.days)));
    let mut info = ChirpInfo { members, fetched: now_unix(), min_days: 7.0, ..Default::default() };
    if let Some(p) = get_json(&format!("{}?mode=pool&chain=blake2b", CHIRP_API), 8) {
        info.candidates = p.get("candidates").and_then(|x| x.as_u64()).unwrap_or(0);
        info.workers = p.get("workers").and_then(|x| x.as_u64()).unwrap_or(0);
        info.blocks = p.get("blocks").and_then(|x| x.as_u64()).unwrap_or(0);
        info.hashrate_ths = p.get("hashrate").and_then(|x| x.as_f64()).unwrap_or(0.0);   // the site formats this as TH/s
        info.min_days = p.get("min_days").and_then(|x| x.as_f64()).unwrap_or(7.0);
        info.min_power = p.get("min_power").and_then(|x| x.as_f64()).unwrap_or(0.0);
    } else {
        info.candidates = info.members.iter().filter(|m| m.eligible).count() as u64;
    }
    if let Some(c) = get_json(&format!("{}?mode=coinbase&chain=blake2b", CHIRP_API), 8) {
        info.reward_sats = c.get("reward_sats").and_then(|x| x.as_u64()).unwrap_or(0);
        info.height = c.get("height").and_then(|x| x.as_u64()).unwrap_or(0);
        info.fee_bps = c.get("fee_bps").and_then(|x| x.as_u64()).unwrap_or(90);
    }
    Some(info)
}
// Supplier names are typed by THIRD PARTIES on the pool → strip control chars / escape sequences and cap the length
// before they reach the terminal. Printable Unicode (emoji, accents) stays.
fn clean_label(s: &str) -> String { s.chars().filter(|c| !c.is_control()).take(40).collect::<String>().trim().to_string() }
fn poll_carousel() -> Option<CarouselInfo> {
    let v = get_json(CAROUSEL_API, 10)?;
    let strs = |k: &str| -> Vec<String> { v.get(k).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|e| {
        // suppliers: ["name", …] · recent: [["name"], …] or [["name", ts], …]
        e.as_str().map(clean_label).or_else(|| e.as_array().and_then(|i| i.first()).and_then(|s| s.as_str()).map(clean_label))
    }).collect()).unwrap_or_default() };
    Some(CarouselInfo {
        suppliers: strs("suppliers"), recent: strs("recent"),
        current: clean_label(v.get("current").and_then(|x| x.as_str()).unwrap_or("")),
        miners: v.get("miners").and_then(|x| x.as_u64()).unwrap_or(0),
        hashrate_ths: v.get("hashrate").and_then(|x| x.as_f64()).unwrap_or(0.0) / 1e12,   // endpoint returns H/s
        live: v.get("live").and_then(|x| x.as_bool()).unwrap_or(false),
        fetched: now_unix(),
    })
}

// ── local stats API: GET / → JSON · GET /metrics → Prometheus text. 127.0.0.1 only, one tiny HTTP/1.0 server. ──
fn stats_json(st: &Stats) -> Value {
    let workers: Vec<Value> = st.gpu_names.iter().enumerate().map(|(i, n)| json!({
        "name": n, "ghs": st.gpu_ghs.get(i).copied().unwrap_or(0.0), "offline": st.gpu_dead.get(i).copied().unwrap_or(false) })).collect();
    let chirp = st.chirp.as_ref().map(|c| json!({
        "listed": c.me(&st.addr).is_some(), "eligible": c.me(&st.addr).map(|m| m.eligible).unwrap_or(false),
        "slice_pct": c.my_pct(&st.addr), "tenure_days": c.me(&st.addr).map(|m| m.days),
        "candidates": c.candidates, "workers": c.workers, "hashrate_ths": c.hashrate_ths, "blocks": c.blocks,
        "reward_btc": c.reward_sats as f64 / 1e8, "expected_btc_day": c.my_pct(&st.addr).map(|p| chirp_btc_per_day(st, c, p)) }));
    let carousel = st.carousel.as_ref().map(|k| json!({ "current": k.current, "suppliers": k.suppliers, "miners": k.miners, "hashrate_ths": k.hashrate_ths }));
    json!({
        "version": VERSION, "ts": now_unix(), "uptime_s": st.started.map(|s| s.elapsed().as_secs()).unwrap_or(0),
        "pool": st.endpoint, "mode": st.mode.label(), "network": st.network, "address": st.addr, "worker": st.worker,
        "connected": st.connected, "paused": st.paused, "blake2b_active": st.blake2b_active,
        "hashrate_ghs": st.hr_total, "workers": workers,
        "blocks": st.blocks, "shares_accepted": st.accepted, "shares_rejected": st.rejected, "donation_blocks": st.donated,
        "difficulty": st.diff, "best_share_diff": st.best_diff,
        "balance_btc": if st.balance_ok { Some(st.balance_btc) } else { None },
        "net": { "ok": st.net_ok, "miners": st.net_miners, "hashrate_ghs": st.net_ghs, "height": st.net_height },
        "chirp": chirp, "carousel": carousel, "alerts_sent": st.alerts_sent,
    })
}
fn metrics_text(st: &Stats) -> String {
    let mut o = String::new();
    let g = |o: &mut String, name: &str, help: &str, v: f64| { o.push_str(&format!("# HELP {n} {h}\n# TYPE {n} gauge\n{n} {v}\n", n = name, h = help, v = v)); };
    g(&mut o, "pyblock_hashrate_ghs", "total hashrate in GH/s", st.hr_total);
    g(&mut o, "pyblock_connected", "1 if the stratum session is up", if st.connected { 1.0 } else { 0.0 });
    g(&mut o, "pyblock_paused", "1 if mining is paused", if st.paused { 1.0 } else { 0.0 });
    g(&mut o, "pyblock_blocks_found", "blocks found this session", st.blocks as f64);
    g(&mut o, "pyblock_shares_accepted", "shares accepted this session", st.accepted as f64);
    g(&mut o, "pyblock_shares_rejected", "shares rejected this session", st.rejected as f64);
    g(&mut o, "pyblock_best_share_diff", "best share difficulty this session", st.best_diff);
    g(&mut o, "pyblock_difficulty", "current share difficulty", st.diff);
    g(&mut o, "pyblock_net_hashrate_ghs", "pyblockMiner network hashrate in GH/s", st.net_ghs);
    g(&mut o, "pyblock_net_miners", "pyblockMiner miners online", st.net_miners as f64);
    g(&mut o, "pyblock_uptime_seconds", "seconds since first connect", st.started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0));
    if st.balance_ok { g(&mut o, "pyblock_balance_btc", "address balance on the BLAKE2b chain", st.balance_btc); }
    if let Some(c) = st.chirp.as_ref() {
        g(&mut o, "pyblock_chirp_slice_pct", "your share of every CHIRP block (0 if not eligible)", c.my_pct(&st.addr).unwrap_or(0.0));
        g(&mut o, "pyblock_chirp_candidates", "eligible miners in the CHIRP coinbase", c.candidates as f64);
        g(&mut o, "pyblock_chirp_hashrate_ths", "CHIRP syndicate hashrate in TH/s", c.hashrate_ths);
    }
    o.push_str("# HELP pyblock_worker_hashrate_ghs per-worker hashrate in GH/s\n# TYPE pyblock_worker_hashrate_ghs gauge\n");
    for (i, n) in st.gpu_names.iter().enumerate() {
        o.push_str(&format!("pyblock_worker_hashrate_ghs{{worker=\"{}\",index=\"{}\"}} {}\n", n.replace('"', "'"), i, st.gpu_ghs.get(i).copied().unwrap_or(0.0)));
    }
    o
}
fn api_server(port: u16, stats: Arc<Mutex<Stats>>) {
    let listener = match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => { stats.lock().unwrap().logline(format!("api: cannot bind 127.0.0.1:{} — {}", port, e)); return; }
    };
    stats.lock().unwrap().logline(format!("api: http://127.0.0.1:{}/  (JSON)  ·  /metrics  (Prometheus)", port));
    for s in listener.incoming() {
        let Ok(mut s) = s else { continue };
        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
        let mut buf = [0u8; 2048];
        let n = s.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let path = req.split_whitespace().nth(1).unwrap_or("/");
        let (ctype, body) = { let st = stats.lock().unwrap();
            if path.starts_with("/metrics") { ("text/plain; version=0.0.4", metrics_text(&st)) } else { ("application/json", stats_json(&st).to_string()) } };
        let _ = write!(s, "HTTP/1.0 200 OK\r\nContent-Type: {}\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", ctype, body.len(), body);
    }
}

// ═══════════════════════ TABS / UI ═══════════════════════
#[derive(Clone, Copy, PartialEq)]
enum Tab { Mine, Data, Stratums, Learn, Network, Setup, Help }
const TABS: [(Tab, &str); 7] = [
    (Tab::Mine, "MINE"), (Tab::Data, "DATA"), (Tab::Stratums, "STRATUMS"), (Tab::Learn, "LEARN"),
    (Tab::Network, "NETWORK"), (Tab::Setup, "SETUP"), (Tab::Help, "HELP"),
];

enum Input { AddStratum, EditAddr, EditWorker, EditTelegram }
struct App {
    tab: Tab,
    cfg: Config,
    strat_cur: usize,     // cursor in the stratums list
    learn_page: usize,
    input: Option<Input>,
    buf: String,
    msg: String,          // transient status line
    paused: Arc<AtomicBool>,   // shared with the engine: `p` toggles pause/resume of mining
    list_scroll: usize,   // ↑↓ offset into the CHIRP coinbase list (MINE + NETWORK tabs)
}
impl App {
    fn network(&self) -> String { self.cfg.stratums.get(self.cfg.selected).map(|s| s.network.clone()).unwrap_or_else(|| "mainnet".into()) }
    fn addr(&self) -> String { self.cfg.addrs.get(&self.network()).cloned().unwrap_or_default() }
}

// ── visual language: one quiet rounded frame everywhere, muted labels, bold values, ONE accent per pool mode ──
fn card(title: &str, accent: Color) -> Block<'static> {
    Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(DIM))
        .title(Span::styled(format!(" {} ", title), Style::new().fg(accent)))
}
// stat tile: label · big value · footnote (3 rows + frame = 5). `accent` colours the label.
fn tile(title: &str, value: Line<'static>, sub: &str, accent: Color) -> Paragraph<'static> {
    let lab = if accent == BRD || accent == MUT { MUT } else { accent };
    Paragraph::new(Text::from(vec![
        Line::from(Span::styled(title.to_string(), Style::new().fg(lab))),
        value,
        Line::from(Span::styled(sub.to_string(), Style::new().fg(MUT))),
    ])).alignment(Alignment::Center)
        .block(Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(DIM)))
}
fn bold(s: String, col: Color) -> Span<'static> { Span::styled(s, Style::new().fg(col).add_modifier(Modifier::BOLD)) }
fn dim(s: &str) -> Span<'static> { Span::styled(s.to_string(), Style::new().fg(MUT)) }
// bc1qg8dfm…llyr — like the pool site. `full` shows the whole address (wide terminals).
fn mask_addr(a: &str, full: bool) -> String {
    let c: Vec<char> = a.chars().collect();
    if full || c.len() <= 14 { a.to_string() }
    else { format!("{}…{}", c[..6].iter().collect::<String>(), c[c.len() - 4..].iter().collect::<String>()) }
}
// TH/s magnitude → human string (mirrors the pool site's formatHashrate)
fn fmt_ths(th: f64) -> String {
    if th <= 0.0 { "—".into() }
    else if th >= 1e6 { format!("{:.2} EH/s", th / 1e6) } else if th >= 1e3 { format!("{:.2} PH/s", th / 1e3) }
    else if th >= 1.0 { format!("{:.2} TH/s", th) } else if th >= 1e-3 { format!("{:.2} GH/s", th * 1e3) }
    else if th >= 1e-6 { format!("{:.2} MH/s", th * 1e6) } else { format!("{:.0} KH/s", th * 1e9) }
}
fn fmt_num(v: f64) -> String {
    if v >= 1e9 { format!("{:.1}G", v / 1e9) } else if v >= 1e6 { format!("{:.1}M", v / 1e6) }
    else if v >= 1e3 { format!("{:.1}K", v / 1e3) } else { format!("{:.0}", v) }
}
// ▰▰▰▱▱ — a `w`-cell progress bar
fn bar(frac: f64, w: usize) -> String {
    let n = (frac.clamp(0.0, 1.0) * w as f64).round() as usize;
    format!("{}{}", "▰".repeat(n), "▱".repeat(w - n))
}
fn fmt_ago(secs: u64) -> String {
    if secs < 60 { format!("{}s", secs) } else if secs < 3600 { format!("{}m", secs / 60) }
    else if secs < 86_400 { format!("{}h", secs / 3600) } else { format!("{}d", secs / 86_400) }
}

fn tab_bar(app: &App) -> Line<'static> {
    let mut spans = vec![Span::styled("PyBLØCK ", Style::new().fg(GRN).add_modifier(Modifier::BOLD))];
    for (i, (t, name)) in TABS.iter().enumerate() {
        let sel = *t == app.tab;
        let st = if sel { Style::new().fg(Color::Black).bg(GRN).add_modifier(Modifier::BOLD) } else { Style::new().fg(MUT) };
        spans.push(Span::styled(format!(" {}·{} ", i + 1, name), st));
    }
    Line::from(spans)
}

fn ui(f: &mut Frame, app: &App, st: &Stats) {
    let outer = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).split(f.area());
    f.render_widget(Paragraph::new(tab_bar(app)), outer[0]);
    let body = outer[1];
    match app.tab {
        Tab::Mine => render_mine(f, body, st, app),
        Tab::Data => render_data(f, body, st),
        Tab::Stratums => render_stratums(f, body, app),
        Tab::Learn => render_learn(f, body, app),
        Tab::Network => render_network(f, body, st, app),
        Tab::Setup => render_setup(f, body, app),
        Tab::Help => render_help(f, body),
    }
    // footer
    let foot = if let Some(k) = &app.input {
        let label = match k { Input::AddStratum => "new stratum (name,host:port,network)", Input::EditAddr => "address",
                              Input::EditWorker => "worker name (letters · digits · - _ · empty = none)", Input::EditTelegram => "telegram  bot_token,chat_id  (empty = off)" };
        Line::from(vec![Span::styled(format!(" {} > ", label), Style::new().fg(Color::Black).bg(YLW)),
                        Span::styled(format!("{}_", app.buf), Style::new().fg(YLW)),
                        Span::styled("   Enter=ok  Esc=cancel", Style::new().fg(MUT))])
    } else if !app.msg.is_empty() {
        Line::from(Span::styled(app.msg.clone(), Style::new().fg(AMB)))
    } else {
        let mut sp = vec![Span::styled(" 1-7 ", Style::new().fg(Color::Black).bg(GRN)),
                        Span::styled(" tabs · ", Style::new().fg(MUT)),
                        Span::styled("Tab", Style::new().fg(GRN)), Span::styled(" next · ", Style::new().fg(MUT))];
        if st.mode == PoolMode::Chirp && matches!(app.tab, Tab::Mine | Tab::Network) {
            sp.push(Span::styled("↑↓", Style::new().fg(GRN))); sp.push(Span::styled(" coinbase list · ", Style::new().fg(MUT)));
        }
        sp.extend([Span::styled("p", Style::new().fg(GRN)),
                        Span::styled(if st.paused { " resume · " } else { " pause · " }, Style::new().fg(MUT)),
                        Span::styled("q", Style::new().fg(GRN)), Span::styled(" quit", Style::new().fg(MUT))]);
        Line::from(sp)
    };
    f.render_widget(Paragraph::new(foot).wrap(Wrap { trim: true }), outer[2]);
    // version — bottom-right corner
    f.render_widget(Paragraph::new(Line::from(Span::styled(format!("pyblockMiner v{} ", VERSION), Style::new().fg(MUT))))
        .alignment(ratatui::layout::Alignment::Right), outer[2]);
}

fn render_mine(f: &mut Frame, area: Rect, st: &Stats, app: &App) {
    let gpu_h = gpu_rows(st);
    // The mode panel (CHIRP coinbase list · CAROUSEL rotation · LOTTO odds) takes what its content needs,
    // within what the screen can spare after header + tiles + workers + sparkline + a small log.
    let head_h = header_rows(area.width);
    let fixed = head_h + 5 + 5 + gpu_h + 4 + 6;
    let want = mode_panel_rows(st, area.width) as u16 + 2;
    let panel_h = want.min(area.height.saturating_sub(fixed)).max(5);
    let c = Layout::vertical([Constraint::Length(head_h), Constraint::Length(5), Constraint::Length(5),
        Constraint::Length(panel_h), Constraint::Length(gpu_h), Constraint::Length(4), Constraint::Min(3)]).split(area);
    render_header(f, c[0], st);
    render_your_tiles(f, c[1], st);
    render_net_tiles(f, c[2], st);
    render_mode_panel(f, c[3], st, app);
    // workers
    let mut glines: Vec<Line> = vec![];
    for (i, g) in st.gpu_ghs.iter().enumerate() {
        let name = st.gpu_names.get(i).cloned().unwrap_or_else(|| format!("GPU {}", i));
        let down = st.gpu_dead.get(i).copied().unwrap_or(false);
        let mut sp = vec![dim(&format!("  {:>2}  ", i)), Span::styled(format!("{:<28}", name), Style::new().fg(if down { MUT } else { WHT }))];
        if down { sp.push(Span::styled("○ offline · auto-respawning", Style::new().fg(Color::Red))); }
        else { sp.push(bold(format!("{:>7.2}", g), GRN)); sp.push(dim(" GH/s")); }
        glines.push(Line::from(sp));
    }
    if glines.is_empty() { glines.push(Line::from(dim("  warming up…"))); }
    f.render_widget(Paragraph::new(Text::from(glines)).block(card("WORKERS", MUT)), c[4]);
    // ratatui's Sparkline renders the FIRST N=min(width,len) points, so feed it exactly the last inner_w
    // samples → the chart fills the full terminal width and updates live (adapts to any window size).
    let inner_w = (c[5].width.saturating_sub(2) as usize).max(1);   // block borders eat 2 columns
    let data: Vec<u64> = st.hr_hist.iter().rev().take(inner_w).rev().cloned().collect();
    f.render_widget(Sparkline::default().block(card("hashrate", MUT)).data(&data).style(Style::new().fg(GRN)), c[5]);
    let items: Vec<ListItem> = st.log.iter().rev().take(c[6].height.saturating_sub(2) as usize).rev().map(|l| {
        let col = if l.contains("BLOCK FOUND") { GRN } else if l.contains("donation") { AMB } else if l.contains("rejected") || l.contains("stale") || l.contains("switching") { AMB } else { MUT };
        ListItem::new(Line::from(Span::styled(format!("  {}", l), Style::new().fg(col))))
    }).collect();
    f.render_widget(List::new(items).block(card("log", MUT)), c[6]);
}

// ── header: which pool MODE you're on, chain, connection, and — the thing a miner cares about — who gets paid ──
// 2 content rows on wide terminals; on narrow ones the address/balance and the payout get a row each (nothing truncates)
fn header_rows(width: u16) -> u16 { if width < 132 { 5 } else { 4 } }
fn render_header(f: &mut Frame, area: Rect, st: &Stats) {
    let m = st.mode; let ac = m.accent();
    // engine heartbeat: connected + unpaused + not gated, yet no loop iteration for 20s → something blocked the engine
    let stalled = st.connected && !st.paused && st.blake2b_active != Some(false)
        && st.last_tick.map(|t| t.elapsed() > Duration::from_secs(20)).unwrap_or(false);
    let dot = if st.paused { bold("⏸ PAUSED".into(), YLW) }
              else if st.blake2b_active == Some(false) { bold("⏳ WAITING · SHA-256d".into(), AMB) }
              else if stalled { bold("⚠ ENGINE STALLED".into(), Color::Red) }
              else if st.connected { Span::styled("● LIVE", Style::new().fg(GRN)) }
              else { Span::styled("● OFFLINE", Style::new().fg(Color::Red)) };
    let (nt, nco) = match st.network.as_str() { "mainnet" => (" MAINNET ", GRN), "testnet4" => (" TESTNET4 ", YLW), _ => (" REGTEST ", AMB) };
    let mut l1 = vec![bold(format!("{} {} BLAKE2b", m.icon(), m.label()), ac), Span::raw("  "),
        Span::styled(nt, Style::new().fg(Color::Black).bg(nco).add_modifier(Modifier::BOLD)), Span::raw("  "), dot,
        dim(&format!("   {}", st.endpoint))];
    if st.blake2b_active == Some(false) {
        l1.push(Span::styled(format!("   ⛔ BLAKE2b @ {} ({} to go) — not mining, saving power", st.activation_height, st.blocks_until_act), Style::new().fg(AMB)));
    }
    if st.update_available { l1.push(bold(format!("   ⬆ v{} — git pull && ./build.sh", st.latest_version), PNK)); }
    let bal = if st.balance_ok { format!("balance {:.8} BTC", st.balance_btc) } else { "balance —".to_string() };
    let narrow = header_rows(area.width) == 5;
    let mut l2 = vec![dim("your address  "), Span::styled(st.addr.clone(), Style::new().fg(CYN)),
        dim(&if st.worker.is_empty() { String::new() } else { format!(".{}", st.worker) }),
        Span::styled(format!("   {}", bal), Style::new().fg(GRN)), Span::raw("   ")];
    let mut l3 = vec![dim("payout        ")];
    if narrow { std::mem::swap(&mut l2, &mut l3); }   // narrow: l3 is now the address row, l2 collects the payout
    match m {
        // CHIRP: your LIVE slice of every block the syndicate finds
        PoolMode::Chirp => {
            l2.push(match st.chirp.as_ref() {
                None => dim("weighted split · loading the coinbase draw…"),
                Some(c) => match c.me(&st.addr) {
                    Some(me) if me.eligible => bold(format!("your slice of every block  {:.2}%", c.my_pct(&st.addr).unwrap_or(0.0)), PUR),
                    Some(me) => Span::styled(format!("joining the draw · {:.1} of {:.0} days", me.days, c.min_days), Style::new().fg(AMB)),
                    None => Span::styled("not in the coinbase draw yet · mine here to enter", Style::new().fg(AMB)),
                },
            });
            l2.push(dim(" · fee 0.9%"));
        }
        PoolMode::Custom => l2.push(dim(m.payout_short())),
        _ => l2.push(Span::styled(m.payout_short().to_string(), Style::new().fg(PNK))),
    }
    if st.donate > 0.0 { l2.push(Span::styled(format!(" · donation {:.1}% → PyBLØCK", st.donate), Style::new().fg(AMB))); }
    let lines = if narrow { vec![Line::from(l1), Line::from(l3), Line::from(l2)] } else { vec![Line::from(l1), Line::from(l2)] };
    f.render_widget(Paragraph::new(Text::from(lines)).block(card(&format!("⛏ Bitcoin BLAKE2b · {}", m.tagline()), ac)), area);
}

fn render_your_tiles(f: &mut Frame, area: Rect, st: &Stats) {
    let row = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(area);
    f.render_widget(tile("YOUR HASHRATE", Line::from(vec![bold(format!("{:.1}", st.hr_total), GRN), dim(" GH/s")]),
        &format!("{} worker(s)", st.gpu_ghs.len()), MUT), row[0]);
    f.render_widget(tile("BLOCKS FOUND", Line::from(bold(format!("{}", st.blocks), GRN)),
        &format!("{} shares acc · {} rej · {} don", st.accepted, st.rejected, st.donated), MUT), row[1]);
    f.render_widget(tile("DIFFICULTY", Line::from(bold(format!("bits {}", st.bits), YLW)),
        &format!("diff {:.0} · best {}", st.diff, fmt_diff(st.best_diff)), MUT), row[2]);
}

// ── network tiles, per MODE: LOTTO = the pyblockMiner network · CHIRP = the syndicate · CAROUSEL = the rotation ──
fn render_net_tiles(f: &mut Frame, area: Rect, st: &Stats) {
    let r = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(area);
    match (st.mode, st.chirp.as_ref(), st.carousel.as_ref()) {
        (PoolMode::Chirp, Some(c), _) => {
            let reward = c.reward_sats as f64 / 1e8;
            let cut = c.my_pct(&st.addr).map(|p| format!("your cut ≈ {:.5} BTC", reward * p / 100.0 * (1.0 - c.fee_bps as f64 / 10_000.0)))
                .unwrap_or_else(|| "split by weight among eligible miners".into());
            f.render_widget(tile("◈ IN THE COINBASE", Line::from(bold(format!("{}", c.candidates), PUR)), &format!("eligible miners · {} workers", c.workers), PUR), r[0]);
            f.render_widget(tile("◈ SYNDICATE HASHRATE", Line::from(bold(fmt_ths(c.hashrate_ths), PUR)), &format!("{} blocks found together", c.blocks), PUR), r[1]);
            f.render_widget(tile("◈ LAST BLOCK REWARD", Line::from(vec![bold(if reward > 0.0 { format!("{:.4}", reward) } else { "—".into() }, PUR), dim(" BTC")]), &cut, PUR), r[2]);
        }
        (PoolMode::Carousel, _, Some(k)) => {
            let cur = if k.current.is_empty() { "—".to_string() } else { k.current.clone() };
            f.render_widget(tile("◈ SUPPLIERS", Line::from(bold(format!("{}", k.suppliers.len()), WHT)), "clean templates in rotation", WHT), r[0]);
            f.render_widget(tile("◈ CAROUSEL HASHRATE", Line::from(bold(fmt_ths(k.hashrate_ths), WHT)), &format!("{} miners on the rotation", k.miners), WHT), r[1]);
            f.render_widget(tile("◈ NOW MINING", Line::from(bold(cur, WHT)), "this supplier's template · rotates every cycle", WHT), r[2]);
        }
        _ => {
            let (nm, ng, nh) = if st.net_ok { (format!("{}", st.net_miners), fmt_ths(st.net_ghs / 1e3), format!("{}", st.net_height)) }
                else { ("—".into(), "—".into(), "—".into()) };
            let sub = match st.mode { PoolMode::Chirp => "syndicate data loading…", PoolMode::Carousel => "rotation data loading…", _ => "PyBLØCK LOTTO network" };
            f.render_widget(tile("◈ MINERS ONLINE", Line::from(bold(nm, CYN)), "using pyblockMiner", CYN), r[0]);
            f.render_widget(tile("◈ NETWORK HASHRATE", Line::from(bold(ng, CYN)), "all pyblockMiner users", CYN), r[1]);
            f.render_widget(tile("◈ POOL HEIGHT", Line::from(bold(nh, CYN)), sub, CYN), r[2]);
        }
    }
}

// rows the mode panel wants (content only) — MINE sizes the panel from this, NETWORK gives it the whole tab
fn mode_panel_rows(st: &Stats, width: u16) -> usize {
    let w = (width.saturating_sub(2) as usize).max(20);
    let wrapped = |len: usize| (len.max(1) + w - 1) / w;   // rows a `len`-char line takes once wrapped
    match st.mode {
        PoolMode::Chirp => st.chirp.as_ref().map(|c| c.members.len() + 3).unwrap_or(1),
        PoolMode::Carousel => match st.carousel.as_ref() {
            Some(k) => {
                let rot = 14 + k.suppliers.iter().map(|s| s.chars().count() + 3).sum::<usize>() + 2;
                let trail = 14 + k.recent.iter().rev().take(10).map(|s| s.chars().count() + 3).sum::<usize>();
                1 + wrapped(rot) + wrapped(trail) + wrapped(14 + 92)
            }
            None => 1,
        },
        PoolMode::Lotto => 3,
        PoolMode::Custom => 2,
    }
}
fn render_mode_panel(f: &mut Frame, area: Rect, st: &Stats, app: &App) {
    match st.mode {
        PoolMode::Chirp => render_chirp_panel(f, area, st, app.list_scroll),
        PoolMode::Carousel => {
            let upd = st.carousel.as_ref().map(|k| format!(" · updated {} ago", fmt_ago(now_unix().saturating_sub(k.fetched)))).unwrap_or_default();
            f.render_widget(Paragraph::new(Text::from(carousel_lines(st))).wrap(Wrap { trim: false })
                .block(card(&format!("🎠 CAROUSEL · rotating clean templates{}", upd), WHT)), area)
        }
        PoolMode::Lotto => f.render_widget(Paragraph::new(Text::from(lotto_lines(st))).wrap(Wrap { trim: false })
            .block(card("🎰 LOTTO · solo lottery", YLW)), area),
        PoolMode::Custom => f.render_widget(Paragraph::new(Text::from(vec![
            Line::from(vec![dim("  stratum  "), Span::styled(st.endpoint.clone(), Style::new().fg(WHT))]),
            Line::from(vec![dim("  payout   "), dim(PoolMode::Custom.payout())]),
        ])).block(card("⛏ CUSTOM STRATUM", CYN)), area),
    }
}
fn lotto_lines(st: &Stats) -> Vec<Line<'static>> {
    let eta = eta_to_block(st.net_nbits, st.hr_total);
    let per_day = if eta.is_finite() && eta > 0.0 { 86_400.0 / eta } else { 0.0 };
    vec![
        Line::from(vec![dim("  how it pays  "), Span::styled("every winning share IS a block — the whole coinbase goes to your address", Style::new().fg(WHT))]),
        if eta.is_finite() {
            Line::from(vec![dim("  your odds    "), bold(format!("~{} per block", fmt_dur(eta)), YLW),
                dim(&format!("  at {:.1} GH/s · mean, high variance · ~{:.4} blocks/day", st.hr_total, per_day))])
        } else {
            Line::from(vec![dim("  your odds    "), dim("waiting for hashrate + a network target to estimate your time-to-block…")])
        },
        Line::from(vec![dim("  payout       "), Span::styled("you keep 99.1% · PyBLØCK fee 0.9% · non-custodial", Style::new().fg(PNK))]),
    ]
}
fn carousel_lines(st: &Stats) -> Vec<Line<'static>> {
    let Some(k) = st.carousel.as_ref() else { return vec![Line::from(dim("  loading the rotation from the pool…"))]; };
    let mut out = vec![Line::from(vec![dim("  now mining  "),
        bold(if k.current.is_empty() { "—".to_string() } else { format!("{}'s clean template", k.current) }, WHT),
        dim(&format!("   · {} suppliers · {} miners · {}{}", k.suppliers.len(), k.miners, fmt_ths(k.hashrate_ths), if k.live { "" } else { " · rotation paused" }))])];
    // the wheel: every supplier in the rotation, the live one lit
    let mut rot = vec![dim("  rotation    ")];
    for (i, s) in k.suppliers.iter().enumerate() {
        if i > 0 { rot.push(Span::styled(" · ", Style::new().fg(DIM))); }
        if *s == k.current { rot.push(bold(format!("▶ {}", s), WHT)); } else { rot.push(dim(s)); }
    }
    out.push(Line::from(rot));
    // recent is oldest → newest; show the last few so the trail ends at what's being mined now
    let trail: Vec<String> = k.recent.iter().rev().take(10).rev().cloned().collect();
    out.push(Line::from(vec![dim("  recent      "), Span::styled(trail.join(" › "), Style::new().fg(Color::Rgb(150, 150, 165)))]));
    out.push(Line::from(vec![dim("  payout      "), Span::styled("finder keeps 98% · supplier 1% · PyBLØCK 1% · split on-chain in the coinbase · non-custodial", Style::new().fg(PNK))]));
    out
}
// Expected BTC/day for a CHIRP member holding `pct`% of the split: syndicate blocks/day (mean, from its hashrate vs
// the network target) × reward × slice × (1 − fee). An honest mean — real luck swings wildly around it.
fn chirp_btc_per_day(st: &Stats, c: &ChirpInfo, pct: f64) -> f64 {
    let eta = eta_to_block(st.net_nbits, c.hashrate_ths * 1e3);
    if !eta.is_finite() || eta <= 0.0 || c.reward_sats == 0 { return 0.0; }
    (86_400.0 / eta) * (c.reward_sats as f64 / 1e8) * pct / 100.0 * (1.0 - c.fee_bps as f64 / 10_000.0)
}
fn fmt_btc(x: f64) -> String { if x <= 0.0 { "—".into() } else if x < 1e-4 { format!("{:.0} sats", x * 1e8) } else { format!("{:.5} BTC", x) } }
// ── CHIRP: EVERY miner in the coinbase draw — rank · address · tenure · power · share of the next block · status.
//    Your row is marked ▶. Eligible miners first (by weight), then the ones still earning their 7 days. ↑↓ scrolls. ──
fn render_chirp_panel(f: &mut Frame, area: Rect, st: &Stats, scroll: usize) {
    let inner_h = area.height.saturating_sub(2) as usize;
    let inner_w = area.width.saturating_sub(2) as usize;
    let Some(c) = st.chirp.as_ref() else {
        f.render_widget(Paragraph::new(Line::from(dim("  loading the coinbase draw from the pool…"))).block(card("🌌 CHIRP · who is in the coinbase", PUR)), area);
        return;
    };
    let full = inner_w >= 124;            // wide terminal → whole addresses
    let show_power = inner_w >= 96;
    let aw = if full { 62 } else { 13 };
    let sum = c.sum_weight(); let now = now_unix();
    let reward = c.reward_sats as f64 / 1e8;
    let keep = 1.0 - c.fee_bps as f64 / 10_000.0;
    let soft = Color::Rgb(120, 80, 170);
    // bars are relative to the leader (leader = full bar) so the distribution reads at a glance; the % is the truth
    let top_w = c.members.iter().filter(|m| m.eligible).map(|m| m.weight).fold(0.0f64, f64::max).max(1e-9);
    // ── summary: the draw, then YOU ──
    let mut head: Vec<Line<'static>> = vec![Line::from(vec![
        bold(format!("  {} ", c.candidates), PUR), dim("eligible miners share every block · "),
        Span::styled(fmt_ths(c.hashrate_ths), Style::new().fg(WHT)),
        dim(&format!(" · {} workers · {} blocks · last reward {}", c.workers, c.blocks, if reward > 0.0 { format!("{:.4} BTC", reward) } else { "—".into() })),
    ])];
    head.push(match c.me(&st.addr) {
        Some(m) if m.eligible => { let p = if sum > 0.0 { m.weight / sum * 100.0 } else { 0.0 };
            Line::from(vec![bold("  you  ".into(), PUR), bold(format!("{:.2}% of every block ≈ {:.5} BTC", p, reward * p / 100.0 * keep), WHT),
                Span::styled(format!(" · ≈ {} / day expected", fmt_btc(chirp_btc_per_day(st, c, p))), Style::new().fg(PUR)),
                dim(&format!(" · tenure {:.1} d · power {}", m.days, fmt_num(m.power)))]) }
        Some(m) => Line::from(vec![bold("  you  ".into(), AMB), Span::styled(format!("joining the draw · {:.1} of {:.0} days  {}  keep mining, stay connected",
                m.days, c.min_days, bar(m.days / c.min_days.max(0.1), 10)), Style::new().fg(AMB))]),
        None => Line::from(vec![dim("  you  "), dim(&format!("not in the draw yet — mine here {:.0} days to enter · the pool lists you once it sees your shares", c.min_days))]),
    });
    head.push(Line::from(Span::styled(format!("    {:>2}  {:<aw$}  {:>7}  {}{:<10} {:>7}   STATUS", "#", "MINER", "TENURE",
        if show_power { format!("{:>9}  ", "POWER 24h") } else { String::new() }, "SHARE", "", aw = aw), Style::new().fg(DIM))));
    // ── rows ──
    let rows: Vec<Line<'static>> = c.members.iter().enumerate().map(|(i, m)| {
        let is_me = !st.addr.is_empty() && m.addr == st.addr;
        let pct = if m.eligible && sum > 0.0 { m.weight / sum * 100.0 } else { 0.0 };
        let age = now.saturating_sub(m.last_seen);
        let (status, scol) = if !m.eligible {
            if m.days < c.min_days { (format!("⏳ {:.1} d to go", c.min_days - m.days), AMB) }
            else if m.power < c.min_power { ("⚠ below min power".to_string(), AMB) }
            else { ("⏳ pending".to_string(), AMB) }
        } else if age > 20 * 3600 { (format!("⚠ drops in {}h", 24u64.saturating_sub(age / 3600)), Color::Red) }
        else if age > 3600 { (format!("● offline {}", fmt_ago(age)), AMB) }
        else { ("● eligible".to_string(), GRN) };
        let name_st = if is_me { Style::new().fg(PUR).add_modifier(Modifier::BOLD) } else if m.eligible { Style::new().fg(WHT) } else { Style::new().fg(MUT) };
        let mut sp = vec![bold(if is_me { "  ▶ " } else { "    " }.into(), PUR), dim(&format!("{:>2}  ", i + 1)),
            Span::styled(format!("{:<aw$}  ", mask_addr(&m.addr, full), aw = aw), name_st), dim(&format!("{:>5.1} d  ", m.days))];
        if show_power { sp.push(dim(&format!("{:>9}  ", fmt_num(m.power)))); }
        if m.eligible {
            sp.push(Span::styled(bar(m.weight / top_w, 10), Style::new().fg(if is_me { PUR } else { soft })));
            sp.push(if is_me { bold(format!(" {:>6.2}%   ", pct), WHT) } else { Span::styled(format!(" {:>6.2}%   ", pct), Style::new().fg(WHT)) });
        } else {
            sp.push(Span::styled(bar((m.days / c.min_days.max(0.1)).min(1.0), 10), Style::new().fg(DIM)));
            sp.push(dim(&format!(" {:>6}   ", "—")));
        }
        sp.push(Span::styled(status, Style::new().fg(scol)));
        if is_me { sp.push(bold("  you".into(), PUR)); }
        Line::from(sp)
    }).collect();
    // ── scroll window ──
    let vis = inner_h.saturating_sub(head.len());
    let off = scroll.min(rows.len().saturating_sub(vis));
    let shown: Vec<Line<'static>> = rows.iter().skip(off).take(vis).cloned().collect();
    let scroll_hint = if rows.len() > vis && vis > 0 { format!(" · ↑↓ {}–{} of {}", off + 1, (off + vis).min(rows.len()), rows.len()) } else { String::new() };
    let title = format!("🌌 CHIRP · who is in the coinbase · {} miners{} · updated {} ago", c.members.len(), scroll_hint, fmt_ago(now.saturating_sub(c.fetched)));
    let mut lines = head; lines.extend(shown);
    f.render_widget(Paragraph::new(Text::from(lines)).block(card(&title, PUR)), area);
}

// human-readable duration (for uptime + solo-lottery ETA)
fn fmt_dur(s: f64) -> String {
    if !s.is_finite() || s <= 0.0 { return "—".into(); }
    let (y, d, h, m) = (31_557_600.0, 86_400.0, 3600.0, 60.0);
    if s >= y { format!("{:.1} yr", s / y) }
    else if s >= d { format!("{:.1} d", s / d) }
    else if s >= h { format!("{:.1} h", s / h) }
    else if s >= m { format!("{:.1} min", s / m) }
    else { format!("{:.0} s", s) }
}
// a 32-byte big-endian target as an f64 magnitude (precision-lossy, but we only need the order of magnitude)
fn target_f64(t: &[u8; 32]) -> f64 { let mut v = 0f64; for b in t { v = v * 256.0 + *b as f64; } v }
// expected seconds to find a block SOLO at `hr_ghs` GH/s against the network target (compact nbits).
// expected hashes per block = 2^256 / target ; time = hashes / (hashes per second). Mean of a geometric —
// real luck varies wildly around it, but it's the honest "your odds" number for a lottery miner.
fn eta_to_block(nbits: u32, hr_ghs: f64) -> f64 {
    if nbits == 0 || hr_ghs <= 0.0 { return f64::INFINITY; }
    let t = target_f64(&nbits_to_target(nbits));
    if t <= 0.0 { return f64::INFINITY; }
    (2f64.powi(256) / t) / (hr_ghs * 1e9)
}
// height (rows) of the per-worker box: one row per worker, clamped
fn gpu_rows(st: &Stats) -> u16 { (st.gpu_names.len().max(1) + 2).min(9) as u16 }

// ── DATA — session analytics (the "modo análisis de datos"): hashrate stats, share odds, per-worker breakdown ──
fn render_data(f: &mut Frame, area: Rect, st: &Stats) {
    let c = Layout::vertical([Constraint::Length(5), Constraint::Length(gpu_rows(st)), Constraint::Min(3)]).split(area);
    // hashrate stats from the live history buffer (stored as GH/s×100)
    let (peak, avg) = if st.hr_hist.is_empty() { (0.0, 0.0) } else {
        let mx = *st.hr_hist.iter().max().unwrap() as f64 / 100.0;
        let av = st.hr_hist.iter().sum::<u64>() as f64 / st.hr_hist.len() as f64 / 100.0;
        (mx, av)
    };
    let share = if st.net_ok && st.net_ghs > 0.0 { st.hr_total / st.net_ghs * 100.0 } else { 0.0 };
    let r = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(c[0]);
    f.render_widget(tile("AVG HASHRATE", Line::from(vec![Span::styled(format!("{:.2}", avg), Style::new().fg(GRN).add_modifier(Modifier::BOLD)),
        Span::styled(" GH/s", Style::new().fg(MUT))]), &format!("now {:.2} GH/s", st.hr_total), BRD), r[0]);
    f.render_widget(tile("PEAK HASHRATE", Line::from(vec![Span::styled(format!("{:.2}", peak), Style::new().fg(GRN).add_modifier(Modifier::BOLD)),
        Span::styled(" GH/s", Style::new().fg(MUT))]), "session peak", BRD), r[1]);
    f.render_widget(tile("YOUR NET SHARE", Line::from(vec![Span::styled(if st.net_ok { format!("{:.2}", share) } else { "—".into() }, Style::new().fg(CYN).add_modifier(Modifier::BOLD)),
        Span::styled(if st.net_ok { " %" } else { "" }, Style::new().fg(MUT))]), "of pyblockMiner network", CYN), r[2]);
    // per-worker breakdown (name · GH/s · share of your total)
    let mut wl: Vec<Line> = vec![];
    for (i, g) in st.gpu_ghs.iter().enumerate() {
        let name = st.gpu_names.get(i).cloned().unwrap_or_else(|| format!("worker {}", i));
        let pct = if st.hr_total > 0.0 { g / st.hr_total * 100.0 } else { 0.0 };
        wl.push(Line::from(vec![Span::styled(format!(" {:<26}", name), Style::new().fg(CYN)),
            Span::styled(format!("{:>8.2} GH/s", g), Style::new().fg(GRN)),
            Span::styled(format!("   {:>5.1}%", pct), Style::new().fg(MUT))]));
    }
    if wl.is_empty() { wl.push(Line::from(Span::styled(" warming up…", Style::new().fg(MUT)))); }
    f.render_widget(Paragraph::new(Text::from(wl)).block(card("WORKERS · hashrate share", MUT)), c[1]);
    // session analytics text
    let up = st.started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
    let total = st.accepted + st.rejected;
    let rej_rate = if total > 0 { st.rejected as f64 / total as f64 * 100.0 } else { 0.0 };
    let eta = eta_to_block(st.net_nbits, st.hr_total);
    let per_day = if eta.is_finite() && eta > 0.0 { 86_400.0 / eta } else { 0.0 };
    let kv = |k: &str, v: String, col: Color| Line::from(vec![
        Span::styled(format!("  {:<22}", k), Style::new().fg(MUT)), Span::styled(v, Style::new().fg(col))]);
    let mut lines = vec![
        kv("uptime", fmt_dur(up), GRN),
        kv("shares accepted", format!("{}", st.accepted), GRN),
        kv("shares rejected", format!("{}  ({:.1}%)", st.rejected, rej_rate), if rej_rate > 5.0 { AMB } else { MUT }),
        kv("blocks found", format!("{}", st.blocks), GRN),
        kv("best share difficulty", fmt_diff(st.best_diff), YLW),
        kv("current difficulty", format!("diff {:.0} · bits {}", st.diff, st.bits), YLW),
        Line::from(""),
        kv("expected time / block", format!("{}   (solo, mean — high variance)", fmt_dur(eta)), CYN),
        kv("expected blocks / day", if per_day > 0.0 { format!("{:.4}", per_day) } else { "—".into() }, CYN),
        kv("hashrate donated", format!("{} blocks → PyBLØCK", st.donated), AMB),
    ];
    // CHIRP: the number that matters is not YOUR time-to-block but the syndicate's, times your slice
    if let (PoolMode::Chirp, Some(c)) = (st.mode, st.chirp.as_ref()) {
        let s_eta = eta_to_block(st.net_nbits, c.hashrate_ths * 1e3);
        let s_day = if s_eta.is_finite() && s_eta > 0.0 { 86_400.0 / s_eta } else { 0.0 };
        let pct = c.my_pct(&st.addr);
        lines.push(Line::from(""));
        lines.push(kv("CHIRP syndicate", format!("{} · ~{} per block · ~{:.3} blocks/day", fmt_ths(c.hashrate_ths), fmt_dur(s_eta), s_day), PUR));
        lines.push(kv("your slice", pct.map(|p| format!("{:.2}% of every block", p)).unwrap_or_else(|| "not eligible yet".into()), PUR));
        lines.push(kv("expected income", pct.map(|p| format!("≈ {} / day · ≈ {} / month  (mean — high variance)", fmt_btc(chirp_btc_per_day(st, c, p)), fmt_btc(chirp_btc_per_day(st, c, p) * 30.0)))
            .unwrap_or_else(|| "—".into()), PUR));
    }
    lines.push(Line::from(""));
    lines.push(kv("alerts sent", format!("{}  (log: {})", st.alerts_sent, if st.log_file.is_some() { "miner.log" } else { "off" }), MUT));
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true })
        .block(card("ANÁLISIS · session data (press p to pause mining while you review)", CYN)), c[2]);
}

// ── STRATUMS: each pool as a two-line card row — name · url · chain, then WHAT IT DOES WITH THE COINBASE ──
fn render_stratums(f: &mut Frame, area: Rect, app: &App) {
    let mut items: Vec<ListItem> = vec![];
    let row_bg = Style::new().bg(Color::Rgb(20, 28, 20));
    for (i, s) in app.cfg.stratums.iter().enumerate() {
        let cur = i == app.strat_cur;
        let active = i == app.cfg.selected;
        let m = pool_mode(&s.url, &s.name); let ac = m.accent();
        let name_st = if active { Style::new().fg(ac).add_modifier(Modifier::BOLD) } else { Style::new().fg(WHT) };
        let mut l1 = Line::from(vec![
            Span::styled(if active { " ● " } else if cur { " › " } else { "   " }, Style::new().fg(if active { GRN } else { ac })),
            Span::raw(format!("{} ", m.icon())),
            Span::styled(format!("{:<24}", s.name), name_st),
            dim(&format!("{:<26}", s.url)),
            Span::styled(format!("{:<10}", s.network), Style::new().fg(AMB)),
            Span::styled(if s.custom { "custom  " } else { "        " }, Style::new().fg(DIM)),
            Span::styled(if active { "● LIVE" } else { "" }, Style::new().fg(GRN)),
        ]);
        let mut l2 = Line::from(vec![Span::raw("      "), Span::styled(format!("{:<28}", m.tagline()), Style::new().fg(ac)), dim(m.payout())]);
        if cur { l1 = l1.style(row_bg); l2 = l2.style(row_bg); }
        items.push(ListItem::new(Text::from(vec![l1, l2, Line::from("")])));
    }
    let help = Line::from(vec![dim("  ↑↓ move · "), Span::styled("Enter", Style::new().fg(GRN)), dim(" switch LIVE (no restart) · "),
        Span::styled("a", Style::new().fg(GRN)), dim(" add custom  name,host:port,network · "), Span::styled("d", Style::new().fg(GRN)), dim(" delete custom")]);
    let c = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(area);
    f.render_widget(List::new(items).block(card("STRATUMS · same BLAKE2b chain, three ways to get paid", GRN)), c[0]);
    f.render_widget(Paragraph::new(help).block(card("", MUT)), c[1]);
}

fn info_page(app: &App) -> (String, Vec<Line<'static>>) {
    let pages: Vec<(&str, Vec<&str>)> = vec![
        ("What is this?", vec![
            "pyblockMiner mines BLAKE2b — a *proposed* new Proof-of-Work for Bitcoin (Bitcoin Knots PR #359).",
            "It's solo-lottery: you mine to YOUR OWN address and keep 99.1% of any block you find,",
            "straight to that address. Non-custodial — the pool never holds your coins. PyBLØCK fee 0.9%.",
            "",
            "It saturates your GPU (NVIDIA / AMD / Intel via OpenCL on Linux & Windows, Apple Silicon/Metal on macOS) and/or CPU cores, with live hashrate/blocks/difficulty.",
        ]),
        ("The hardfork — honest status", vec![
            "BLAKE2b is NOT merged and NOT active on Bitcoin mainnet — there is no activation date.",
            "MAINNET is still SHA-256. Do not expect real rewards on mainnet yet.",
            "",
            "TESTNET4: the change activates on the public testnet4 chain at a flag-day block (Knots 29.4.1 RC).",
            "That is the FIRST place you can actually mine BLAKE2b on a public chain.",
            "⚠ testnet4 coins have NO monetary value — it's for testing / being ready.",
        ]),
        ("How solo-lottery mining works", vec![
            "Every share your GPU finds that beats the block target IS a block — paid entirely to your address.",
            "Blocks are rare (that's the lottery); when you hit one, you get the whole coinbase (minus 0.9% fee).",
            "There's no steady payout — it's all-or-nothing per block, like a lottery ticket that never expires.",
            "",
            "The 2% dev donation (mainnet only) sends a small slice of your hashrate to the PyBLØCK pool.",
        ]),
        ("Get started", vec![
            "1. SETUP [5]: pick your network + generate (or paste) an address for it.",
            "2. STRATUMS [2]: pick a pool (PyBLØCK defaults are there; add custom ones).",
            "3. MINE [1]: it connects and mines. Leave it running — on testnet4 it starts the moment BLAKE2b activates.",
            "",
            "No GPU? it falls back to CPU. Multiple GPUs are auto-detected and all saturated.",
        ]),
    ];
    let p = app.learn_page.min(pages.len() - 1);
    let (title, lines) = &pages[p];
    let out: Vec<Line<'static>> = lines.iter().map(|s| Line::from(Span::styled(s.to_string(), Style::new().fg(if s.starts_with('⚠') { AMB } else { GRN })))).collect();
    (format!(" LEARN — {}/{} · {}   (←/→ pages) ", p + 1, pages.len(), title), out)
}

fn render_learn(f: &mut Frame, area: Rect, app: &App) {
    let (title, lines) = info_page(app);
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true }).block(card(title.trim(), CYN)), area);
}

// ── NETWORK: the pool's numbers for your MODE, you, and the full mode panel (CHIRP: the whole coinbase list) ──
fn render_network(f: &mut Frame, area: Rect, st: &Stats, app: &App) {
    let c = Layout::vertical([Constraint::Length(5), Constraint::Length(4), Constraint::Min(4)]).split(area);
    render_net_tiles(f, c[0], st);
    let bal = if st.balance_ok { format!("{:.8} BTC", st.balance_btc) } else { "—".into() };
    let (nm, ng, nh) = if st.net_ok { (format!("{}", st.net_miners), fmt_ths(st.net_ghs / 1e3), format!("{}", st.net_height)) } else { ("—".into(), "—".into(), "—".into()) };
    let lines = vec![
        Line::from(vec![dim("  you     "), Span::styled(app.network(), Style::new().fg(GRN)), dim(" · "), Span::styled(app.addr(), Style::new().fg(CYN)),
            dim(" · balance "), Span::styled(bal, Style::new().fg(GRN)),
            dim(&format!(" · {} blocks · {} shares acc · {} rej", st.blocks, st.accepted, st.rejected))]),
        Line::from(vec![dim("  chain   "), dim(&format!("pyblockMiner network: {} miners · {} · pool height {} · stats + balance from the PyBLØCK BLAKE2b node for this chain", nm, ng, nh))]),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)).block(card("NETWORK & YOU", MUT)), c[1]);
    render_mode_panel(f, c[2], st, app);
}

fn render_setup(f: &mut Frame, area: Rect, app: &App) {
    let net = app.network();
    let addr = app.addr();
    let addr_disp = if addr.is_empty() { "— not set —".to_string() } else { addr.clone() };
    let donate_disp = if net_cfg(&net).donate { format!("{:.1}% (mainnet)", app.cfg.donate) } else { "off (testnet/regtest)".into() };
    let gpus_disp = app.cfg.gpus.map(|n| n.to_string()).unwrap_or_else(|| "auto".into());
    let a = &app.cfg.alerts;
    let onoff = |b: bool| if b { "on" } else { "off" };
    let row = |label: &str, v: Vec<Span<'static>>| { let mut sp = vec![dim(&format!("  {:<18}", label))]; sp.extend(v); Line::from(sp) };
    let key = |k: &str, what: &str| vec![Span::styled(format!("  {}", k), Style::new().fg(GRN)), dim(&format!(" {}", what))];
    let worker_v = if app.cfg.worker.is_empty() { vec![dim("— none — mining as the bare address  (w to name this rig)")] }
        else { vec![Span::styled(app.cfg.worker.clone(), Style::new().fg(GRN)), dim(&format!("   → login {}", login(&addr, &app.cfg.worker)))] };
    let tg = if a.telegram_token.is_empty() { "off".to_string() } else { format!("on → chat {}", a.telegram_chat) };
    let wh = if a.webhook_url.is_empty() { "off".to_string() } else { "on".to_string() };
    let logp = if app.cfg.log_file { config_path().parent().map(|p| p.join("miner.log").to_string_lossy().into_owned()).unwrap_or_default() } else { "off (--no-log-file)".into() };
    let api = if app.cfg.api_port > 0 { format!("http://127.0.0.1:{}/  ·  /metrics for Prometheus", app.cfg.api_port) } else { "off  (--api-port <port>)".into() };
    let lines = vec![
        row("selected stratum", vec![Span::styled(app.cfg.stratums.get(app.cfg.selected).map(|s| s.name.clone()).unwrap_or_default(), Style::new().fg(GRN)), Span::styled(format!("  [{}]", net), Style::new().fg(AMB))]),
        row("your address", vec![Span::styled(addr_disp, Style::new().fg(CYN))]),
        row("worker name", worker_v),
        row("donation", vec![Span::styled(donate_disp, Style::new().fg(AMB))]),
        row("gpus", vec![Span::styled(gpus_disp, Style::new().fg(GRN)), Span::styled(format!("   cpu: {}", onoff(app.cfg.cpu)), Style::new().fg(GRN))]),
        row("alerts", vec![Span::styled(format!("bell {} · desktop {} · telegram {} · webhook {}", onoff(a.bell), onoff(a.desktop), tg, wh), Style::new().fg(AMB)),
                           dim("   (blocks · GPU down/up · pool outage · CHIRP eligibility)")]),
        row("log file", vec![dim(&logp)]),
        row("local api", vec![dim(&api)]),
        Line::from(""),
        Line::from([key("g", "generate address   "), key("e", "edit/paste address   "), key("w", "worker name   "), key("c", "toggle CPU   "), key("+/-", "donation")].concat()),
        Line::from([key("b", "bell   "), key("n", "desktop notifications   "), key("t", "telegram token,chat   "), key("x", "send a test alert")].concat()),
        Line::from(dim("  changes auto-save + apply live · devices, log file and api port apply on restart")),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true }).block(card("SETUP · address, worker, alerts, config (saved)", GRN)), area);
}

fn render_help(f: &mut Frame, area: Rect) {
    let l = |a: &str, b: &str| Line::from(vec![Span::styled(format!("  {:<17} ", a), Style::new().fg(GRN)), Span::styled(b.to_string(), Style::new().fg(MUT))]);
    let lines = vec![
        Line::from(Span::styled(" Keys", Style::new().fg(CYN).add_modifier(Modifier::BOLD))),
        l("1-7 / Tab", "switch tabs (MINE · DATA · STRATUMS · LEARN · NETWORK · SETUP · HELP)"),
        l("p", "pause / resume mining (works from any tab — GPU/CPU go idle, pool stays connected)"),
        l("DATA", "session analytics: avg/peak hashrate, net share, per-worker split, solo ETA-to-block"),
        l("q / Esc", "quit (Esc also cancels an input)"),
        l("STRATUMS", "↑↓ move · Enter switch live · a add · d delete custom"),
        l("SETUP", "g generate · e edit address · w worker name · c CPU · +/- donation · b bell · n desktop · t telegram · x test alert"),
        l("MINE / NETWORK", "on CHIRP: ↑↓ PgUp PgDn Home scroll the coinbase list (everyone in the draw)"),
        Line::from(""),
        Line::from(Span::styled(" Alerts · log · API", Style::new().fg(CYN).add_modifier(Modifier::BOLD))),
        l("alerts", "block found · GPU down / back · pool unreachable / back · CHIRP: on the list, in the draw, falling out, dropped"),
        l("channels", "terminal bell · desktop (notify-send / macOS) · Telegram (SETUP t, or --telegram token,chat) · --webhook <url> (POST JSON)"),
        l("log file", "every log line, timestamped → <config dir>/miner.log (rotates at 5 MB) · --no-log-file"),
        l("--api-port N", "http://127.0.0.1:N/ live stats as JSON · /metrics for Prometheus/Grafana · localhost only"),
        l("--worker NAME", "login as addr.NAME so the pool tells your rigs apart"),
        Line::from(""),
        Line::from(Span::styled(" Pools — same BLAKE2b chain, three ways to get paid", Style::new().fg(CYN).add_modifier(Modifier::BOLD))),
        l("🎰 LOTTO :4445", PoolMode::Lotto.payout()),
        l("🌌 CHIRP :5574", PoolMode::Chirp.payout()),
        l("🎠 CAROUSEL :30110", PoolMode::Carousel.payout()),
        l("coinbase panel", "MINE shows who the next block pays: CHIRP lists every eligible miner + share; CAROUSEL the live template"),
        Line::from(""),
        Line::from(Span::styled(" Troubleshooting", Style::new().fg(CYN).add_modifier(Modifier::BOLD))),
        l("build fails", "CL/cl.h missing → sudo apt install ocl-icd-opencl-dev opencl-headers"),
        l("no GPU", "it falls back to CPU automatically (much slower)"),
        l("connection failed", "the pool may be down/gated; it retries every 3s (no GPU wasted while offline)"),
        l("wrong address", "each network needs its own type: bc1 mainnet · tb1 testnet4 · bcrt1 regtest"),
        l("testnet4 addr", "SETUP [5] → g, or:  pyblockMiner --genaddr testnet4"),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(" Update", Style::new().fg(CYN).add_modifier(Modifier::BOLD))),
        l("new version", "the MINE header shows ⬆ v<x> when a newer release is published on the pool"),
        l("update command", "cd pyblock-miner && git pull && ./build.sh    (then relaunch the miner)"),
        Line::from(""),
        Line::from(vec![Span::styled("  GitHub   ", Style::new().fg(MUT)), Span::styled("github.com/GaltRanch/pyblock-miner", Style::new().fg(CYN))]),
        Line::from(vec![Span::styled("  Pool     ", Style::new().fg(MUT)), Span::styled("pool.pyblock.xyz  ·  MIT licensed", Style::new().fg(CYN))]),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true }).block(card("HELP", CYN)), area);
}

// apply the current selected stratum + address to the live engine target + stats display
fn apply_target(app: &App, tgt: &Arc<Mutex<Target>>, stats: &Arc<Mutex<Stats>>) {
    let s = match app.cfg.stratums.get(app.cfg.selected) { Some(s) => s.clone(), None => return };
    let addr = app.addr();
    let donate = if net_cfg(&s.network).donate { app.cfg.donate.max(DONATE_MIN) } else { 0.0 };
    let worker = clean_worker(&app.cfg.worker);
    { let mut t = tgt.lock().unwrap(); t.pool = s.url.clone(); t.addr = addr.clone(); t.worker = worker.clone(); t.network = s.network.clone(); t.donate = donate; }
    { let mut st = stats.lock().unwrap(); st.endpoint = s.url.clone(); st.addr = addr; st.worker = worker; st.alerts = app.cfg.alerts.clone();
      st.network = s.network.clone(); st.donate = donate; st.balance_ok = false; st.net_ok = false;
      // pool mode drives the MINE/NETWORK panels; drop the old mode's data so the new one starts clean (poller refills within ~1s)
      st.mode = pool_mode(&s.url, &s.name); st.chirp = None; st.carousel = None; }
}

// append a generated key to ~/.config/pyblockminer/keys.txt (0600). Returns the path on success.
fn save_wif(net: &str, addr: &str, wif: &str) -> Option<String> {
    let path = config_path().parent()?.join("keys.txt");
    if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)); }
    f.write_all(format!("{}\t{}\t{}\n", net, addr, wif).as_bytes()).ok()?;
    Some(path.to_string_lossy().into_owned())
}
// generate a P2WPKH address + its WIF natively (pure Rust via rust-bitcoin — no Python needed).
// testnet4 uses the testnet3 address format (tb1 / WIF 0xef); regtest → bcrt1; else mainnet bc1.
fn gen_address(net: &str) -> Option<(String, String)> {
    use bitcoin::{Network, PrivateKey, CompressedPublicKey, Address};
    use bitcoin::secp256k1::{Secp256k1, rand};
    let network = match net {
        "testnet4" => Network::Testnet,
        "regtest"  => Network::Regtest,
        _          => Network::Bitcoin,
    };
    let secp = Secp256k1::new();
    let (sk, pk) = secp.generate_keypair(&mut rand::thread_rng());
    let privkey = PrivateKey::new(sk, network);
    let comp = CompressedPublicKey(pk);
    let addr = Address::p2wpkh(&comp, network);
    Some((addr.to_string(), privkey.to_wif()))
}

fn handle_key(app: &mut App, code: KeyCode, tgt: &Arc<Mutex<Target>>, stats: &Arc<Mutex<Stats>>) -> bool {
    app.msg.clear();
    // input mode: type into the buffer
    if let Some(kind) = &app.input {
        match code {
            KeyCode::Esc => { app.input = None; app.buf.clear(); }
            KeyCode::Backspace => { app.buf.pop(); }
            KeyCode::Enter => {
                let buf = app.buf.clone();
                match kind {
                    Input::AddStratum => {
                        let parts: Vec<&str> = buf.splitn(3, ',').map(|s| s.trim()).collect();
                        if parts.len() == 3 && parts[1].contains(':') {
                            let net = net_cfg(parts[2]).name.to_string();
                            app.cfg.stratums.push(Stratum { name: parts[0].into(), url: parts[1].into(), network: net, custom: true });
                            save_config(&app.cfg); app.msg = "stratum added".into();
                        } else { app.msg = "format: name,host:port,network".into(); }
                    }
                    Input::EditAddr => {
                        let net = app.network();
                        if addr_ok(&net, buf.trim()) {
                            app.cfg.addrs.insert(net, buf.trim().to_string());
                            save_config(&app.cfg); apply_target(app, tgt, stats); app.msg = "address saved".into();
                        } else { app.msg = format!("not a valid {} address", app.network()); }
                    }
                    Input::EditWorker => {
                        app.cfg.worker = clean_worker(buf.trim()); save_config(&app.cfg); apply_target(app, tgt, stats);
                        app.msg = if app.cfg.worker.is_empty() { "worker cleared — mining as your bare address".into() }
                                  else { format!("worker saved — mining as {}", login(&app.addr(), &app.cfg.worker)) };
                    }
                    Input::EditTelegram => {
                        let parts: Vec<&str> = buf.split(',').map(|s| s.trim()).collect();
                        if buf.trim().is_empty() { app.cfg.alerts.telegram_token.clear(); app.cfg.alerts.telegram_chat.clear(); app.msg = "telegram alerts off".into(); }
                        else if parts.len() == 2 && parts[0].contains(':') && !parts[1].is_empty() {
                            app.cfg.alerts.telegram_token = parts[0].into(); app.cfg.alerts.telegram_chat = parts[1].into();
                            app.msg = "telegram alerts on — a test message is on its way".into();
                            let mut st = stats.lock().unwrap(); st.alerts = app.cfg.alerts.clone();
                            alert(&mut st, "alerts armed", "pyblockMiner will notify you here: blocks, GPU down/up, pool outages, CHIRP eligibility");
                        } else { app.msg = "format: <bot_token>,<chat_id>   (empty to disable)".into(); }
                        save_config(&app.cfg); apply_target(app, tgt, stats);
                    }
                }
                app.input = None; app.buf.clear();
            }
            KeyCode::Char(ch) => { if app.buf.len() < 120 { app.buf.push(ch); } }
            _ => {}
        }
        return false;
    }
    // normal mode
    match code {
        KeyCode::Char('q') | KeyCode::Esc => return true,
        KeyCode::Char(c @ '1'..='7') => { app.tab = TABS[c as usize - '1' as usize].0; }
        KeyCode::Tab => { let i = TABS.iter().position(|(t, _)| *t == app.tab).unwrap_or(0); app.tab = TABS[(i + 1) % TABS.len()].0; }
        // pause/resume mining — works from any tab (the engine picks it up within ~120ms; connection stays alive)
        KeyCode::Char('p') => {
            let v = !app.paused.load(Ordering::Relaxed);
            app.paused.store(v, Ordering::Relaxed);
            app.msg = if v { "⏸ mining paused — press p to resume".into() } else { "▶ mining resumed".into() };
        }
        _ => match app.tab {
            Tab::Stratums => match code {
                KeyCode::Up => { if app.strat_cur > 0 { app.strat_cur -= 1; } }
                KeyCode::Down => { if app.strat_cur + 1 < app.cfg.stratums.len() { app.strat_cur += 1; } }
                KeyCode::Enter => { app.cfg.selected = app.strat_cur; save_config(&app.cfg); apply_target(app, tgt, stats); app.list_scroll = 0;
                    let s = &app.cfg.stratums[app.strat_cur];
                    app.msg = format!("switched to {} · {}", s.name, pool_mode(&s.url, &s.name).payout()); }
                KeyCode::Char('a') => { app.input = Some(Input::AddStratum); app.buf.clear(); }
                KeyCode::Char('d') => {
                    if let Some(s) = app.cfg.stratums.get(app.strat_cur) { if s.custom {
                        app.cfg.stratums.remove(app.strat_cur);
                        if app.cfg.selected >= app.cfg.stratums.len() { app.cfg.selected = 0; }
                        if app.strat_cur >= app.cfg.stratums.len() && app.strat_cur > 0 { app.strat_cur -= 1; }
                        save_config(&app.cfg); app.msg = "deleted".into();
                    } else { app.msg = "can't delete a default stratum".into(); } }
                }
                _ => {}
            },
            // CHIRP coinbase list: scroll through everyone in the draw (MINE panel + NETWORK full view)
            Tab::Mine | Tab::Network => {
                let n = stats.lock().unwrap().chirp.as_ref().map(|c| c.members.len()).unwrap_or(0);
                match code {
                    KeyCode::Up => { app.list_scroll = app.list_scroll.saturating_sub(1); }
                    KeyCode::Down => { if app.list_scroll + 1 < n { app.list_scroll += 1; } }
                    KeyCode::PageUp => { app.list_scroll = app.list_scroll.saturating_sub(10); }
                    KeyCode::PageDown => { app.list_scroll = (app.list_scroll + 10).min(n.saturating_sub(1)); }
                    KeyCode::Home => { app.list_scroll = 0; }
                    KeyCode::End => { app.list_scroll = n.saturating_sub(1); }
                    _ => {}
                }
            }
            Tab::Learn => match code {
                KeyCode::Left => { if app.learn_page > 0 { app.learn_page -= 1; } }
                KeyCode::Right => { if app.learn_page + 1 < 4 { app.learn_page += 1; } }   // 4 LEARN pages
                _ => {}
            },
            Tab::Setup => match code {
                KeyCode::Char('g') => {
                    let net = app.network();
                    app.msg = "generating…".into();
                    if let Some((a, w)) = gen_address(&net) {
                        app.cfg.addrs.insert(net.clone(), a.clone()); save_config(&app.cfg); apply_target(app, tgt, stats);
                        app.msg = match save_wif(&net, &a, &w) {
                            Some(p) => format!("generated {} · WIF saved to {} — BACK IT UP", a, p),
                            None    => format!("generated {} · WIF {} — SAVE THIS (could not write keys.txt!)", a, w),
                        };
                    }
                    else { app.msg = "address generation failed. Use 'e' to paste one.".into(); }
                }
                KeyCode::Char('e') => { app.input = Some(Input::EditAddr); app.buf.clear(); }
                KeyCode::Char('c') => { app.cfg.cpu = !app.cfg.cpu; save_config(&app.cfg); app.msg = "cpu toggled (restart to apply devices)".into(); }
                KeyCode::Char('w') => { app.input = Some(Input::EditWorker); app.buf = app.cfg.worker.clone(); }
                KeyCode::Char('t') => { app.input = Some(Input::EditTelegram); app.buf.clear(); }
                KeyCode::Char('b') => { app.cfg.alerts.bell = !app.cfg.alerts.bell; save_config(&app.cfg); apply_target(app, tgt, stats);
                                        app.msg = format!("bell {}", if app.cfg.alerts.bell { "on" } else { "off" }); }
                KeyCode::Char('n') => { app.cfg.alerts.desktop = !app.cfg.alerts.desktop; save_config(&app.cfg); apply_target(app, tgt, stats);
                                        app.msg = format!("desktop notifications {}", if app.cfg.alerts.desktop { "on" } else { "off" }); }
                KeyCode::Char('x') => { let mut st = stats.lock().unwrap(); alert(&mut st, "test alert", "if you can read this, alerts work"); app.msg = "test alert sent — check bell / desktop / telegram".into(); }
                KeyCode::Char('+') => { app.cfg.donate += 1.0; save_config(&app.cfg); apply_target(app, tgt, stats); }
                KeyCode::Char('-') => { app.cfg.donate = (app.cfg.donate - 1.0).max(DONATE_MIN); save_config(&app.cfg); apply_target(app, tgt, stats); }
                _ => {}
            },
            _ => {}
        }
    }
    false
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut cfg = load_config();
    let mut headless = false;
    let mut genaddr_net: Option<String> = None;
    // CLI overrides (optional; config is the source of truth otherwise)
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--addr" => { i += 1; if i < args.len() { let net = cfg.stratums.get(cfg.selected).map(|s| s.network.clone()).unwrap_or_else(|| "mainnet".into()); cfg.addrs.insert(net, args[i].clone()); } }
            "--network" | "--net" | "--chain" => { i += 1; if i < args.len() { let n = net_cfg(&args[i]).name; if let Some(idx) = cfg.stratums.iter().position(|s| s.network == n) { cfg.selected = idx; } } }
            "--pool" => { i += 1; if i < args.len() {
                // An ad-hoc pool becomes a CUSTOM stratum (deduped by URL) and is selected. Overwriting the selected
                // default's URL in place got persisted on the next auto-save, and reconcile_defaults then re-added the
                // real default → duplicated LOTTO entries.
                let url = args[i].clone();
                let net = cfg.stratums.get(cfg.selected).map(|s| s.network.clone()).unwrap_or_else(|| "mainnet".into());
                cfg.selected = match cfg.stratums.iter().position(|s| s.url == url) {
                    Some(idx) => idx,
                    None => { cfg.stratums.push(Stratum { name: format!("--pool {}", url), url, network: net, custom: true }); cfg.stratums.len() - 1 }
                };
            } }
            "--gpus" => { i += 1; if i < args.len() { cfg.gpus = args[i].parse().ok(); } }
            "--cpu" => { cfg.cpu = true; }
            "--worker" => { i += 1; if i < args.len() { cfg.worker = clean_worker(&args[i]); } }
            "--api-port" => { i += 1; if i < args.len() { cfg.api_port = args[i].parse().unwrap_or(0); } }
            "--telegram" => { i += 1; if i < args.len() { let mut p = args[i].splitn(2, ',');
                cfg.alerts.telegram_token = p.next().unwrap_or("").trim().to_string(); cfg.alerts.telegram_chat = p.next().unwrap_or("").trim().to_string(); } }
            "--webhook" => { i += 1; if i < args.len() { cfg.alerts.webhook_url = args[i].clone(); } }
            "--no-log-file" => { cfg.log_file = false; }
            "--no-bell" => { cfg.alerts.bell = false; }
            "--no-desktop" => { cfg.alerts.desktop = false; }
            "--donate" => { i += 1; if i < args.len() { cfg.donate = args[i].parse().unwrap_or(DONATE_MIN); } }
            "--genaddr" | "--newaddr" => { let n = if i + 1 < args.len() && !args[i + 1].starts_with("--") { i += 1; net_cfg(&args[i]).name.to_string() } else { "mainnet".to_string() }; genaddr_net = Some(n); }
            "--version" | "-V" => { println!("pyblockMiner {}", VERSION); return; }
            "--headless" | "--daemon" | "--no-tui" => { headless = true; }
            _ => {}
        }
        i += 1;
    }

    // --genaddr [net]: print a fresh address + WIF (saved to keys.txt 0600) and exit. Pure Rust, no Python, no pool.
    if let Some(net) = genaddr_net {
        match gen_address(&net) {
            Some((a, w)) => {
                let saved = save_wif(&net, &a, &w);
                println!("── PyBLØCK · BLAKE2b mining address · {} ──", net.to_uppercase());
                println!("  address : {}", a);
                println!("  WIF     : {}", w);
                match saved {
                    Some(p) => println!("  saved   : {} (0600) — BACK IT UP", p),
                    None    => println!("  ⚠ could not write keys.txt — SAVE THE WIF ABOVE"),
                }
                println!("  mine    : pyblockMiner --network {} --addr {} --pool <host:port>", net, a);
            }
            None => { eprintln!("genaddr failed"); std::process::exit(1); }
        }
        return;
    }
    if cfg.selected >= cfg.stratums.len() { cfg.selected = 0; }

    let paused = Arc::new(AtomicBool::new(false));   // shared pause flag: `p` toggles, engine idles the grinders
    let mut app = App { tab: Tab::Mine, cfg, strat_cur: 0, learn_page: 0, input: None, buf: String::new(), msg: String::new(), paused: paused.clone(), list_scroll: 0 };
    app.strat_cur = app.cfg.selected;

    // devices — attempt the GPU grinder even if no GPU is name-detected (it enumerates OpenCL/Metal and
    // signals READY; if there's no device or it isn't built, spawn_daemon drops it and the engine falls
    // back to CPU). `--cpu` forces CPU-only.
    let detected = gpu_names().len() as u32;
    let ngpu = if app.cfg.cpu { 0 } else { app.cfg.gpus.unwrap_or(if detected > 0 { detected } else { 1 }) };
    let mut use_cpu = app.cfg.cpu;
    if ngpu == 0 { use_cpu = true; }
    let cpu_threads = if use_cpu { std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) } else { 0 };

    // shared state
    let stats = Arc::new(Mutex::new(Stats::default()));
    let net0 = app.network();
    let donate0 = if net_cfg(&net0).donate { app.cfg.donate.max(DONATE_MIN) } else { 0.0 };
    let tgt = Arc::new(Mutex::new(Target {
        pool: app.cfg.stratums.get(app.cfg.selected).map(|s| s.url.clone()).unwrap_or_default(),
        addr: app.addr(), worker: clean_worker(&app.cfg.worker), network: net0.clone(), donate: donate0,
    }));
    { let mut st = stats.lock().unwrap(); st.endpoint = tgt.lock().unwrap().pool.clone(); st.addr = app.addr(); st.network = net0; st.donate = donate0;
      st.mode = app.cfg.stratums.get(app.cfg.selected).map(|s| pool_mode(&s.url, &s.name)).unwrap_or_default();
      st.worker = clean_worker(&app.cfg.worker); st.alerts = app.cfg.alerts.clone(); st.api_port = app.cfg.api_port;
      st.log_file = if app.cfg.log_file { open_log_file() } else { None };
      let banner = format!("pyblockMiner v{} starting · {} · login {}", VERSION, st.endpoint, login(&st.addr, &st.worker));
      st.logline(banner); }

    { let stats = stats.clone(); let tgt = tgt.clone(); let paused = paused.clone(); std::thread::spawn(move || engine(stats, tgt, ngpu, cpu_threads, paused)); }
    if app.cfg.api_port > 0 { let stats = stats.clone(); let port = app.cfg.api_port; std::thread::spawn(move || api_server(port, stats)); }
    // mode poller: CHIRP coinbase draw / CAROUSEL rotation for the ACTIVE stratum. Refreshes every 15s (the pool's
    // own pages do the same) and immediately on a stratum switch. A failed poll keeps the last good data on screen.
    { let stats = stats.clone(); std::thread::spawn(move || {
        let mut last_mode: Option<PoolMode> = None; let mut last_poll = Instant::now();
        // your CHIRP state at the previous poll: (listed, eligible, stale>1h) — transitions become alerts
        let mut chirp_prev: Option<(bool, bool, bool)> = None;
        loop {
            let mode = stats.lock().unwrap().mode;
            if last_mode != Some(mode) || last_poll.elapsed() >= Duration::from_secs(15) {
                if last_mode != Some(mode) { chirp_prev = None; }
                last_mode = Some(mode); last_poll = Instant::now();
                match mode {
                    PoolMode::Chirp => {
                        let r = poll_chirp();
                        let mut st = stats.lock().unwrap();
                        if st.mode == PoolMode::Chirp { if let Some(c) = r {
                            let now = now_unix();
                            let me = c.me(&st.addr).map(|m| (m.eligible, now.saturating_sub(m.last_seen) > 3600, m.days));
                            let cur = me.map(|(e, s, _)| (true, e, s)).unwrap_or((false, false, false));
                            if let Some(prev) = chirp_prev { if prev != cur {
                                let pct = c.my_pct(&st.addr).unwrap_or(0.0);
                                let days = me.map(|m| m.2).unwrap_or(0.0);
                                match (prev, cur) {
                                    ((_, false, _), (true, true, _)) => alert(&mut st, "CHIRP · you're IN the coinbase draw", &format!("your slice of every block: {:.2}%", pct)),
                                    ((false, _, _), (true, false, _)) => alert(&mut st, "CHIRP · you're on the list", &format!("{:.1} of {:.0} days to the coinbase draw — keep mining", days, c.min_days)),
                                    ((true, _, _), (false, _, _))     => alert(&mut st, "CHIRP · dropped off the list", "the pool no longer lists your address — reconnect and mine to re-enter"),
                                    ((_, true, _), (true, false, _))  => alert(&mut st, "CHIRP · eligibility lost", &format!("tenure {:.1} d · check your power / connection", days)),
                                    ((_, _, false), (true, _, true))  => alert(&mut st, "CHIRP · falling out", "no shares from you for 1h+ — you drop off the list 24h after your last share"),
                                    _ => {}
                                }
                            } }
                            chirp_prev = Some(cur);
                            st.chirp = Some(c);
                        } }
                    }
                    PoolMode::Carousel => { let r = poll_carousel(); let mut st = stats.lock().unwrap(); if st.mode == PoolMode::Carousel && r.is_some() { st.carousel = r; } }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(400));
        }
    }); }
    // network-stats poller (per current network)
    { let stats = stats.clone(); let tgt = tgt.clone(); std::thread::spawn(move || loop {
        let net = tgt.lock().unwrap().network.clone();
        match net_stats_url(&net).and_then(poll_network_stats) {
            Some(ns) => { let mut st = stats.lock().unwrap();
                st.net_ok = true; st.net_miners = ns.miners; st.net_ghs = ns.ghs; st.net_height = ns.height;
                st.blake2b_active = ns.blake2b_active; st.activation_height = ns.activation_height; st.blocks_until_act = ns.blocks_until;
                if !ns.latest.is_empty() { st.update_available = is_newer(&ns.latest, VERSION); st.latest_version = ns.latest; } }
            None => { stats.lock().unwrap().net_ok = false; }
        }
        std::thread::sleep(Duration::from_secs(8));   // match blake_stats' ~8s cache so POOL HEIGHT (+ the BLOCK FOUND height) stay fresh on fast chains
    }); }
    // balance poller (per current network + address)
    { let stats = stats.clone(); let tgt = tgt.clone(); std::thread::spawn(move || loop {
        let (net, addr) = { let t = tgt.lock().unwrap(); (t.network.clone(), t.addr.clone()) };
        match poll_balance(&net, &addr) {
            Some(b) => { let mut st = stats.lock().unwrap(); st.balance_ok = true; st.balance_btc = b; }
            None => { stats.lock().unwrap().balance_ok = false; }
        }
        std::thread::sleep(Duration::from_secs(60));
    }); }

    // headless (service) mode: no TUI, heartbeat to stdout (journald). Engine + pollers already running.
    if headless {
        println!("pyblockMiner (headless) · network={} · pool={} · addr={} · donate={:.1}%",
            app.network(), tgt.lock().unwrap().pool, app.addr(), donate0);
        let _ = std::io::stdout().flush();
        let mut last_printed = 0usize;
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let (net, conn, hr, nworkers, blk, acc, rej, neth, bd, newlogs) = {
                let st = stats.lock().unwrap();
                let logs: Vec<String> = st.log.iter().cloned().collect();
                let fresh: Vec<String> = if logs.len() > last_printed { logs[last_printed..].to_vec() } else { vec![] };
                last_printed = logs.len();
                (st.network.clone(), st.connected, st.hr_total, st.gpu_ghs.len(), st.blocks, st.accepted, st.rejected, st.net_height, st.best_diff, fresh)
            };
            for l in &newlogs { println!("  · {}", l); }
            println!("{} · {} · {:.1} GH/s ({}w) · blocks {} · {} acc (rej {}) · net_h {} · best {}",
                net, if conn { "LIVE" } else { "waiting" }, hr, nworkers, blk, acc, rej, neth, fmt_diff(bd));
            let _ = std::io::stdout().flush();
        }
    }

    // restore the terminal (leave raw mode / alt screen) even if a thread or handler panics
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| { ratatui::restore(); default_hook(info); }));
    let mut terminal = ratatui::init();
    loop {
        let ring = { let mut st = stats.lock().unwrap(); let _ = terminal.draw(|f| ui(f, &app, &st)); std::mem::take(&mut st.ring_bell) };
        if ring { let mut o = std::io::stdout(); let _ = o.write_all(b"\x07"); let _ = o.flush(); }   // alert → terminal bell
        if event::poll(Duration::from_millis(150)).unwrap_or(false) {
            if let Ok(Event::Key(k)) = event::read() {
                // Windows consoles emit Press AND Release (and Repeat) for one physical keypress; Unix emits
                // only Press. Without this guard, Tab/arrows/typing fire 2-4× per press (numkeys hid it — they're
                // absolute/idempotent). Filter to Press so every platform behaves like Unix.
                if k.kind == KeyEventKind::Press && handle_key(&mut app, k.code, &tgt, &stats) { break; }
            }
        }
    }
    ratatui::restore();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_pot_is_log2_floor() {
        assert_eq!(floor_pot(1.0), 0); assert_eq!(floor_pot(2.0), 1); assert_eq!(floor_pot(3.0), 1);
        assert_eq!(floor_pot(4096.0), 12); assert_eq!(floor_pot(0.5), 0);
    }
    #[test]
    fn share_target_matches_difficulty() {
        // diff 1 ⇔ bits 0 ⇔ target 2^224-1 : top 4 bytes zero, rest 0xff
        let t = target_be(0);
        assert_eq!(&t[..4], &[0, 0, 0, 0]); assert!(t[4..].iter().all(|&b| b == 0xff));
        // each extra bit halves the target (shifts the 0xff run right by one bit)
        let t1 = target_be(1); assert_eq!(t1[4], 0x7f);
        let t12 = target_be(12); assert_eq!(&t12[..5], &[0, 0, 0, 0, 0]); assert_eq!(t12[5], 0x0f);
    }
    #[test]
    fn nbits_decodes_like_bitcoin() {
        // 0x1d00ffff (genesis): mantissa 0x00ffff at byte offset 32-29=3
        let t = nbits_to_target(0x1d00ffff);
        assert_eq!(&t[..3], &[0, 0, 0]); assert_eq!(&t[3..6], &[0x00, 0xff, 0xff]); assert!(t[6..].iter().all(|&b| b == 0));
    }
    #[test]
    fn hash_diff_is_monotonic_and_calibrated() {
        // a hash exactly at the diff-1 boundary (top 32 bits zero, then 0xff…) ≈ diff 1
        let mut h = [0xffu8; 32]; h[..4].copy_from_slice(&[0, 0, 0, 0]);
        let d1 = hash_diff(&h); assert!((d1 - 1.0).abs() < 1e-6, "{}", d1);
        let mut h2 = h; h2[4] = 0x7f;   // one more leading zero bit → diff ≈ 2
        assert!(hash_diff(&h2) > 1.99 && hash_diff(&h2) < 2.01);
        assert!(hash_le_target(&h, &target_be(0)) && !hash_le_target(&h, &target_be(1)));
    }
    #[test]
    fn pool_mode_from_port_then_name() {
        assert_eq!(pool_mode("pool.pyblock.xyz:5574", "x"), PoolMode::Chirp);
        assert_eq!(pool_mode("pool.pyblock.xyz:30110", "x"), PoolMode::Carousel);
        assert_eq!(pool_mode("pool.pyblock.xyz:4445", "x"), PoolMode::Lotto);
        assert_eq!(pool_mode("10.0.0.5:3333", "my CHIRP relay"), PoolMode::Chirp);
        assert_eq!(pool_mode("10.0.0.5:3333", "home node"), PoolMode::Custom);
    }
    #[test]
    fn version_compare_is_semver() {
        assert!(is_newer("0.2.20", "0.2.19")); assert!(is_newer("v0.3.0", "0.2.99")); assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.2.19", "0.2.19")); assert!(!is_newer("0.2.18", "0.2.19")); assert!(!is_newer("", "0.2.19"));
    }
    #[test]
    fn address_masking_and_labels() {
        assert_eq!(mask_addr("bc1qjdqlvwfxum8dh4t5v9mvskdarjvlek2a9g5pw2", false), "bc1qjd…5pw2");
        assert_eq!(mask_addr("bc1qjdqlvwfxum8dh4t5v9mvskdarjvlek2a9g5pw2", true).len(), 42);
        assert_eq!(mask_addr("short", false), "short");
        assert_eq!(clean_label("  Ec1ipse\x1b[31m\u{7f} "), "Ec1ipse[31m");   // control chars gone, text kept
        assert_eq!(clean_label(&"a".repeat(100)).len(), 40);
    }
    #[test]
    fn formatting_helpers() {
        assert_eq!(fmt_ths(8.42), "8.42 TH/s"); assert_eq!(fmt_ths(0.5), "500.00 GH/s"); assert_eq!(fmt_ths(1500.0), "1.50 PH/s"); assert_eq!(fmt_ths(0.0), "—");
        assert_eq!(bar(0.0, 10), "▱▱▱▱▱▱▱▱▱▱"); assert_eq!(bar(1.0, 4), "▰▰▰▰"); assert_eq!(bar(0.5, 4), "▰▰▱▱"); assert_eq!(bar(7.0, 3), "▰▰▰");
        assert_eq!(fmt_num(57_302_984.0), "57.3M"); assert_eq!(fmt_ago(59), "59s"); assert_eq!(fmt_ago(7200), "2h");
    }
    #[test]
    fn timestamps_and_logins() {
        assert_eq!(fmt_ts(0), "1970-01-01 00:00:00");
        assert_eq!(fmt_ts(1_700_000_000), "2023-11-14 22:13:20");
        assert_eq!(fmt_ts(951_782_400), "2000-02-29 00:00:00");   // leap day
        assert_eq!(clean_worker("rig #1 (garage)!"), "rig1garage");
        assert_eq!(clean_worker(&"x".repeat(40)).len(), 24);
        assert_eq!(login("bc1qabc", ""), "bc1qabc"); assert_eq!(login("bc1qabc", "rig1"), "bc1qabc.rig1");
        assert_eq!(fmt_btc(0.0), "—"); assert_eq!(fmt_btc(0.00005), "5000 sats"); assert_eq!(fmt_btc(0.0123), "0.01230 BTC");
    }
    #[test]
    fn chirp_share_math() {
        let mk = |a: &str, w: f64, e: bool| ChirpMember { addr: a.into(), weight: w, eligible: e, ..Default::default() };
        let c = ChirpInfo { members: vec![mk("A", 75.0, true), mk("B", 25.0, true), mk("C", 999.0, false)], ..Default::default() };
        assert_eq!(c.sum_weight(), 100.0);                       // ineligible weight doesn't count
        assert_eq!(c.my_pct("B"), Some(25.0)); assert_eq!(c.my_pct("C"), None); assert_eq!(c.my_pct(""), None);
    }
}
