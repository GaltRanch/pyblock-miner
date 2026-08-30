// pyblockMiner — PyBLØCK LOTTO BLAKE2b miner (Bitcoin BLAKE2b, solo lottery). Rust + ratatui TUI.
// Tabbed app: MINE dashboard · DATA (session analytics) · STRATUMS (switch pools live) · LEARN · NETWORK · SETUP · HELP.
// `p` pauses/resumes mining from any tab (GPU/CPU idle, pool stays connected).
// Native SV1 stratum client + N-GPU/CPU BLAKE2b grinding via persistent gpu_grind daemons.
// You mine to YOUR address → keep 99.1% of every block · PyBLØCK pool fee 0.9%. Non-custodial.
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use serde::{Serialize, Deserialize};
use serde_json::{json, Value};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Sparkline, Wrap};

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

// ── persisted config ──
#[derive(Serialize, Deserialize)]
struct Config {
    #[serde(default = "default_stratums")] stratums: Vec<Stratum>,
    #[serde(default)] selected: usize,                 // index into stratums
    #[serde(default)] addrs: HashMap<String, String>,  // network -> address
    #[serde(default = "d_donate")] donate: f64,
    #[serde(default)] gpus: Option<u32>,
    #[serde(default)] cpu: bool,
}
fn d_donate() -> f64 { DONATE_MIN }
impl Default for Config {
    fn default() -> Self {
        Config { stratums: default_stratums(), selected: 0, addrs: HashMap::new(), donate: DONATE_MIN, gpus: None, cpu: false }
    }
}
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
struct Target { pool: String, addr: String, network: String, donate: f64 }

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
}
impl Stats {
    fn logline(&mut self, s: String) {
        self.log.push_back(s);
        while self.log.len() > 300 { self.log.pop_front(); }
    }
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

struct Daemon { _child: Child, stdin: ChildStdin, stdout: BufReader<ChildStdout>, name: String, weight: f64, dead: bool }
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
            Some(Daemon { _child: child, stdin, stdout, name, weight: 1.0, dead: false })
        }
        _ => { let _ = child.kill(); None }
    }
}
fn grind_all(ds: &mut [Daemon], cpu_threads: usize, cpu_rate: &mut f64,
             prevhash: &str, ntime: &str, work_root: &str, bits: u32, secs: f64) -> (Vec<String>, Vec<f64>, f64) {
    let space: u64 = 1u64 << 32;
    let gpu_caps: Vec<u64> = ds.iter().map(|d| if d.dead { 0 } else { ((d.weight * 1e9 * secs) as u64).max(1 << 22) }).collect();
    let cpu_cap: u64 = if cpu_threads > 0 { ((*cpu_rate * 1e9 * secs) as u64).max(2_000_000) } else { 0 };
    let total: u64 = gpu_caps.iter().sum::<u64>() + cpu_cap;
    let sweep: u64 = if total == 0 || total >= space { space } else { total };
    let mut cursor: u64 = 0;
    for (i, d) in ds.iter_mut().enumerate() {
        if d.dead || gpu_caps[i] == 0 { continue; }   // dead daemons get no job; their nonce share went to live workers via `total`
        let span = if total > 0 { (sweep as u128 * gpu_caps[i] as u128 / total as u128) as u64 } else { 0 };
        let _ = writeln!(d.stdin, "{} {} {} {} {} {}", prevhash, ntime, work_root, bits, cursor, span.max(1));
        let _ = d.stdin.flush();
        cursor += span;
    }
    let mut nonces: Vec<String> = vec![];
    let mut cpu_ghs = 0.0f64;
    if cpu_threads > 0 && cursor < sweep {
        let cpu_span = sweep - cursor;
        let t0 = Instant::now();
        let w = cpu_grind(prevhash, ntime, work_root, bits, cpu_threads, cursor, cpu_span);
        let dt = t0.elapsed().as_secs_f64();
        cpu_ghs = if dt > 0.0 { cpu_span as f64 / dt / 1e9 } else { 0.0 };
        if cpu_ghs > 0.0 { *cpu_rate = cpu_ghs; }
        nonces.extend(w);
    }
    let mut gpu_ghs = vec![0.0f64; ds.len()];
    for (i, d) in ds.iter_mut().enumerate() {
        if d.dead { continue; }   // no job was sent to a dead daemon
        loop {
            let mut line = String::new();
            match d.stdout.read_line(&mut line) {
                Ok(0) | Err(_) => { d.dead = true; d.weight = 0.0; break; }   // daemon died → mark dead so its nonce range redistributes next cycle
                Ok(_) => {
                    let t = line.trim();
                    if let Some(rest) = t.strip_prefix("END ") {
                        gpu_ghs[i] = rest.parse().unwrap_or(0.0);
                        if gpu_ghs[i] > 0.0 { d.weight = gpu_ghs[i]; }
                        break;
                    } else if !t.is_empty() { nonces.push(t.to_string()); }
                }
            }
        }
    }
    (nonces, gpu_ghs, cpu_ghs)
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

struct Conn {
    stream: TcpStream, buf: Vec<u8>, en1: Option<String>, en2size: usize, diff: f64,
    job: Option<Vec<Value>>, en2ctr: u64, pending: HashMap<u64, (String, bool, u64)>, subid: u64, addr: String, is_dev: bool, idle: Instant, last_notify: Instant,
}
impl Conn {
    fn connect(pool: &str, addr: &str, is_dev: bool) -> Option<Conn> {
        let mut stream = TcpStream::connect(pool).ok()?;
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
                if let Some((nonce, is_block, height)) = self.pending.remove(&idv) {
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
    // adaptive sweep length: track the observed block cadence (from the user's prevhash changes) so sweeps
    // stay short on fast chains (switch to new work sooner → far fewer stale shares + less wasted hashrate)
    // but keep the efficient default on normal-speed chains.
    let mut last_prevhash = String::new();
    let mut last_block_at = Instant::now();
    let mut block_interval_ema = 30.0f64;   // conservative start → sweep capped at MAX until real cadence is seen

    loop {
        // read current live target (pool/addr/donate change when the user switches stratum)
        let (pool, addr, donate, network) = { let t = tgt.lock().unwrap(); (t.pool.clone(), t.addr.clone(), t.donate, t.network.clone()) };
        if addr.is_empty() {
            { let mut st = stats.lock().unwrap(); st.connected = false;
              st.logline("no address set — go to SETUP [5] to set/generate one".into()); }
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        let mut user = match Conn::connect(&pool, &addr, false) {
            Some(c) => c,
            None => {
                { let mut st = stats.lock().unwrap(); st.connected = false; st.logline(format!("connection failed to {} — retrying in 3s…", pool)); }
                std::thread::sleep(Duration::from_secs(3));
                continue;
            }
        };
        let mut dev: Option<Conn> = if donate > 0.0 { Conn::connect(DONATE_POOL, DEV_DONATION_ADDR, true) } else { None };
        { let mut st = stats.lock().unwrap(); st.connected = true; st.started.get_or_insert(Instant::now());
          st.logline(format!("connected to {}", pool));
          if donate > 0.0 { st.logline(format!("hashrate donation {:.1}% → PyBLØCK", donate)); } }

        loop {
            // live switch: if the shared target's pool/addr changed, drop + reconnect
            { let t = tgt.lock().unwrap();
              if t.pool != pool || t.addr != addr || t.network != network || t.donate != donate {
                stats.lock().unwrap().logline(format!("switching stratum → {}", t.pool)); break;
              } }
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
            let (nonces, gpu_ghs, cpu_ghs) = grind_all(&mut daemons, cpu_threads, &mut cpu_rate, &prevhash, &ntime, &work_root, bits, sweep_secs);
            {
                let mut st = stats.lock().unwrap();
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
                let is_block = match nonce_hash(&prevhash, &ntime, &work_root, &nh) {
                    Some(h) => { let d = hash_diff(&h); if d > sweep_best { sweep_best = d; } nbits != 0 && hash_le_target(&h, &net_target) }
                    None => false,
                };
                if !still_current { continue; }   // stale sweep (a block landed mid-grind) — don't submit worthless shares
                conn.subid += 1; let sid = conn.subid;
                conn.pending.insert(sid, (nh.clone(), is_block, job_height));
                send(&mut conn.stream, &json!({"id":sid,"method":"mining.submit","params":[conn.addr, job_id, en2hex, ntime, nh, version]}));
            }
            if sweep_best > 0.0 && !is_dev { let mut st = stats.lock().unwrap(); if sweep_best > st.best_diff { st.best_diff = sweep_best; } }
        }
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

// ═══════════════════════ TABS / UI ═══════════════════════
#[derive(Clone, Copy, PartialEq)]
enum Tab { Mine, Data, Stratums, Learn, Network, Setup, Help }
const TABS: [(Tab, &str); 7] = [
    (Tab::Mine, "MINE"), (Tab::Data, "DATA"), (Tab::Stratums, "STRATUMS"), (Tab::Learn, "LEARN"),
    (Tab::Network, "NETWORK"), (Tab::Setup, "SETUP"), (Tab::Help, "HELP"),
];

enum Input { AddStratum, EditAddr }
struct App {
    tab: Tab,
    cfg: Config,
    strat_cur: usize,     // cursor in the stratums list
    learn_page: usize,
    input: Option<Input>,
    buf: String,
    msg: String,          // transient status line
    paused: Arc<AtomicBool>,   // shared with the engine: `p` toggles pause/resume of mining
}
impl App {
    fn network(&self) -> String { self.cfg.stratums.get(self.cfg.selected).map(|s| s.network.clone()).unwrap_or_else(|| "mainnet".into()) }
    fn addr(&self) -> String { self.cfg.addrs.get(&self.network()).cloned().unwrap_or_default() }
}

fn tile(title: &str, value: Line<'static>, sub: &str, border: Color) -> Paragraph<'static> {
    Paragraph::new(Text::from(vec![
        Line::from(Span::styled(title.to_string(), Style::new().fg(MUT))), Line::from(""),
        value, Line::from(Span::styled(sub.to_string(), Style::new().fg(MUT))),
    ])).alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(border)))
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
        Tab::Mine => render_mine(f, body, st),
        Tab::Data => render_data(f, body, st),
        Tab::Stratums => render_stratums(f, body, app),
        Tab::Learn => render_learn(f, body, app),
        Tab::Network => render_network(f, body, st, app),
        Tab::Setup => render_setup(f, body, app),
        Tab::Help => render_help(f, body),
    }
    // footer
    let foot = if let Some(k) = &app.input {
        let label = match k { Input::AddStratum => "new stratum (name,host:port,network)", Input::EditAddr => "address" };
        Line::from(vec![Span::styled(format!(" {} > ", label), Style::new().fg(Color::Black).bg(YLW)),
                        Span::styled(format!("{}_", app.buf), Style::new().fg(YLW)),
                        Span::styled("   Enter=ok  Esc=cancel", Style::new().fg(MUT))])
    } else if !app.msg.is_empty() {
        Line::from(Span::styled(app.msg.clone(), Style::new().fg(AMB)))
    } else {
        Line::from(vec![Span::styled(" 1-7 ", Style::new().fg(Color::Black).bg(GRN)),
                        Span::styled(" tabs · ", Style::new().fg(MUT)),
                        Span::styled("Tab", Style::new().fg(GRN)), Span::styled(" next · ", Style::new().fg(MUT)),
                        Span::styled("p", Style::new().fg(GRN)),
                        Span::styled(if st.paused { " resume · " } else { " pause · " }, Style::new().fg(MUT)),
                        Span::styled("q", Style::new().fg(GRN)), Span::styled(" quit", Style::new().fg(MUT))])
    };
    f.render_widget(Paragraph::new(foot).wrap(Wrap { trim: true }), outer[2]);
    // version — bottom-right corner
    f.render_widget(Paragraph::new(Line::from(Span::styled(format!("pyblockMiner v{} ", VERSION), Style::new().fg(MUT))))
        .alignment(ratatui::layout::Alignment::Right), outer[2]);
}

fn render_mine(f: &mut Frame, area: Rect, st: &Stats) {
    let gpu_h = (st.gpu_names.len().max(1) + 2).min(9) as u16;
    let c = Layout::vertical([Constraint::Length(4), Constraint::Length(6), Constraint::Length(6),
        Constraint::Length(gpu_h), Constraint::Length(5), Constraint::Min(3)]).split(area);
    let dot = if st.paused { Span::styled("⏸ PAUSED", Style::new().fg(YLW).add_modifier(Modifier::BOLD)) }
              else if st.blake2b_active == Some(false) { Span::styled("⏳ WAITING · SHA-256d", Style::new().fg(AMB).add_modifier(Modifier::BOLD)) }
              else if st.connected { Span::styled("● LIVE", Style::new().fg(GRN)) }
              else { Span::styled("● OFFLINE", Style::new().fg(Color::Red)) };
    let (nt, nco) = match st.network.as_str() { "mainnet" => (" MAINNET ", GRN), "testnet4" => (" TESTNET4 ", YLW), _ => (" REGTEST ", AMB) };
    let bal = if st.balance_ok { format!("balance {:.8} BTC", st.balance_btc) } else { "balance —".to_string() };
    let mut l2 = vec![Span::styled("your address  ", Style::new().fg(MUT)), Span::styled(st.addr.clone(), Style::new().fg(CYN)),
        Span::styled(format!("   {}", bal), Style::new().fg(GRN)), Span::styled("   keep 99.1% · ", Style::new().fg(MUT)),
        Span::styled("pool fee 0.9%", Style::new().fg(PNK))];
    if st.donate > 0.0 { l2.push(Span::styled(format!(" · donation {:.1}% → PyBLØCK", st.donate), Style::new().fg(AMB))); }
    let mut l1 = vec![Span::styled("LOTTO BLAKE2b", Style::new().fg(YLW).add_modifier(Modifier::BOLD)), Span::raw("  "),
        Span::styled(nt, Style::new().fg(Color::Black).bg(nco).add_modifier(Modifier::BOLD)), Span::raw("  "), dot,
        Span::styled(format!("   {}", st.endpoint), Style::new().fg(MUT))];
    if st.blake2b_active == Some(false) {
        l1.push(Span::styled(format!("   ⛔ BLAKE2b @ {} ({} to go) — not mining, saving power", st.activation_height, st.blocks_until_act), Style::new().fg(AMB)));
    }
    if st.update_available {
        l1.push(Span::styled(format!("   ⬆ v{} — git pull && ./build.sh", st.latest_version), Style::new().fg(PNK).add_modifier(Modifier::BOLD)));
    }
    f.render_widget(Paragraph::new(Text::from(vec![
        Line::from(l1),
        Line::from(l2)])).block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(GRN))
        .title(Span::styled(" ⛏ Bitcoin BLAKE2b · solo lottery ", Style::new().fg(GRN)))), c[0]);
    let row = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(c[1]);
    f.render_widget(tile("YOUR HASHRATE", Line::from(vec![Span::styled(format!("{:.1}", st.hr_total), Style::new().fg(GRN).add_modifier(Modifier::BOLD)),
        Span::styled(" GH/s", Style::new().fg(MUT))]), &format!("{} worker(s)", st.gpu_ghs.len()), BRD), row[0]);
    f.render_widget(tile("BLOCKS FOUND", Line::from(Span::styled(format!("{}", st.blocks), Style::new().fg(GRN).add_modifier(Modifier::BOLD))),
        &format!("{} shares acc · {} rej · {} don", st.accepted, st.rejected, st.donated), BRD), row[1]);
    f.render_widget(tile("DIFFICULTY", Line::from(Span::styled(format!("bits {}", st.bits), Style::new().fg(YLW).add_modifier(Modifier::BOLD))),
        &format!("diff {:.0} · best {}", st.diff, fmt_diff(st.best_diff)), BRD), row[2]);
    let (nm, ng, nh) = if st.net_ok { (format!("{}", st.net_miners), format!("{:.1}", st.net_ghs), format!("{}", st.net_height)) }
        else { ("—".into(), "—".into(), "—".into()) };
    let nr = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(c[2]);
    f.render_widget(tile("◈ MINERS ONLINE", Line::from(Span::styled(nm, Style::new().fg(CYN).add_modifier(Modifier::BOLD))), "using pyblockMiner", CYN), nr[0]);
    f.render_widget(tile("◈ NETWORK HASHRATE", Line::from(vec![Span::styled(ng, Style::new().fg(CYN).add_modifier(Modifier::BOLD)),
        Span::styled(if st.net_ok { " GH/s" } else { "" }, Style::new().fg(MUT))]), "all pyblockMiner users", CYN), nr[1]);
    f.render_widget(tile("◈ POOL HEIGHT", Line::from(Span::styled(nh, Style::new().fg(CYN).add_modifier(Modifier::BOLD))), "PyBLØCK LOTTO network", CYN), nr[2]);
    let mut glines: Vec<Line> = vec![];
    for (i, g) in st.gpu_ghs.iter().enumerate() {
        let name = st.gpu_names.get(i).cloned().unwrap_or_else(|| format!("GPU {}", i));
        glines.push(Line::from(vec![Span::styled(format!(" {:>2}  ", i), Style::new().fg(MUT)),
            Span::styled(format!("{:<26}", name), Style::new().fg(CYN)), Span::styled(format!("{:>7.2} GH/s", g), Style::new().fg(GRN))]));
    }
    if glines.is_empty() { glines.push(Line::from(Span::styled(" warming up…", Style::new().fg(MUT)))); }
    f.render_widget(Paragraph::new(Text::from(glines)).block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(BRD)).title(Span::styled(" WORKERS ", Style::new().fg(MUT)))), c[3]);
    // ratatui's Sparkline renders the FIRST N=min(width,len) points, so feed it exactly the last inner_w
    // samples → the chart fills the full terminal width and updates live (adapts to any window size).
    let inner_w = (c[4].width.saturating_sub(2) as usize).max(1);   // block borders eat 2 columns
    let data: Vec<u64> = st.hr_hist.iter().rev().take(inner_w).rev().cloned().collect();
    f.render_widget(Sparkline::default().block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(BRD)).title(Span::styled(" hashrate ", Style::new().fg(MUT)))).data(&data).style(Style::new().fg(GRN)), c[4]);
    let items: Vec<ListItem> = st.log.iter().rev().take(c[5].height.saturating_sub(2) as usize).rev().map(|l| {
        let col = if l.contains("BLOCK FOUND") { GRN } else if l.contains("donation") { AMB } else if l.contains("rejected") || l.contains("stale") || l.contains("switching") { AMB } else { MUT };
        ListItem::new(Line::from(Span::styled(l.clone(), Style::new().fg(col))))
    }).collect();
    f.render_widget(List::new(items).block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(BRD)).title(Span::styled(" log ", Style::new().fg(MUT)))), c[5]);
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
    let c = Layout::vertical([Constraint::Length(6), Constraint::Length(gpu_rows(st)), Constraint::Min(3)]).split(area);
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
    f.render_widget(Paragraph::new(Text::from(wl)).block(Block::default().borders(Borders::ALL)
        .border_style(Style::new().fg(BRD)).title(Span::styled(" WORKERS — hashrate share ", Style::new().fg(MUT)))), c[1]);
    // session analytics text
    let up = st.started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
    let total = st.accepted + st.rejected;
    let rej_rate = if total > 0 { st.rejected as f64 / total as f64 * 100.0 } else { 0.0 };
    let eta = eta_to_block(st.net_nbits, st.hr_total);
    let per_day = if eta.is_finite() && eta > 0.0 { 86_400.0 / eta } else { 0.0 };
    let kv = |k: &str, v: String, col: Color| Line::from(vec![
        Span::styled(format!("  {:<22}", k), Style::new().fg(MUT)), Span::styled(v, Style::new().fg(col))]);
    let lines = vec![
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
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true }).block(Block::default().borders(Borders::ALL)
        .border_style(Style::new().fg(CYN)).title(Span::styled(" ANÁLISIS — session data (press p to pause mining while you review) ", Style::new().fg(CYN)))), c[2]);
}

