//! moq-lora-recv: CatWAN USB serial (`R <t> <rssi> <snr> <hex>` lines from the sx1276-rs
//! catwan-fw) -> reassembly + XOR FEC -> MoQ group gate -> H.264 Annex-B to
//! (a) ffplay for live display, (b) a recording, (c) an ffmpeg decoder whose output
//! frames are scanned for the burned-in barcode = capture->decoded latency.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
mod radio;

use anyhow::{bail, Context, Result};
use clap::Parser;
use moq_lora_wire::{seal, stamp, Event, GroupGate, ObjHdr, Reassembler};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// CatWAN serial port (macOS: /dev/cu.usbmodemXXXX)
    #[arg(long)]
    port: Option<String>,
    /// Receive on the local SX1276 (Pi HAT) instead of a CatWAN port (reverse direction)
    #[arg(long)]
    radio: bool,
    /// Replay a file of `R` lines instead of a port (offline test)
    #[arg(long)]
    replay: Option<String>,
    /// With --replay: honour the line timestamps (real-time replay)
    #[arg(long)]
    paced: bool,
    /// FSK BITRATE:FDEV:PREAMBLE[:x]  (must match the sender)
    #[arg(long, default_value = "300000:100000:6")]
    profile: String,
    #[arg(long, default_value_t = 915.0)]
    freq: f64,
    #[arg(long, default_value_t = 330.0)]
    secs: f64,
    /// Output directory (rec.h264, objects.jsonl, decoded.jsonl, summary.json)
    #[arg(long, default_value = "run")]
    out: String,
    /// Live window with ffplay
    #[arg(long)]
    play: bool,
    /// Decode with ffmpeg and read the barcode (needs --size)
    #[arg(long)]
    decode: bool,
    #[arg(long, default_value = "320x240")]
    size: String,
    /// Sender clock minus receiver clock, ms, as printed by --clock-check (latency = t_recv - t_cap_sender + offset)
    #[arg(long, default_value_t = 0.0)]
    clock_offset_ms: f64,
    /// Feed every complete object to the decoder, even inside a broken group
    #[arg(long)]
    no_group_gate: bool,
    /// Measure the sender's clock offset over one persistent ssh session to HOST, then exit
    #[arg(long)]
    clock_check: Option<String>,
    /// Log every serial line (R lines as length only) with host timestamps to OUT/serial.log
    #[arg(long)]
    raw_log: bool,
    /// Decrypt objects with rind AEAD using the hex key in this file (must match the sender)
    #[arg(long)]
    key_file: Option<String>,
    /// chacha20poly1305 | aes128gcm | aes256gcm
    #[arg(long, default_value = "chacha20poly1305")]
    cipher: String,
    /// Stall threshold, ms (gap between consecutive displayed frames)
    #[arg(long, default_value_t = 250.0)]
    stall_ms: f64,
}

fn unix_us() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros() as u64
}

fn open_serial(path: &str) -> Result<File> {
    let f = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOCTTY).open(path).with_context(|| format!("open {path}"))?;
    // SAFETY: plain termios calls on a valid fd.
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(f.as_raw_fd(), &mut t) != 0 {
            bail!("tcgetattr failed");
        }
        libc::cfmakeraw(&mut t);
        t.c_cflag |= libc::CLOCAL | libc::CREAD;
        libc::cfsetspeed(&mut t, libc::B115200);
        libc::tcsetattr(f.as_raw_fd(), libc::TCSANOW, &t);
        libc::tcflush(f.as_raw_fd(), libc::TCIOFLUSH);
    }
    Ok(f)
}

