// pyblockMiner — PyBLØCK LOTTO BLAKE2b miner (Bitcoin BLAKE2b, solo lottery). Rust + ratatui TUI.
// Native SV1 stratum client + work construction + N-GPU grinding via persistent `gpu_grind` daemons
// (one per GPU, OpenCL kernel compiled once → all GPUs saturated, no per-sweep overhead).
// You mine to YOUR address → keep 99.1% of every block · PyBLØCK fee 0.9%. Non-custodial.
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

// PyBLØCK palette
const GRN: Color = Color::Rgb(0, 255, 65);
const YLW: Color = Color::Rgb(255, 255, 0);
const CYN: Color = Color::Rgb(0, 255, 255);
const MUT: Color = Color::Rgb(130, 154, 130);
const AMB: Color = Color::Rgb(224, 176, 53);
const PNK: Color = Color::Rgb(255, 92, 200);
const BRD: Color = Color::Rgb(35, 60, 35);

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
fn read_msgs(stream: &mut TcpStream, buf: &mut Vec<u8>) -> Vec<Value> {
    let mut tmp = [0u8; 8192];
    if let Ok(n) = stream.read(&mut tmp) {
        if n > 0 { buf.extend_from_slice(&tmp[..n]); }
    }
    let mut out = vec![];
    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
        let line: Vec<u8> = buf.drain(..=pos).collect();
        if let Ok(v) = serde_json::from_slice::<Value>(&line[..line.len().saturating_sub(1)]) {
            out.push(v);
        }
    }
    out
}

fn handle_msgs(msgs: Vec<Value>, stats: &Arc<Mutex<Stats>>,
    en1: &mut Option<String>, en2size: &mut usize, diff: &mut f64,
    job: &mut Option<Vec<Value>>, pending: &mut HashMap<u64, String>) {
    for m in msgs {
        let id = m.get("id").and_then(|v| v.as_u64());
        let meth = m.get("method").and_then(|v| v.as_str());
        if id == Some(1) && m.get("result").map_or(false, |r| !r.is_null()) {
            let r = &m["result"];
            *en1 = r.get(1).and_then(|v| v.as_str()).map(|s| s.to_string());
            *en2size = r.get(2).and_then(|v| v.as_u64()).unwrap_or(8) as usize;
            let mut st = stats.lock().unwrap();
            st.logline(format!("subscribed · extranonce1={}", en1.clone().unwrap_or_default()));
        } else if meth == Some("mining.set_difficulty") {
            *diff = m.get("params").and_then(|p| p.get(0)).and_then(|v| v.as_f64()).unwrap_or(1.0);
            let mut st = stats.lock().unwrap();
            st.diff = *diff; st.bits = floor_pot(*diff);
            st.logline(format!("difficulty set · bits={}", floor_pot(*diff)));
        } else if meth == Some("mining.notify") {
            *job = m.get("params").and_then(|v| v.as_array()).cloned();
        } else if let Some(idv) = id {
            if let Some(nonce) = pending.remove(&idv) {
                let mut st = stats.lock().unwrap();
                if m.get("result") == Some(&Value::Bool(true)) {
                    st.blocks += 1;
                    let n = st.blocks;
                    st.logline(format!("🎉 BLOCK FOUND (#{})  paid to your address · nonce {}", n, nonce));
                } else if m.get("error").map_or(false, |e| !e.is_null()) {
                    st.rejected += 1;
                    let e = m.get("error").map(|v| v.to_string()).unwrap_or_default();
                    st.logline(format!("✗ rejected ({})  nonce {}", e, nonce));
                }
            }
        }
    }
}