fn render_stratums(f: &mut Frame, area: Rect, app: &App) {
    let mut items: Vec<ListItem> = vec![];
    for (i, s) in app.cfg.stratums.iter().enumerate() {
        let cur = i == app.strat_cur;
        let active = i == app.cfg.selected;
        let mark = if active { "▶ " } else { "  " };
        let tag = if s.custom { "custom" } else { "default" };
        let line = Line::from(vec![
            Span::styled(mark, Style::new().fg(GRN)),
            Span::styled(format!("{:<22}", s.name), Style::new().fg(if active { GRN } else { CYN }).add_modifier(if cur { Modifier::REVERSED } else { Modifier::empty() })),
            Span::styled(format!("{:<28}", s.url), Style::new().fg(MUT)),
            Span::styled(format!("[{}] {}", s.network, tag), Style::new().fg(AMB)),
        ]);
        items.push(ListItem::new(line));
    }
    let help = Line::from(vec![Span::styled("  ↑↓ move · ", Style::new().fg(MUT)), Span::styled("Enter", Style::new().fg(GRN)),
        Span::styled(" switch LIVE · ", Style::new().fg(MUT)), Span::styled("a", Style::new().fg(GRN)), Span::styled(" add custom · ", Style::new().fg(MUT)),
        Span::styled("d", Style::new().fg(GRN)), Span::styled(" delete custom", Style::new().fg(MUT))]);
    let c = Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).split(area);
    f.render_widget(List::new(items).block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(GRN)).title(Span::styled(" STRATUMS — switch pool without leaving the app ", Style::new().fg(GRN)))), c[0]);
    f.render_widget(Paragraph::new(help).block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(BRD))), c[1]);
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
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(CYN)).title(Span::styled(title, Style::new().fg(CYN)))), area);
}