/// A child process fed through a channel, so a slow consumer never blocks the radio loop.
struct Sink {
    tx: Option<mpsc::SyncSender<Vec<u8>>>,
    th: Option<std::thread::JoinHandle<u64>>,
    child: Option<Child>,
}
impl Sink {
    fn new(mut stdin: ChildStdin, child: Option<Child>) -> Sink {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(256);
        let th = std::thread::spawn(move || {
            let mut dropped = 0;
            for b in rx {
                if stdin.write_all(&b).and_then(|_| stdin.flush()).is_err() {
                    dropped += 1;
                }
            }
            dropped
        });
        Sink { tx: Some(tx), th: Some(th), child }
    }
    fn send(&self, b: &[u8]) {
        if let Some(t) = &self.tx {
            let _ = t.try_send(b.to_vec());
        }
    }
    fn close(&mut self, kill_after: Duration) {
        self.tx.take();
        if let Some(t) = self.th.take() {
            let _ = t.join();
        }
        if let Some(mut c) = self.child.take() {
            let t0 = Instant::now();
            while t0.elapsed() < kill_after {
                if let Ok(Some(_)) = c.try_wait() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

const AUD: [u8; 6] = [0, 0, 0, 1, 0x09, 0xf0];

/// Bytes for a live decoder: the access unit minus its leading AUD, followed by the AUD
/// of the *next* access unit. ffmpeg's H.264 parser only closes a frame when it sees the
/// next start of frame, so this lets it decode each frame on arrival instead of one frame
/// (66 ms at 15 fps) later.
fn live_bytes(au: &[u8]) -> Vec<u8> {
    let n = moq_lora_wire::annexb::nals(au);
    let body = match n.as_slice() {
        [(0, 9), (next, _), ..] => &au[*next..],
        [(0, 9)] => &[][..],
        _ => au,
    };
    [body, &AUD[..]].concat()
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn stalls(times_us: &[u64], thr_ms: f64) -> (u32, f64) {
    let mut n = 0;
    let mut worst = 0f64;
    for w in times_us.windows(2) {
        let g = (w[1] - w[0]) as f64 / 1000.0;
        worst = worst.max(g);
        if g > thr_ms {
            n += 1;
        }
    }
    (n, worst)
}

/// Ping-pong `date` over one ssh session; offset = remote - local midpoint of the
/// lowest-RTT exchange (NTP-style).
fn clock_check(host: &str) -> Result<()> {
    let mut c = Command::new("ssh")
        .args(["-o", "BatchMode=yes", host, "while read l; do date +%s%N; done"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut si = c.stdin.take().unwrap();
    let mut so = BufReader::new(c.stdout.take().unwrap());
    let mut best = (f64::MAX, 0f64);
    let mut l = String::new();
    for i in 0..40 {
        let t1 = unix_us() as f64;
        si.write_all(b"x\n")?;
        si.flush()?;
        l.clear();
        so.read_line(&mut l)?;
        let t2 = unix_us() as f64;
        let remote = l.trim().parse::<f64>()? / 1000.0;
        let (rtt, off) = ((t2 - t1) / 1000.0, (remote - (t1 + t2) / 2.0) / 1000.0);
        if i > 2 && rtt < best.0 {
            best = (rtt, off);
        }
    }
    drop(si);
    let _ = c.kill();
    let _ = c.wait();
    println!("{{\"host\":\"{host}\",\"best_rtt_ms\":{:.2},\"remote_minus_local_ms\":{:.2}}}", best.0, best.1);
    Ok(())
}

fn main() -> Result<()> {
    let a = Args::parse();
    if let Some(h) = &a.clock_check {
        return clock_check(h);
    }
    if !(902.0..=928.0).contains(&a.freq) || (a.freq - 925.0).abs() < 0.3 {
        bail!("--freq must be inside 902-928 MHz and not 925.0");
    }
    let (w, h): (usize, usize) = {
        let (x, y) = a.size.split_once('x').context("--size WxH")?;
        (x.parse()?, y.parse()?)
    };
    std::fs::create_dir_all(&a.out)?;
    let p: Vec<&str> = a.profile.split(':').collect();
    if p.len() < 3 {
        bail!("profile BITRATE:FDEV:PREAMBLE[:x]");
    }
    let (br, fdev, pre): (u32, u32, u32) = (p[0].parse()?, p[1].parse()?, p[2].parse()?);
    let unchecked = p.get(3) == Some(&"x");

    // ---- line source
    let (ltx, lrx) = mpsc::channel::<(u64, String)>();
    let mut port_w = None;
    if let Some(port) = &a.port {
        let f = open_serial(port)?;
        let mut wr = f.try_clone()?;
        let rd = f;
        std::thread::spawn(move || {
            let mut r = BufReader::with_capacity(1 << 16, rd);
            let mut l = Vec::new();
            loop {
                l.clear();
                match r.read_until(b'\n', &mut l) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if ltx.send((unix_us(), String::from_utf8_lossy(&l).trim().to_string())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let cmd = |wr: &mut File, c: &str, want: &str| -> Result<String> {
            wr.write_all(format!("{c}\n").as_bytes())?;
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(3) {
                if let Ok((_, l)) = lrx.recv_timeout(Duration::from_millis(100)) {
                    if l.starts_with(want) {
                        return Ok(l);
                    }
                }
            }
            bail!("no {want:?} reply to {c:?}")
        };
        cmd(&mut wr, "S", "OK S")?;
        let freq_hz = (a.freq * 1e6).round() as u32;
        let r = cmd(&mut wr, &format!("F {freq_hz} {br} {fdev} 5 {pre} 1 {}", unchecked as u8), "OK F")?;
        if !r.ends_with("None") {
            bail!("CatWAN config error: {r}");
        }
        cmd(&mut wr, "R", "OK R")?;
        eprintln!("CatWAN listening: {r}");
        port_w = Some(wr);
    } else if a.radio {
        #[cfg(target_os = "linux")]
        {
            let cfg = sx1276::fsk::FskConfig { bitrate_bps: br, fdev_hz: fdev, preamble_bytes: pre as u16, unchecked, ..Default::default() };
            cfg.validate().map_err(|x| anyhow::anyhow!("fsk config: {x:?}"))?;
            let mut r = radio::Radio::open(8_000_000, (a.freq * 1e6).round() as u32, &cfg, 2)?;
            r.start_rx()?;
            let stall = 36 * 8_000_000 / br;
            eprintln!("Pi SX1276 listening (FSK {br} bit/s)");
            std::thread::spawn(move || {
                let t0 = Instant::now();
                let mut st = sx1276::FskLongRx::default();
                loop {
                    let now = t0.elapsed().as_micros() as u32;
                    let line = match r.poll(&mut st, now, stall) {
                        Ok(Some((p, rssi))) => format!("R {now} {rssi} 0 {}", p.iter().map(|b| format!("{b:02x}")).collect::<String>()),
                        Ok(None) => continue,
                        Err(x) => format!("X {now} {x}"),
                    };
                    if ltx.send((unix_us(), line)).is_err() {
                        break;
                    }
                }
                r.idle();
            });
        }
        #[cfg(not(target_os = "linux"))]
        bail!("--radio needs Linux");
    } else if let Some(rp) = &a.replay {
        let f = BufReader::new(File::open(rp)?);
        let paced = a.paced;
        std::thread::spawn(move || {
            let t0 = Instant::now();
            for l in f.lines().map_while(Result::ok) {
                if paced {
                    if let Some(us) = l.split(' ').nth(1).and_then(|x| x.parse::<u64>().ok()) {
                        let due = Duration::from_micros(us);
                        if let Some(w) = due.checked_sub(t0.elapsed()) {
                            std::thread::sleep(w);
                        }
                    }
                }
                if ltx.send((unix_us(), l)).is_err() {
                    break;
                }
            }
        });
    } else {
        bail!("need --port or --replay");
    }

    // ---- sinks
    let mut raw = if a.raw_log { Some(BufWriter::new(File::create(format!("{}/serial.log", a.out))?)) } else { None };
    let mut rec = BufWriter::new(File::create(format!("{}/rec.h264", a.out))?);
    let mut objlog = BufWriter::new(File::create(format!("{}/objects.jsonl", a.out))?);
    // single-threaded decode: frame threading would hold back (threads-1) frames
    let low = ["-fflags", "nobuffer", "-flags", "low_delay", "-probesize", "32", "-analyzeduration", "0", "-threads", "1"];
    let mut play = if a.play {
        let mut c = Command::new("ffplay")
            .args(["-hide_banner", "-loglevel", "error"])
            .args(low)
            .args(["-framedrop", "-sync", "ext", "-window_title", "LoRa SX1276 live video (moq-lora)", "-x", "640", "-y", "480", "-f", "h264", "-i", "pipe:0"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn ffplay")?;
        let si = c.stdin.take().unwrap();
        Some(Sink::new(si, Some(c)))
    } else {
        None
    };
    let mut dec_th = None;
    let mut dec = if a.decode {
        let mut c = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error"])
            .args(low)
            .args(["-f", "h264", "-i", "pipe:0", "-f", "rawvideo", "-pix_fmt", "gray", "-s", &a.size, "-fps_mode", "passthrough", "-flush_packets", "1", "pipe:1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn ffmpeg decoder")?;
        let si = c.stdin.take().unwrap();
        let mut so = c.stdout.take().unwrap();
        let path = format!("{}/decoded.jsonl", a.out);
        let off = a.clock_offset_ms;
        dec_th = Some(std::thread::spawn(move || -> (Vec<u64>, Vec<f64>, u64) {
            let mut lg = BufWriter::new(File::create(path).unwrap());
            let mut buf = vec![0u8; w * h];
            let (mut times, mut lat, mut unread) = (Vec::new(), Vec::new(), 0u64);
            while so.read_exact(&mut buf).is_ok() {
                let t = unix_us();
                times.push(t);
                match stamp::read_barcode(&buf, w, h) {
                    Some(v) => {
                        let now_ms = (t / 1000) as u32;
                        let l = now_ms.wrapping_sub(v) as i32 as f64 + off;
                        lat.push(l);
                        let _ = writeln!(lg, "{{\"t_us\":{t},\"cap_ms_lo\":{v},\"lat_ms\":{l:.1}}}");
                    }
                    None => {
                        unread += 1;
                        let _ = writeln!(lg, "{{\"t_us\":{t},\"cap_ms_lo\":null}}");
                    }
                }
            }
            (times, lat, unread)
        }));
        Some(Sink::new(si, Some(c)))
    } else {
        None
    };

    for s in play.iter().chain(dec.iter()) {
        s.send(&AUD);
    }
    let sealer = match &a.key_file {
        Some(kf) => {
            let c = seal::Cipher::parse(&a.cipher).context("--cipher")?;
            let k = seal::parse_key_hex(&std::fs::read_to_string(kf)?).context("key file must be hex")?;
            Some(seal::ObjectSealer::new(c, &k).map_err(|x| anyhow::anyhow!(x))?)
        }
        None => None,
    };
    let (mut auth_fail, mut open_s) = (0u64, 0f64);
    // ---- main loop
    let mut re = Reassembler::new();
    let mut gate = GroupGate::default();
    let t0 = Instant::now();
    let (mut ok_objs, mut lost_objs, mut delivered_bytes, mut au_bytes_all) = (0u64, 0u64, 0u64, 0u64);
    let (mut rx_err, mut first_frame, mut last_frame) = (0u64, None::<u64>, 0u64);
    let mut obj_lat = Vec::new();
    let mut rx_bytes = 0u64;
    let mut obj_times = Vec::new();
    let mut rssi = Vec::new();
    let mut last_rx = Instant::now();
    let mut first_frame_no: Option<u32> = None;
    let mut last_frame_no = 0u32;
    let mut handle = |e: Event, t: u64| -> Result<()> {
        match e {
            Event::Object { group, obj, data, recovered } => {
                let data = match &sealer {
                    Some(sl) => {
                        let t0 = Instant::now();
                        let r = sl.open(group, obj, &data);
                        open_s += t0.elapsed().as_secs_f64();
                        match r {
                            Some(d) => d,
                            None => {
                                auth_fail += 1;
                                lost_objs += 1;
                                gate.lost(group, obj);
                                writeln!(objlog, "{{\"t_us\":{t},\"group\":{group},\"obj\":{obj},\"lost\":true,\"auth_fail\":true}}")?;
                                return Ok(());
                            }
                        }
                    }
                    None => data,
                };
                let Some((oh, au)) = ObjHdr::decode(&data) else { return Ok(()) };
                ok_objs += 1;
                au_bytes_all += au.len() as u64;
                first_frame_no.get_or_insert(oh.frame_no);
                last_frame_no = oh.frame_no;
                let pass = a.no_group_gate || gate.accept(group, obj);
                let lat = (t as f64 - oh.cap_us as f64) / 1000.0 + a.clock_offset_ms;
                writeln!(objlog, "{{\"t_us\":{t},\"group\":{group},\"obj\":{obj},\"frame_no\":{},\"key\":{},\"au_bytes\":{},\"recovered\":{recovered},\"passed\":{pass},\"enc_ms\":{:.1},\"lat_ms\":{lat:.1}}}", oh.frame_no, oh.key, au.len(), oh.enc_us as f64 / 1000.0)?;
                if pass {
                    delivered_bytes += au.len() as u64;
                    obj_lat.push(lat);
                    obj_times.push(t);
                    rec.write_all(au)?;
                    let live = live_bytes(au);
                    if let Some(s) = &play {
                        s.send(&live);
                    }
                    if let Some(s) = &dec {
                        s.send(&live);
                    }
                }
            }
            Event::Lost { group, obj, have, need } => {
                lost_objs += 1;
                gate.lost(group, obj);
                writeln!(objlog, "{{\"t_us\":{t},\"group\":{group},\"obj\":{obj},\"lost\":true,\"have\":{have},\"need\":{need}}}")?;
            }
        }
        Ok(())
    };
    while t0.elapsed().as_secs_f64() < a.secs {
        match lrx.recv_timeout(Duration::from_millis(20)) {
            Ok((t, l)) => {
                if let Some(r) = raw.as_mut() {
                    match l.rsplit_once(' ') {
                        Some((head, hx)) if l.starts_with("R ") => writeln!(r, "{t} {head} len={} seq={}", hx.len() / 2, hx.get(0..4).unwrap_or(""))?,
                        _ => writeln!(r, "{t} {l}")?,
                    }
                }
                if let Some(rest) = l.strip_prefix("R ") {
                    let mut it = rest.split(' ');
                    let (_tdev, rs, _snr, hx) = (it.next(), it.next(), it.next(), it.next().unwrap_or(""));
                    if let Some(r) = rs.and_then(|s| s.parse::<i32>().ok()) {
                        rssi.push(r as f64);
                    }
                    let fr: Option<Vec<u8>> = (0..hx.len() / 2).map(|i| u8::from_str_radix(&hx[2 * i..2 * i + 2], 16).ok()).collect();
                    if let Some(fr) = fr {
                        first_frame.get_or_insert(t);
                        rx_bytes += fr.len() as u64;
                        last_frame = t;
                        last_rx = Instant::now();
                        for e in re.push(&fr) {
                            handle(e, t)?;
                        }
                    }
                } else if l.starts_with("X ") {
                    rx_err += 1;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if a.replay.is_some() {
                    break;
                }
                bail!("serial port closed");
            }
        }
        if last_rx.elapsed() > Duration::from_millis(150) {
            if let Some(e) = re.flush() {
                handle(e, unix_us())?;
            }
        }
    }
    if let Some(e) = re.flush() {
        handle(e, unix_us())?;
    }
    let mut stats_line = String::new();
    if let Some(mut wr) = port_w {
        wr.write_all(b"S\n")?;
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(2) {
            if let Ok((_, l)) = lrx.recv_timeout(Duration::from_millis(100)) {
                if l.starts_with("OK S") {
                    stats_line = l;
                    break;
                }
            }
        }
    }
    rec.flush()?;
    objlog.flush()?;
    if let Some(r) = raw.as_mut() {
        r.flush()?;
    }
    if let Some(s) = play.as_mut() {
        s.close(Duration::from_secs(3));
    }
    let (dtimes, mut dlat, unread) = if let Some(mut s) = dec.take() {
        s.close(Duration::from_secs(5));
        dec_th.take().unwrap().join().unwrap()
    } else {
        (Vec::new(), Vec::new(), 0)
    };
    let span = first_frame.map(|f| (last_frame - f) as f64 / 1e6).unwrap_or(0.0);
    let (ost, oworst) = stalls(&obj_times, a.stall_ms);
    let (dst, dworst) = stalls(&dtimes, a.stall_ms);
    let frames_expected = last_frame_no.saturating_sub(first_frame_no.unwrap_or(0)) as u64 + 1;
    let s = format!(
        "{{\"role\":\"recv\",\"profile\":\"{}\",\"span_s\":{span:.2},\"radio_frames_ok\":{},\"radio_frames_missing\":{},\"radio_frames_bad\":{},\"radio_frame_loss\":{:.5},\"rx_err_lines\":{rx_err},\"fec_recovered\":{},\"objects_ok\":{ok_objs},\"objects_lost\":{lost_objs},\"objects_delivered\":{},\"frames_expected\":{frames_expected},\"video_frame_loss\":{:.5},\"groups_cut\":{},\"objects_skipped_by_gate\":{},\"delivered_video_bps\":{:.0},\"received_video_bps\":{:.0},\"obj_lat_ms\":{{\"p50\":{:.1},\"p95\":{:.1},\"max\":{:.1}}},\"decoded_frames\":{},\"barcode_unread\":{unread},\"g2g_lat_ms\":{{\"p50\":{:.1},\"p95\":{:.1},\"max\":{:.1}}},\"stalls_obj\":{ost},\"worst_gap_obj_ms\":{oworst:.0},\"stalls_decoded\":{dst},\"worst_gap_decoded_ms\":{dworst:.0},\"stall_threshold_ms\":{},\"rssi_mean\":{:.1},\"clock_offset_ms\":{},\"cipher\":\"{}\",\"auth_fail\":{auth_fail},\"open_us_per_object\":{:.1},\"radio_goodput_bps\":{:.0},\"catwan\":\"{}\"}}",
        a.profile, re.frames_ok, re.frames_missing, re.frames_bad,
        re.frames_missing as f64 / (re.frames_ok + re.frames_missing).max(1) as f64,
        re.fec_recovered, obj_times.len(),
        1.0 - obj_times.len() as f64 / frames_expected.max(1) as f64,
        gate.groups_cut, gate.objects_skipped,
        delivered_bytes as f64 * 8.0 / span.max(1e-9), au_bytes_all as f64 * 8.0 / span.max(1e-9),
        pct(&mut obj_lat.clone(), 0.5), pct(&mut obj_lat.clone(), 0.95), obj_lat.iter().cloned().fold(f64::NAN, f64::max),
        dtimes.len(), pct(&mut dlat.clone(), 0.5), pct(&mut dlat.clone(), 0.95), dlat.iter_mut().map(|x| *x).fold(f64::NAN, f64::max),
        a.stall_ms, rssi.iter().sum::<f64>() / rssi.len().max(1) as f64, a.clock_offset_ms,
        sealer.as_ref().map(|x| x.cipher.name()).unwrap_or("none"),
        1e6 * open_s / ok_objs.max(1) as f64,
        rx_bytes as f64 * 8.0 / span.max(1e-9),
        stats_line
    );
    std::fs::write(format!("{}/summary.json", a.out), &s)?;
    println!("{s}");
    Ok(())
}
