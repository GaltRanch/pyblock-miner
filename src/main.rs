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
// dispatch the same work to all daemons, splitting the nonce space PROPORTIONALLY to each GPU's speed
// (so a faster GPU gets more nonces → all finish together → all stay saturated). Collect winners + per-GPU GH/s.
fn grind_daemons(ds: &mut [Daemon], prevhash: &str, ntime: &str, work_root: &str, bits: u32) -> (Vec<String>, Vec<f64>) {
    let total_w: f64 = ds.iter().map(|d| d.weight.max(0.01)).sum();
    let space: u64 = 1u64 << 32;
    let last = ds.len().saturating_sub(1);
    let mut cursor = 0u64;
    for (i, d) in ds.iter_mut().enumerate() {
        let nstart = cursor;
        let span = if i == last { space - nstart } else { ((space as f64) * d.weight.max(0.01) / total_w) as u64 };
        cursor = (nstart + span).min(space);
        let _ = writeln!(d.stdin, "{} {} {} {} {} {}", prevhash, ntime, work_root, bits, nstart, span);
        let _ = d.stdin.flush();
    }
    let mut nonces = vec![];
    let mut ghs = vec![0.0f64; ds.len()];
    for (i, d) in ds.iter_mut().enumerate() {
        loop {
            let mut line = String::new();
            match d.stdout.read_line(&mut line) {
                Ok(0) | Err(_) => break, // daemon died
                Ok(_) => {
                    let t = line.trim();
                    if let Some(rest) = t.strip_prefix("END ") {
                        ghs[i] = rest.parse().unwrap_or(0.0);
                        if ghs[i] > 0.0 { d.weight = ghs[i]; }
                        break;
                    } else if !t.is_empty() {
                        nonces.push(t.to_string());
                    }
                }
            }
        }
    }
    (nonces, ghs)
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

fn engine(stats: Arc<Mutex<Stats>>, pool: String, addr: String, ngpu: u32) {
    // spawn one persistent daemon per GPU (kernel compiled once → all GPUs stay saturated)
    let names = gpu_names();
    let mut daemons: Vec<Daemon> = Vec::new();
    for dev in 0..ngpu {
        let nm = names.get(dev as usize).cloned().unwrap_or_else(|| format!("GPU {}", dev));
        if let Some(d) = spawn_daemon(dev, nm) {
            daemons.push(d);
        }
    }
    { let mut st = stats.lock().unwrap();
      st.gpu_names = daemons.iter().map(|d| d.name.clone()).collect();
      st.gpu_ghs = vec![0.0; daemons.len()];
      let names_str = st.gpu_names.join(", ");
      st.logline(format!("{} GPU(s) ready: {}", daemons.len(), names_str)); }

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
            let (nonces, ghs) = grind_daemons(&mut daemons, &sweep_prevhash, &ntime_hex, &hex::encode(work_root), bits);
            {
                let mut st = stats.lock().unwrap();
                if !ghs.is_empty() { st.gpu_ghs = ghs.clone(); }
                st.hr_total = ghs.iter().sum();
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
        Span::styled(" GH/s", Style::new().fg(MUT))]), &format!("{} GPU(s)", st.gpu_ghs.len())), row[0]);
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
            Span::styled(format!(" GPU {} ", i), Style::new().fg(MUT)),
            Span::styled(format!("{:<24}", name), Style::new().fg(CYN)),
            Span::styled(format!("{:>7.2} GH/s", g), Style::new().fg(GRN)),
        ]));
    }
    if glines.is_empty() { glines.push(Line::from(Span::styled(" warming up…", Style::new().fg(MUT)))); }
    f.render_widget(Paragraph::new(Text::from(glines))
        .block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(BRD)).title(Span::styled(" GPUs ", Style::new().fg(MUT)))), chunks[2]);

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
    let mut gpus: u32 = 0; // 0 = auto (all detected)
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--pool" => { i += 1; if i < args.len() { pool = args[i].clone(); } }
            "--addr" => { i += 1; if i < args.len() { addr = args[i].clone(); } }
            "--gpus" => { i += 1; if i < args.len() { gpus = args[i].parse().unwrap_or(0); } }
            _ => {}
        }
        i += 1;
    }
    if addr.is_empty() {
        eprintln!("usage: pyblockMiner --addr <btc_address> [--pool host:port] [--gpus N]");
        eprintln!("tip: generate an address with  python3 tools/genaddr.py");
        std::process::exit(2);
    }
    if gpus == 0 {
        gpus = gpu_names().len().max(1) as u32;
    }

    let stats = Arc::new(Mutex::new(Stats::default()));
    { let mut st = stats.lock().unwrap(); st.endpoint = pool.clone(); st.addr = addr.clone(); }
    {
        let stats = stats.clone();
        let (p, a) = (pool.clone(), addr.clone());
        std::thread::spawn(move || engine(stats, p, a, gpus));
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