fn render_network(f: &mut Frame, area: Rect, st: &Stats, app: &App) {
    let c = Layout::vertical([Constraint::Length(6), Constraint::Min(3)]).split(area);
    let (nm, ng, nh) = if st.net_ok { (format!("{}", st.net_miners), format!("{:.1}", st.net_ghs), format!("{}", st.net_height)) } else { ("—".into(), "—".into(), "—".into()) };
    let r = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(c[0]);
    f.render_widget(tile("◈ MINERS ONLINE", Line::from(Span::styled(nm, Style::new().fg(CYN).add_modifier(Modifier::BOLD))), "using pyblockMiner", CYN), r[0]);
    f.render_widget(tile("◈ NETWORK HASHRATE", Line::from(vec![Span::styled(ng, Style::new().fg(CYN).add_modifier(Modifier::BOLD)), Span::styled(if st.net_ok { " GH/s" } else { "" }, Style::new().fg(MUT))]), "all pyblockMiner users", CYN), r[1]);
    f.render_widget(tile("◈ POOL HEIGHT", Line::from(Span::styled(nh, Style::new().fg(CYN).add_modifier(Modifier::BOLD))), "PyBLØCK LOTTO network", CYN), r[2]);
    let bal = if st.balance_ok { format!("{:.8} BTC", st.balance_btc) } else { "—".into() };
    let lines = vec![
        Line::from(vec![Span::styled("  network      ", Style::new().fg(MUT)), Span::styled(app.network(), Style::new().fg(GRN))]),
        Line::from(vec![Span::styled("  your address ", Style::new().fg(MUT)), Span::styled(app.addr(), Style::new().fg(CYN))]),
        Line::from(vec![Span::styled("  your balance ", Style::new().fg(MUT)), Span::styled(bal, Style::new().fg(GRN))]),
        Line::from(vec![Span::styled("  your blocks  ", Style::new().fg(MUT)), Span::styled(format!("{} blocks · {} shares acc · {} rej", st.blocks, st.accepted, st.rejected), Style::new().fg(GRN))]),
        Line::from(""),
        Line::from(Span::styled("  Network stats + balance come from the PyBLØCK node/pool for the selected chain (mainnet balance via public explorer).", Style::new().fg(MUT))),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)).block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(BRD)).title(Span::styled(" NETWORK & YOU ", Style::new().fg(MUT)))), c[1]);
}

