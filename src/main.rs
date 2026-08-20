// pyblockMiner — PyBLØCK LOTTO BLAKE2b miner (Bitcoin BLAKE2b, solo lottery). Rust + ratatui TUI.
// Native SV1 stratum client + work construction + N-GPU grinding via persistent `gpu_grind` daemons
// (one per GPU, OpenCL kernel compiled once → all GPUs saturated, no per-sweep overhead).
// You mine to YOUR mainnet address → keep 99.1% of every block · PyBLØCK pool fee 0.9%. Non-custodial.
// A small dev donation (default 0.4%, like xmrig) rewards the miner's author — see DEV_DONATION_ADDR.
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde_json::{json, Value};

use crossterm::event::{self, Event, KeyCode};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Sparkline, Wrap};

// ── developer donation (like xmrig's donate-level) ──────────────────────────────────────────────
// Hardcoded, consensus of the project: a small fraction of your hashing mines to the developer's
// address as a thank-you for the miner. This is SEPARATE from the pool fee (0.9%). Minimum 0.4%,
// raise it with --donate <pct>. Mechanism: a 2nd stratum connection is authorized to this address;
// ~donate% of sweeps mine to it, so ~donate% of any blocks you find pay the developer instead.
const DEV_DONATION_ADDR: &str = "1PyBLoCKdiaC46vD9CWcmxa3ey2VzSc5Q2";
const DONATE_MIN: f64 = 0.4; // percent — floor, cannot go lower

// PyBLØCK palette
const GRN: Color = Color::Rgb(0, 255, 65);
const YLW: Color = Color::Rgb(255, 255, 0);
const CYN: Color = Color::Rgb(0, 255, 255);
const MUT: Color = Color::Rgb(130, 154, 130);
const AMB: Color = Color::Rgb(224, 176, 53);
const PNK: Color = Color::Rgb(255, 92, 200);
const BRD: Color = Color::Rgb(35, 60, 35);

// ── address validation: mainnet only (regtest/testnet rewards are unspendable on mainnet) ──
// Returns "mainnet" (ok to mine), "test" (regtest/testnet → refuse), or "unknown" (unrecognized).
fn addr_kind(a: &str) -> &'static str {
    if a.starts_with("bc1") || a.starts_with('1') || a.starts_with('3') { return "mainnet"; }
    if a.starts_with("bcrt1") || a.starts_with("tb1") || a.starts_with("tpub")
        || a.starts_with('m') || a.starts_with('n') || a.starts_with('2') { return "test"; }
    "unknown"
}

// ── locate gpu_grind + blake2b.cl (env override → next to the binary → repo layout → PATH) ──
fn gpu_dir() -> String {
    if let Ok(d) = std::env::var("PYBLOCK_GPU_DIR") {
        return d;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            for cand in [p.join("gpu"), p.join("../../gpu"), p.to_path_buf()] {
                if cand.join("blake2b.cl").exists() {
                    return cand.to_string_lossy().into_owned();
                }
            }
        }
    }
    if Path::new("gpu/blake2b.cl").exists() { return "gpu".into(); }
    ".".into()
}
fn gpu_bin() -> String {
    if let Ok(b) = std::env::var("PYBLOCK_GPU_BIN") { return b; }
    let cand = format!("{}/gpu_grind", gpu_dir());
    if Path::new(&cand).exists() { cand } else { "gpu_grind".into() }
}
fn gpu_names() -> Vec<String> {
    match Command::new("nvidia-smi").args(["--query-gpu=name", "--format=csv,noheader"]).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect(),
        Err(_) => vec![],
    }
}