fn engine(stats: Arc<Mutex<Stats>>, pool: String, addr: String, ngpu: u32, cpu_threads: usize) {
    // spawn one persistent daemon per GPU (kernel compiled once → all GPUs stay saturated)
    let names = gpu_names();
    let mut daemons: Vec<Daemon> = Vec::new();
    for dev in 0..ngpu {
        let nm = names.get(dev as usize).cloned().unwrap_or_else(|| format!("GPU {}", dev));
        if let Some(d) = spawn_daemon(dev, nm) {
            daemons.push(d);
        }
    }
    let mut cpu_rate = 0.05f64;
    { let mut st = stats.lock().unwrap();
      st.gpu_names = daemons.iter().map(|d| d.name.clone()).collect();
      if cpu_threads > 0 { st.gpu_names.push(format!("CPU ({} threads)", cpu_threads)); }
      st.gpu_ghs = vec![0.0; st.gpu_names.len()];
      let nworkers = st.gpu_names.len();
      let names_str = st.gpu_names.join(", ");
      st.logline(format!("{} worker(s) ready: {}", nworkers, names_str)); }

    loop {
        let mut stream = match TcpStream::connect(&pool) {
            Ok(s) => s,
            Err(_) => {
                { let mut st = stats.lock().unwrap(); st.connected = false; st.logline("connection failed — retrying in 3s…".into()); }
                std::thread::sleep(Duration::from_secs(3));
                continue;
            }
        };
        stream.set_nonblocking(true).ok();   // non-blocking reads → the loop grinds continuously (GPUs stay saturated)
        send(&mut stream, &json!({"id":1,"method":"mining.subscribe","params":["PyBLOCK-GPU/BLAKE2b"]}));
        send(&mut stream, &json!({"id":2,"method":"mining.authorize","params":[addr,"x"]}));
        { let mut st = stats.lock().unwrap(); st.connected = true; st.started.get_or_insert(Instant::now()); st.logline(format!("connected to PyBLØCK LOTTO BLAKE2b {}", pool)); }

        let mut buf: Vec<u8> = Vec::new();
        let mut en1: Option<String> = None;
        let mut en2size = 8usize;
        let mut diff = 1f64;
        let mut job: Option<Vec<Value>> = None;
        let mut en2ctr = 0u64;
        let mut pending: HashMap<u64, String> = HashMap::new();
        let mut subid = 100u64;
        let mut idle = Instant::now();

        loop {
            let msgs = read_msgs(&mut stream, &mut buf);
            if !msgs.is_empty() { idle = Instant::now(); }
            handle_msgs(msgs, &stats, &mut en1, &mut en2size, &mut diff, &mut job, &mut pending);
            if idle.elapsed() > Duration::from_secs(90) {
                { let mut st = stats.lock().unwrap(); st.connected = false; st.logline("no data for 90s — reconnecting…".into()); }
                break;
            }
            let (en1v, jobv) = match (&en1, &job) {
                (Some(a), Some(b)) if b.len() >= 8 => (a.clone(), b.clone()),
                _ => { std::thread::sleep(Duration::from_millis(60)); continue; }
            };
            let sweep_prevhash = jobv[1].as_str().unwrap_or("").to_string();
            let job_id = jobv[0].as_str().unwrap_or("").to_string();
            let version = jobv[5].as_str().unwrap_or("").to_string();
            let ntime_hex = jobv[7].as_str().unwrap_or("").to_string();
            let coinb1 = hex::decode(jobv[2].as_str().unwrap_or("")).unwrap_or_default();
            let en2_full = en2ctr.to_le_bytes();
            en2ctr += 1;
            let mut en2 = en2_full[..en2size.min(8)].to_vec();
            en2.resize(en2size.max(1), 0);
            let mut extranonce = hex::decode(&en1v).unwrap_or_default();
            extranonce.extend_from_slice(&en2);
            extranonce.resize(12, 0);
            let mut leaf = vec![0u8];
            leaf.extend_from_slice(&coinb1);
            leaf.extend_from_slice(&extranonce);
            let work_root = b2b(&leaf);
            let bits = floor_pot(diff);
            let (nonces, gpu_ghs, cpu_ghs) = grind_all(&mut daemons, cpu_threads, &mut cpu_rate, &sweep_prevhash, &ntime_hex, &hex::encode(work_root), bits);
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
            // a new block moves the tip during the grind → our nonces are stale, don't submit (avoids reject spam)
            let m2 = read_msgs(&mut stream, &mut buf);
            if !m2.is_empty() { idle = Instant::now(); }
            handle_msgs(m2, &stats, &mut en1, &mut en2size, &mut diff, &mut job, &mut pending);
            let cur_prevhash = job.as_ref().and_then(|j| j.get(1)).and_then(|v| v.as_str()).unwrap_or("");
            if cur_prevhash == sweep_prevhash {
                let en2hex = hex::encode(&en2);
                for nh in nonces {
                    subid += 1;
                    pending.insert(subid, nh.clone());
                    send(&mut stream, &json!({"id":subid,"method":"mining.submit","params":[addr, job_id, en2hex, ntime_hex, nh, version]}));
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
            Span::styled("   you keep 99.1% · ", Style::new().fg(MUT)),
            Span::styled("PyBLØCK fee 0.9%", Style::new().fg(PNK)),
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
        &format!("rejected {}", st.rejected)), row[1]);
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
            let col = if l.contains("BLOCK FOUND") { GRN } else if l.contains("rejected") || l.contains("stale") { AMB } else { MUT };
            ListItem::new(Line::from(Span::styled(l.clone(), Style::new().fg(col))))
        }).collect();
    f.render_widget(List::new(items)
        .block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(BRD)).title(Span::styled(" log ", Style::new().fg(MUT)))), chunks[4]);

    f.render_widget(Paragraph::new(Line::from(vec![
        Span::styled(" q ", Style::new().fg(Color::Black).bg(GRN)),
        Span::styled(" quit   ", Style::new().fg(MUT)),
        Span::styled("REGTEST demo — the coin is not real Bitcoin, no value. Don't trust, verify.", Style::new().fg(MUT)),
    ])).wrap(Wrap { trim: true }), chunks[5]);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut pool = "pool.pyblock.xyz:23110".to_string();
    let mut addr = String::new();
    let mut gpus: Option<u32> = None; // None = auto-detect all GPUs
    let mut cpu_threads: usize = 0;
    let mut use_cpu = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--pool" => { i += 1; if i < args.len() { pool = args[i].clone(); } }
            "--addr" => { i += 1; if i < args.len() { addr = args[i].clone(); } }
            "--gpus" => { i += 1; if i < args.len() { gpus = args[i].parse().ok(); } }
            "--cpu" => { use_cpu = true; }
            "--cpu-threads" => { i += 1; if i < args.len() { cpu_threads = args[i].parse().unwrap_or(0); use_cpu = true; } }
            _ => {}
        }
        i += 1;
    }
    if addr.is_empty() {
        eprintln!("usage: pyblockMiner --addr <btc_address> [--pool host:port] [--gpus N] [--cpu] [--cpu-threads N]");
        eprintln!("tip: generate an address with  python3 tools/genaddr.py");
        std::process::exit(2);
    }
    let detected = gpu_names().len() as u32;
    let ngpu = gpus.unwrap_or(detected);
    if ngpu == 0 { use_cpu = true; } // no GPU → mine on CPU
    if use_cpu && cpu_threads == 0 {
        cpu_threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    }
    if !use_cpu { cpu_threads = 0; }

    let stats = Arc::new(Mutex::new(Stats::default()));
    { let mut st = stats.lock().unwrap(); st.endpoint = pool.clone(); st.addr = addr.clone(); }
    {
        let stats = stats.clone();
        let (p, a) = (pool.clone(), addr.clone());
        std::thread::spawn(move || engine(stats, p, a, ngpu, cpu_threads));
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