fn render_setup(f: &mut Frame, area: Rect, app: &App) {
    let net = app.network();
    let addr = app.addr();
    let addr_disp = if addr.is_empty() { "— not set —".to_string() } else { addr };
    let donate_disp = if net_cfg(&net).donate { format!("{:.1}% (mainnet)", app.cfg.donate) } else { "off (testnet/regtest)".into() };
    let gpus_disp = app.cfg.gpus.map(|n| n.to_string()).unwrap_or_else(|| "auto".into());
    let lines = vec![
        Line::from(vec![Span::styled("  selected stratum  ", Style::new().fg(MUT)), Span::styled(app.cfg.stratums.get(app.cfg.selected).map(|s| s.name.clone()).unwrap_or_default(), Style::new().fg(GRN)),
            Span::styled(format!("  [{}]", net), Style::new().fg(AMB))]),
        Line::from(vec![Span::styled("  your address      ", Style::new().fg(MUT)), Span::styled(addr_disp, Style::new().fg(CYN))]),
        Line::from(vec![Span::styled("  donation          ", Style::new().fg(MUT)), Span::styled(donate_disp, Style::new().fg(AMB))]),
        Line::from(vec![Span::styled("  gpus              ", Style::new().fg(MUT)), Span::styled(gpus_disp, Style::new().fg(GRN)),
            Span::styled(format!("   cpu: {}", if app.cfg.cpu { "on" } else { "off" }), Style::new().fg(GRN))]),
        Line::from(""),
        Line::from(vec![Span::styled("  g", Style::new().fg(GRN)), Span::styled(" generate a new address for this network   ", Style::new().fg(MUT)),
            Span::styled("e", Style::new().fg(GRN)), Span::styled(" edit/paste an address", Style::new().fg(MUT))]),
        Line::from(vec![Span::styled("  c", Style::new().fg(GRN)), Span::styled(" toggle CPU   ", Style::new().fg(MUT)),
            Span::styled("+/-", Style::new().fg(GRN)), Span::styled(" donation (mainnet)   ", Style::new().fg(MUT)),
            Span::styled("changes auto-save + apply live", Style::new().fg(MUT))]),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(GRN)).title(Span::styled(" SETUP — address, network, config (saved) ", Style::new().fg(GRN)))), area);
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
        l("SETUP", "g generate address · e edit address · c toggle CPU · +/- donation"),
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
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(CYN)).title(Span::styled(" HELP ", Style::new().fg(CYN)))), area);
}