#[derive(Default)]
struct Stats {
    connected: bool,
    endpoint: String,
    addr: String,
    donate: f64,
    donated: u64,      // blocks found during donation sweeps (paid to the developer)
    diff: f64,
    bits: u32,
    gpu_ghs: Vec<f64>,
    gpu_names: Vec<String>,
    blocks: u64,
    rejected: u64,
    hr_total: f64,
    hr_hist: VecDeque<u64>,
    log: VecDeque<String>,
    started: Option<Instant>,
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
// target = (2^224-1) >> bits as a 32-byte big-endian array (same as gpu_grind's build_target)
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
// CPU BLAKE2b grinder (native Rust, multi-threaded). Sweeps [nstart, nstart+span), returns winning nonces.
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

// ── persistent gpu_grind daemon (one per GPU) ──
struct Daemon {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    name: String,
    weight: f64,   // relative speed (GH/s of last grind) → proportional nonce-space split so all GPUs finish together
}
fn spawn_daemon(dev: u32, name: String) -> Option<Daemon> {
    let mut child = Command::new(gpu_bin())
        .args(["daemon", &dev.to_string()])
        .current_dir(gpu_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdin = child.stdin.take()?;
    let stdout = BufReader::new(child.stdout.take()?);
    Some(Daemon { _child: child, stdin, stdout, name, weight: 1.0 })
}
// Grind one job across ALL workers: GPU daemons (via stdin) + optional CPU threads, splitting the nonce
// space proportionally to each worker's measured speed (so all finish together). GPU-only → sweeps 2^32;
// CPU-only → sweeps only what the CPU can do per call (rotating extranonce next call).
// Returns (winners, per-GPU GH/s, CPU GH/s).
fn grind_all(ds: &mut [Daemon], cpu_threads: usize, cpu_rate: &mut f64,
             prevhash: &str, ntime: &str, work_root: &str, bits: u32) -> (Vec<String>, Vec<f64>, f64) {
    let secs = 0.35f64;
    let space: u64 = 1u64 << 32;
    let gpu_caps: Vec<u64> = ds.iter().map(|d| ((d.weight * 1e9 * secs) as u64).max(1 << 22)).collect();
    let cpu_cap: u64 = if cpu_threads > 0 { ((*cpu_rate * 1e9 * secs) as u64).max(2_000_000) } else { 0 };
    let total: u64 = gpu_caps.iter().sum::<u64>() + cpu_cap;
    let sweep: u64 = if total == 0 || total >= space { space } else { total };

    // dispatch GPU jobs (they grind in the background while the CPU grinds)
    let mut cursor: u64 = 0;
    for (i, d) in ds.iter_mut().enumerate() {
        let span = if total > 0 { (sweep as u128 * gpu_caps[i] as u128 / total as u128) as u64 } else { 0 };
        let _ = writeln!(d.stdin, "{} {} {} {} {} {}", prevhash, ntime, work_root, bits, cursor, span.max(1));
        let _ = d.stdin.flush();
        cursor += span;
    }
    // CPU grinds the remaining range in parallel with the GPUs
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
    // collect GPU winners
    let mut gpu_ghs = vec![0.0f64; ds.len()];
    for (i, d) in ds.iter_mut().enumerate() {
        loop {
            let mut line = String::new();
            match d.stdout.read_line(&mut line) {
                Ok(0) | Err(_) => break, // daemon died
                Ok(_) => {
                    let t = line.trim();
                    if let Some(rest) = t.strip_prefix("END ") {
                        gpu_ghs[i] = rest.parse().unwrap_or(0.0);
                        if gpu_ghs[i] > 0.0 { d.weight = gpu_ghs[i]; }
                        break;
                    } else if !t.is_empty() {
                        nonces.push(t.to_string());
                    }
                }
            }
        }
    }
    (nonces, gpu_ghs, cpu_ghs)
}

fn send(stream: &mut TcpStream, v: &Value) {
    let mut s = serde_json::to_string(v).unwrap();
    s.push('\n');
    let _ = stream.write_all(s.as_bytes());
}

// ── one stratum connection (the "you" session and the "dev donation" session are both Conn) ──
struct Conn {
    stream: TcpStream,
    buf: Vec<u8>,
    en1: Option<String>,
    en2size: usize,
    diff: f64,
    job: Option<Vec<Value>>,
    en2ctr: u64,
    pending: HashMap<u64, String>,
    subid: u64,
    addr: String,
    is_dev: bool,
    idle: Instant,
}
impl Conn {
    fn connect(pool: &str, addr: &str, is_dev: bool) -> Option<Conn> {
        let mut stream = TcpStream::connect(pool).ok()?;
        stream.set_nonblocking(true).ok();
        let sub_ua = if is_dev { "PyBLOCK-GPU/BLAKE2b-donate" } else { "PyBLOCK-GPU/BLAKE2b" };
        send(&mut stream, &json!({"id":1,"method":"mining.subscribe","params":[sub_ua]}));
        send(&mut stream, &json!({"id":2,"method":"mining.authorize","params":[addr,"x"]}));
        Some(Conn {
            stream, buf: Vec::new(), en1: None, en2size: 8, diff: 1.0, job: None,
            en2ctr: 0, pending: HashMap::new(), subid: 100, addr: addr.to_string(), is_dev,
            idle: Instant::now(),
        })
    }
    // pump the socket: parse messages, update job/diff/subscribe/submit-results. Returns false if dead.
    fn pump(&mut self, stats: &Arc<Mutex<Stats>>) -> bool {
        let mut tmp = [0u8; 8192];
        match self.stream.read(&mut tmp) {
            Ok(0) => return false,                 // peer closed
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
            } else if let Some(idv) = id {
                if let Some(nonce) = self.pending.remove(&idv) {
                    let mut st = stats.lock().unwrap();
                    if m.get("result") == Some(&Value::Bool(true)) {
                        if self.is_dev {
                            st.donated += 1;
                            let n = st.donated;
                            st.logline(format!("💚 donation block (#{}) → developer · thank you! · nonce {}", n, nonce));
                        } else {
                            st.blocks += 1;
                            let n = st.blocks;
                            st.logline(format!("🎉 BLOCK FOUND (#{})  paid to your address · nonce {}", n, nonce));
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
    // job ready for grinding? returns (extranonce1, job array) when subscribed + a notify arrived.
    fn ready(&self) -> Option<(String, Vec<Value>)> {
        match (&self.en1, &self.job) {
            (Some(a), Some(b)) if b.len() >= 8 => Some((a.clone(), b.clone())),
            _ => None,
        }
    }
}

// build the BLAKE2b work_root for a job/identity and return (prevhash, ntime, version, job_id, en2hex, work_root_hex)
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

fn engine(stats: Arc<Mutex<Stats>>, pool: String, addr: String, ngpu: u32, cpu_threads: usize, donate: f64) {
    // spawn one persistent daemon per GPU (kernel compiled once → all GPUs stay saturated)
    let names = gpu_names();
    let mut daemons: Vec<Daemon> = Vec::new();
    for dev in 0..ngpu {
        let nm = names.get(dev as usize).cloned().unwrap_or_else(|| format!("GPU {}", dev));
        if let Some(d) = spawn_daemon(dev, nm) { daemons.push(d); }
    }
    let mut cpu_rate = 0.05f64;
    { let mut st = stats.lock().unwrap();
      st.gpu_names = daemons.iter().map(|d| d.name.clone()).collect();
      if cpu_threads > 0 { st.gpu_names.push(format!("CPU ({} threads)", cpu_threads)); }
      st.gpu_ghs = vec![0.0; st.gpu_names.len()];
      let nworkers = st.gpu_names.len();
      let names_str = st.gpu_names.join(", ");
      st.logline(format!("{} worker(s) ready: {}", nworkers, names_str));
      st.logline(format!("dev donation {:.1}% → {}", donate, DEV_DONATION_ADDR)); }

    let mut donate_credit = 0.0f64;              // accumulates `donate/100` per sweep; ≥1 → this sweep pays the dev
    let mut dev_retry = Instant::now();

    loop {
        let mut user = match Conn::connect(&pool, &addr, false) {
            Some(c) => c,
            None => {
                { let mut st = stats.lock().unwrap(); st.connected = false; st.logline("connection failed — retrying in 3s…".into()); }
                std::thread::sleep(Duration::from_secs(3));
                continue;
            }
        };
        // best-effort donation session (never blocks mining if it fails)
        let mut dev: Option<Conn> = Conn::connect(&pool, DEV_DONATION_ADDR, true);
        { let mut st = stats.lock().unwrap(); st.connected = true; st.started.get_or_insert(Instant::now());
          st.logline(format!("connected to PyBLØCK LOTTO BLAKE2b {}", pool)); }

        loop {
            if !user.pump(&stats) {
                { let mut st = stats.lock().unwrap(); st.connected = false; st.logline("disconnected — reconnecting…".into()); }
                break;
            }
            if let Some(d) = dev.as_mut() { if !d.pump(&stats) { dev = None; dev_retry = Instant::now(); } }
            if dev.is_none() && donate > 0.0 && dev_retry.elapsed() > Duration::from_secs(20) {
                dev = Conn::connect(&pool, DEV_DONATION_ADDR, true);
                dev_retry = Instant::now();
            }
            if user.idle.elapsed() > Duration::from_secs(90) {
                { let mut st = stats.lock().unwrap(); st.connected = false; st.logline("no data for 90s — reconnecting…".into()); }
                break;
            }

            // pick this sweep's identity: dev if its donation quota is due AND its job is ready, else you
            donate_credit += donate / 100.0;
            let dev_ready = dev.as_ref().and_then(|d| d.ready()).is_some();
            let do_donate = donate_credit >= 1.0 && dev_ready;

            let (en1v, jobv, is_dev) = if do_donate {
                let d = dev.as_ref().unwrap();
                let (a, b) = d.ready().unwrap();
                (a, b, true)
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
            let (nonces, gpu_ghs, cpu_ghs) = grind_all(&mut daemons, cpu_threads, &mut cpu_rate, &prevhash, &ntime, &work_root, bits);
            {
                let mut st = stats.lock().unwrap();
                let mut all = gpu_ghs.clone();
                if cpu_threads > 0 { all.push(cpu_ghs); }
                st.gpu_ghs = all;
                st.hr_total = st.gpu_ghs.iter().sum();
                let hv = (st.hr_total * 100.0) as u64;
                st.hr_hist.push_back(hv);
                while st.hr_hist.len() > 160 { st.hr_hist.pop_front(); }
            }
            // pump again — a new block during the grind makes our nonces stale (don't submit → no reject spam)
            user.pump(&stats);
            if let Some(d) = dev.as_mut() { d.pump(&stats); }
            let cur_prev = if is_dev {
                dev.as_ref().and_then(|d| d.job.as_ref()).and_then(|j| j.get(1)).and_then(|v| v.as_str()).unwrap_or("").to_string()
            } else {
                user.job.as_ref().and_then(|j| j.get(1)).and_then(|v| v.as_str()).unwrap_or("").to_string()
            };
            if cur_prev == prevhash {
                let conn = if is_dev { dev.as_mut().unwrap() } else { &mut user };
                for nh in nonces {
                    conn.subid += 1;
                    let sid = conn.subid;
                    conn.pending.insert(sid, nh.clone());
                    send(&mut conn.stream, &json!({"id":sid,"method":"mining.submit","params":[conn.addr, job_id, en2hex, ntime, nh, version]}));
                }
            } else if !nonces.is_empty() {
                let mut st = stats.lock().unwrap();
                st.logline(format!("· dropped {} stale (tip moved during grind)", nonces.len()));
            }
        }
    }
}

fn tile(title: &str, value: Line<'static>, sub: &str) -> Paragraph<'static> {
    let text = Text::from(vec![
        Line::from(Span::styled(title.to_string(), Style::new().fg(MUT))),
        Line::from(""),
        value,
        Line::from(Span::styled(sub.to_string(), Style::new().fg(MUT))),
    ]);
    Paragraph::new(text).alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(BRD)))
}

fn ui(f: &mut Frame, st: &Stats) {
    let gpu_h = (st.gpu_names.len().max(1) + 2).min(9) as u16;
    let chunks = Layout::vertical([
        Constraint::Length(4),        // header
        Constraint::Length(6),        // stat tiles
        Constraint::Length(gpu_h),    // gpus
        Constraint::Length(5),        // sparkline
        Constraint::Min(4),           // log
        Constraint::Length(1),        // footer
    ]).split(f.area());

    let dot = if st.connected { Span::styled("● LIVE", Style::new().fg(GRN)) } else { Span::styled("● OFFLINE", Style::new().fg(Color::Red)) };
    let head = Paragraph::new(Text::from(vec![
        Line::from(vec![
            Span::styled("PyBLØCK", Style::new().fg(GRN).add_modifier(Modifier::BOLD)),
            Span::styled("  LOTTO BLAKE2b", Style::new().fg(YLW).add_modifier(Modifier::BOLD)),
            Span::raw("   "), dot,
            Span::styled(format!("   {}", st.endpoint), Style::new().fg(MUT)),
        ]),
        Line::from(vec![
            Span::styled("your address  ", Style::new().fg(MUT)),
            Span::styled(st.addr.clone(), Style::new().fg(CYN)),
            Span::styled("   keep 99.1% · ", Style::new().fg(MUT)),
            Span::styled("pool fee 0.9%", Style::new().fg(PNK)),
            Span::styled(format!(" · dev donation {:.1}%", st.donate), Style::new().fg(AMB)),
        ]),
    ])).block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(GRN))
        .title(Span::styled(" ⛏ Bitcoin BLAKE2b · solo lottery ", Style::new().fg(GRN))));
    f.render_widget(head, chunks[0]);

    let row = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(chunks[1]);
    f.render_widget(tile("HASHRATE", Line::from(vec![
        Span::styled(format!("{:.1}", st.hr_total), Style::new().fg(GRN).add_modifier(Modifier::BOLD)),
        Span::styled(" GH/s", Style::new().fg(MUT))]), &format!("{} worker(s)", st.gpu_ghs.len())), row[0]);
    f.render_widget(tile("BLOCKS FOUND", Line::from(
        Span::styled(format!("{}", st.blocks), Style::new().fg(GRN).add_modifier(Modifier::BOLD))),
        &format!("rejected {} · donated {}", st.rejected, st.donated)), row[1]);
    f.render_widget(tile("DIFFICULTY", Line::from(
        Span::styled(format!("bits {}", st.bits), Style::new().fg(YLW).add_modifier(Modifier::BOLD))),
        &format!("diff {:.0}", st.diff)), row[2]);

    let mut glines: Vec<Line> = vec![];
    for (i, g) in st.gpu_ghs.iter().enumerate() {
        let name = st.gpu_names.get(i).cloned().unwrap_or_else(|| format!("GPU {}", i));
        glines.push(Line::from(vec![
            Span::styled(format!(" {:>2}  ", i), Style::new().fg(MUT)),
            Span::styled(format!("{:<26}", name), Style::new().fg(CYN)),
            Span::styled(format!("{:>7.2} GH/s", g), Style::new().fg(GRN)),
        ]));
    }
    if glines.is_empty() { glines.push(Line::from(Span::styled(" warming up…", Style::new().fg(MUT)))); }
    f.render_widget(Paragraph::new(Text::from(glines))
        .block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(BRD)).title(Span::styled(" WORKERS ", Style::new().fg(MUT)))), chunks[2]);

    let data: Vec<u64> = st.hr_hist.iter().cloned().collect();
    f.render_widget(Sparkline::default()
        .block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(BRD)).title(Span::styled(" hashrate ", Style::new().fg(MUT))))
        .data(&data).style(Style::new().fg(GRN)), chunks[3]);

    let items: Vec<ListItem> = st.log.iter().rev().take(chunks[4].height.saturating_sub(2) as usize).rev()
        .map(|l| {
            let col = if l.contains("BLOCK FOUND") { GRN } else if l.contains("donation") { AMB }
                      else if l.contains("rejected") || l.contains("stale") { AMB } else { MUT };
            ListItem::new(Line::from(Span::styled(l.clone(), Style::new().fg(col))))
        }).collect();
    f.render_widget(List::new(items)
        .block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(BRD)).title(Span::styled(" log ", Style::new().fg(MUT)))), chunks[4]);

    f.render_widget(Paragraph::new(Line::from(vec![
        Span::styled(" q ", Style::new().fg(Color::Black).bg(GRN)),
        Span::styled(" quit   ", Style::new().fg(MUT)),
        Span::styled("Until the BLAKE2b hardfork activates on mainnet this is a REGTEST demo (no value). Don't trust, verify.", Style::new().fg(MUT)),
    ])).wrap(Wrap { trim: true }), chunks[5]);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut pool = "pool.pyblock.xyz:23110".to_string();
    let mut addr = String::new();
    let mut gpus: Option<u32> = None; // None = auto-detect all GPUs
    let mut cpu_threads: usize = 0;
    let mut use_cpu = false;
    let mut donate = DONATE_MIN;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--pool" => { i += 1; if i < args.len() { pool = args[i].clone(); } }
            "--addr" => { i += 1; if i < args.len() { addr = args[i].clone(); } }
            "--gpus" => { i += 1; if i < args.len() { gpus = args[i].parse().ok(); } }
            "--cpu" => { use_cpu = true; }
            "--cpu-threads" => { i += 1; if i < args.len() { cpu_threads = args[i].parse().unwrap_or(0); use_cpu = true; } }
            "--donate" => { i += 1; if i < args.len() { donate = args[i].parse().unwrap_or(DONATE_MIN); } }
            _ => {}
        }
        i += 1;
    }
    if addr.is_empty() {
        eprintln!("usage: pyblockMiner --addr <mainnet_btc_address> [--pool host:port] [--gpus N] [--cpu] [--cpu-threads N] [--donate PCT]");
        eprintln!("tip: generate a mainnet address with  python3 tools/genaddr.py");
        std::process::exit(2);
    }
    // mainnet-only: regtest/testnet rewards are unspendable on the real chain → refuse to mine.
    match addr_kind(&addr) {
        "mainnet" => {}
        "test" => {
            eprintln!("✗ '{}' looks like a REGTEST/TESTNET address.", addr);
            eprintln!("  pyblockMiner mines for the mainnet BLAKE2b chain — rewards to a regtest/testnet");
            eprintln!("  address would be UNSPENDABLE. Use a MAINNET address (bc1… / 1… / 3…).");
            eprintln!("  generate one with:  python3 tools/genaddr.py");
            std::process::exit(2);
        }
        _ => {
            eprintln!("✗ '{}' is not a recognized Bitcoin address.", addr);
            eprintln!("  use a MAINNET address (bc1… / 1… / 3…). generate one with:  python3 tools/genaddr.py");
            std::process::exit(2);
        }
    }
    if donate < DONATE_MIN { donate = DONATE_MIN; } // floor — thank you 🙏
    let detected = gpu_names().len() as u32;
    let ngpu = gpus.unwrap_or(detected);
    if ngpu == 0 { use_cpu = true; } // no GPU → mine on CPU
    if use_cpu && cpu_threads == 0 {
        cpu_threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    }
    if !use_cpu { cpu_threads = 0; }

    let stats = Arc::new(Mutex::new(Stats::default()));
    { let mut st = stats.lock().unwrap(); st.endpoint = pool.clone(); st.addr = addr.clone(); st.donate = donate; }
    {
        let stats = stats.clone();
        let (p, a) = (pool.clone(), addr.clone());
        std::thread::spawn(move || engine(stats, p, a, ngpu, cpu_threads, donate));
    }

    let mut terminal = ratatui::init();
    loop {
        { let st = stats.lock().unwrap(); let _ = terminal.draw(|f| ui(f, &st)); }
        if event::poll(Duration::from_millis(150)).unwrap_or(false) {
            if let Ok(Event::Key(k)) = event::read() {
                if matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) { break; }
            }
        }
    }
    ratatui::restore();
}