// apply the current selected stratum + address to the live engine target + stats display
fn apply_target(app: &App, tgt: &Arc<Mutex<Target>>, stats: &Arc<Mutex<Stats>>) {
    let s = match app.cfg.stratums.get(app.cfg.selected) { Some(s) => s.clone(), None => return };
    let addr = app.addr();
    let donate = if net_cfg(&s.network).donate { app.cfg.donate.max(DONATE_MIN) } else { 0.0 };
    { let mut t = tgt.lock().unwrap(); t.pool = s.url.clone(); t.addr = addr.clone(); t.network = s.network.clone(); t.donate = donate; }
    { let mut st = stats.lock().unwrap(); st.endpoint = s.url.clone(); st.addr = addr; st.network = s.network.clone(); st.donate = donate; st.balance_ok = false; st.net_ok = false; }
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
                KeyCode::Enter => { app.cfg.selected = app.strat_cur; save_config(&app.cfg); apply_target(app, tgt, stats);
                    app.msg = format!("switched to {}", app.cfg.stratums[app.strat_cur].name); }
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
            "--pool" => { i += 1; if i < args.len() { if let Some(s) = cfg.stratums.get_mut(cfg.selected) { s.url = args[i].clone(); } } }
            "--gpus" => { i += 1; if i < args.len() { cfg.gpus = args[i].parse().ok(); } }
            "--cpu" => { cfg.cpu = true; }
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
    let mut app = App { tab: Tab::Mine, cfg, strat_cur: 0, learn_page: 0, input: None, buf: String::new(), msg: String::new(), paused: paused.clone() };
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
        addr: app.addr(), network: net0.clone(), donate: donate0,
    }));
    { let mut st = stats.lock().unwrap(); st.endpoint = tgt.lock().unwrap().pool.clone(); st.addr = app.addr(); st.network = net0; st.donate = donate0; }

    { let stats = stats.clone(); let tgt = tgt.clone(); let paused = paused.clone(); std::thread::spawn(move || engine(stats, tgt, ngpu, cpu_threads, paused)); }
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
        { let st = stats.lock().unwrap(); let _ = terminal.draw(|f| ui(f, &app, &st)); }
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
