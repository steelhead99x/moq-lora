//! moq-lora-send: live video -> MoQ-style objects/groups -> SX1276 FSK frames.
//!
//! Pipeline (all glue in Rust, ffmpeg only as source and encoder):
//!   source ffmpeg (-re, rawvideo yuv420p) -> [Rust: stamp capture time, burn clock + barcode]
//!   -> encoder ffmpeg (H.264, AUD + in-band SPS/PPS) -> [Rust: split access units,
//!   one object per AU, one group per GOP, fragment + XOR parity] -> TX queue with MoQ
//!   group-drop on staleness -> radio (or `--loopback FILE` to test without RF).
#[cfg(target_os = "linux")]
mod radio;

use anyhow::{bail, Context, Result};
use clap::Parser;
use moq_lora_wire::{
    annexb, parse_fsk_profile, read_line_limited, seal, stamp, Fragmenter, Line, ObjHdr,
    MAX_SERIAL_LINE,
};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Parser, Clone)]
#[command(version, about)]
struct Args {
    /// Video source: `testsrc2`, `smptehdbars`, ... (lavfi) or a file path (looped)
    #[arg(long, default_value = "testsrc2")]
    src: String,
    #[arg(long, default_value = "320x240")]
    size: String,
    #[arg(long, default_value_t = 15)]
    fps: u32,
    /// Keyframe interval in frames (= MoQ group length)
    #[arg(long, default_value_t = 15)]
    gop: u32,
    /// Video bitrate in bit/s
    #[arg(long, default_value_t = 180_000)]
    bitrate: u32,
    /// hw = h264_v4l2m2m (Pi VideoCore), x264 = libx264 ultrafast/zerolatency
    #[arg(long, default_value = "x264")]
    encoder: String,
    /// FSK profile BITRATE:FDEV:PREAMBLE_BYTES[:x]  (x = beyond datasheet, stress)
    #[arg(long, default_value = "300000:100000:6")]
    profile: String,
    /// MHz; US 902-928, never 925.0
    #[arg(long, default_value_t = 915.0)]
    freq: f64,
    /// dBm on PA_BOOST (this demo refuses > 5)
    #[arg(long, default_value_t = 5)]
    power: i8,
    /// FEC: one XOR parity frame per K data frames (0 = off, max 15)
    #[arg(long, default_value_t = 0)]
    fec_k: u8,
    /// Zero-pad radio frames shorter than this (a tiny frame right after a 255 B one is
    /// the one the CatWAN misses; 0 = off)
    #[arg(long, default_value_t = 96)]
    min_frame: usize,
    /// Drop (MoQ-style, whole rest of group) objects older than this before they start TX
    #[arg(long, default_value_t = 1000)]
    max_age_ms: u64,
    /// Stop after this many seconds
    #[arg(long, default_value_t = 330.0)]
    secs: f64,
    /// Start at this unix time (sync with the receiver)
    #[arg(long, default_value_t = 0.0)]
    start_at: f64,
    /// Encrypt every object with rind AEAD using the hex key in this file
    #[arg(long)]
    key_file: Option<String>,
    /// chacha20poly1305 | aes128gcm | aes256gcm
    #[arg(long, default_value = "chacha20poly1305")]
    cipher: String,
    /// Per-object JSON lines log
    #[arg(long)]
    log: Option<String>,
    /// Transmit through a CatWAN stick (sx1276-rs catwan-fw) on this serial port instead of
    /// the local SX1276 (reverse direction: Mac -> CatWAN -> Pi)
    #[arg(long)]
    catwan: Option<String>,
    /// No radio: write frames as CatWAN `R` lines to FILE, paced at the FSK airtime
    #[arg(long)]
    loopback: Option<String>,
    #[arg(long, default_value_t = 8_000_000)]
    spi_hz: u32,
    /// Required to transmit
    #[arg(long)]
    i_have_an_antenna: bool,
}

fn unix_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64
}

fn parse_profile(s: &str) -> Result<moq_lora_wire::FskProfile> {
    parse_fsk_profile(s).map_err(|e| anyhow::anyhow!(e))
}

/// Frame sink: the SX1276, or a loopback file.
enum Link {
    #[cfg(target_os = "linux")]
    Radio(Box<radio::Radio>),
    Loop {
        f: std::io::BufWriter<std::fs::File>,
        toa: Duration,
        t0: Instant,
    },
    /// CatWAN USB CDC: `Q <hex>` per frame; the firmware stops reading USB while its
    /// 12-frame TX queue is full, so writes block = natural backpressure. `errs` counts
    /// the stick's `ERR tx` / `ERR txtimeout` lines; `seen` is how many were reported.
    Catwan {
        w: std::fs::File,
        errs: Arc<AtomicU64>,
        seen: u64,
    },
}

fn open_serial(path: &str) -> Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(path)
        .with_context(|| format!("open {path}"))?;
    // SAFETY: plain termios calls on a valid fd.
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(f.as_raw_fd(), &mut t) != 0 {
            bail!("tcgetattr failed");
        }
        libc::cfmakeraw(&mut t);
        t.c_cflag |= libc::CLOCAL | libc::CREAD;
        libc::tcsetattr(f.as_raw_fd(), libc::TCSANOW, &t);
        libc::tcflush(f.as_raw_fd(), libc::TCIOFLUSH);
    }
    Ok(f)
}

/// Configure the CatWAN for FSK TX; returns the write half and a counter of the TX
/// errors the stick reports. A reader thread drains the stick's replies (B/D/ERR lines)
/// so its output buffer never fills.
fn open_catwan(
    port: &str,
    freq_hz: u32,
    p: &moq_lora_wire::FskProfile,
    power: i8,
) -> Result<(std::fs::File, Arc<AtomicU64>)> {
    let f = open_serial(port)?;
    let mut w = f.try_clone()?;
    let (tx, rx) = mpsc::channel::<String>();
    let errs = Arc::new(AtomicU64::new(0));
    let errs_rd = errs.clone();
    std::thread::spawn(move || {
        let mut r = std::io::BufReader::new(f);
        while let Ok(Some(line)) = read_line_limited(&mut r, MAX_SERIAL_LINE) {
            let Line::Data(buf) = line else {
                continue; // oversize reply is not a config response
            };
            let l = String::from_utf8_lossy(&buf);
            if l.starts_with("ERR tx") {
                errs_rd.fetch_add(1, Ordering::Relaxed);
            }
            if tx.send(l.trim().to_string()).is_err() {
                // keep draining after setup
            }
        }
    });
    let mut cmd = |c: &str, want: &str| -> Result<String> {
        w.write_all(format!("{c}\n").as_bytes())?;
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(3) {
            if let Ok(l) = rx.recv_timeout(Duration::from_millis(100)) {
                if l.starts_with(want) {
                    return Ok(l);
                }
            }
        }
        bail!("CatWAN: no {want:?} reply to {c:?}")
    };
    cmd("S", "OK S")?;
    let r = cmd(
        &format!(
            "F {freq_hz} {} {} {power} {} 1 {}",
            p.bitrate, p.fdev, p.preamble, p.unchecked as u8
        ),
        "OK F",
    )?;
    if !r.ends_with("None") {
        bail!("CatWAN config error: {r}");
    }
    eprintln!("CatWAN TX ready: {r}");
    Ok((w, errs))
}
impl Link {
    /// TX errors the CatWAN reported since the last call. They arrive asynchronously,
    /// after the packet they belong to was queued. Always 0 for the other links.
    fn take_stick_errors(&mut self) -> u64 {
        match self {
            Link::Catwan { errs, seen, .. } => {
                let now = errs.load(Ordering::Relaxed);
                let n = now - *seen;
                *seen = now;
                n
            }
            _ => 0,
        }
    }

    fn send(&mut self, fr: &[u8]) -> Result<()> {
        match self {
            #[cfg(target_os = "linux")]
            Link::Radio(r) => r.send(fr),
            Link::Catwan { w, .. } => {
                let mut l = Vec::with_capacity(4 + 2 * fr.len());
                l.extend_from_slice(b"Q ");
                for b in fr {
                    l.extend_from_slice(format!("{b:02x}").as_bytes());
                }
                l.push(b'\n');
                w.write_all(&l)?;
                Ok(())
            }
            Link::Loop { f, toa, t0 } => {
                std::thread::sleep(*toa);
                let hex: String = fr.iter().map(|b| format!("{b:02x}")).collect();
                writeln!(f, "R {} 0 0 {hex}", t0.elapsed().as_micros())?;
                Ok(())
            }
        }
    }
}

struct TxObj {
    group: u16,
    cap_us: u64,
    started: bool,
    frames: VecDeque<Vec<u8>>,
}

#[derive(Default)]
struct TxStats {
    frames: u64,
    bytes: u64,
    objects_dropped: u64,
    groups_dropped: u64,
    tx_errors: u64,
    max_queue_frames: usize,
}

fn spawn(cmd: &[String], stdin: Stdio) -> Result<Child> {
    Command::new(&cmd[0])
        .args(&cmd[1..])
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn {}", cmd[0]))
}

#[allow(deprecated)]
fn cpu_s(who: i32) -> f64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(who, &mut ru) };
    let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    tv(ru.ru_utime) + tv(ru.ru_stime)
}

/// Capture record for the next access unit: its frame number and capture time, and
/// whether a captured frame backs it. An AU the encoder emits beyond the frames it was
/// given has none; it reuses the last real frame number, because the receiver counts
/// expected frames from first to last frame number and any sentinel would inflate it.
fn next_capture(caps: &mut VecDeque<(u32, u64)>, last: &mut u32, now: u64) -> (u32, u64, bool) {
    match caps.pop_front() {
        Some((n, t)) => {
            *last = n;
            (n, t, true)
        }
        None => (*last, now, false),
    }
}

fn random_u32() -> u32 {
    let mut b = [0u8; 4];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .expect("/dev/urandom");
    u32::from_be_bytes(b)
}

#[allow(deprecated)]
fn local_clock(us: u64) -> String {
    // wall clock in the Pi's local time zone
    let t = (us / 1_000_000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        (us / 1000) % 1000
    )
}

fn main() -> Result<()> {
    let a = Args::parse();
    if !(902.0..=928.0).contains(&a.freq) || (a.freq - 925.0).abs() < 0.3 {
        bail!("--freq must be inside 902-928 MHz and not 925.0");
    }
    if a.power > 5 || a.power < 2 {
        bail!("--power must be 2..=5 dBm");
    }
    let (w, h): (usize, usize) = {
        let (x, y) = a.size.split_once('x').context("--size WxH")?;
        (x.parse()?, y.parse()?)
    };
    if w == 0 || w % 40 != 0 || h % 2 != 0 || h < 64 {
        bail!("--size: width must be a non-zero multiple of 40, height even and at least 64");
    }
    // The clock and counter overlay (two text rows, scale w/80) must fit above the barcode.
    let need_h = 2 + 15 * (w / 80).max(1) + stamp::BAR_ROWS;
    if h < need_h {
        bail!(
            "--size {}: too short for the clock overlay at this width (height must be >= {need_h})",
            a.size
        );
    }
    let prof = parse_profile(&a.profile)?;
    let freq_hz = (a.freq * 1e6).round() as u32;
    let frame_toa_us =
        (prof.preamble as u64 + 2 + 1 + 255 + 2) * 8_000_000 / u64::from(prof.bitrate);

    let mut link = match &a.loopback {
        None if a.catwan.is_some() => {
            if !a.i_have_an_antenna {
                bail!("refusing to transmit without --i-have-an-antenna");
            }
            let (w, errs) = open_catwan(a.catwan.as_ref().unwrap(), freq_hz, &prof, a.power)?;
            Link::Catwan { w, errs, seen: 0 }
        }
        Some(p) => Link::Loop {
            f: std::io::BufWriter::new(std::fs::File::create(p)?),
            toa: Duration::from_micros(frame_toa_us),
            t0: Instant::now(),
        },
        None => {
            #[cfg(target_os = "linux")]
            {
                if !a.i_have_an_antenna {
                    bail!("refusing to transmit without --i-have-an-antenna");
                }
                let cfg = sx1276::fsk::FskConfig {
                    bitrate_bps: prof.bitrate,
                    fdev_hz: prof.fdev,
                    preamble_bytes: prof.preamble,
                    unchecked: prof.unchecked,
                    ..Default::default()
                };
                cfg.validate()
                    .map_err(|x| anyhow::anyhow!("fsk config: {x:?}"))?;
                Link::Radio(Box::new(radio::Radio::open(
                    a.spi_hz, freq_hz, &cfg, a.power,
                )?))
            }
            #[cfg(not(target_os = "linux"))]
            bail!("radio needs Linux; use --loopback")
        }
    };

    if a.start_at > 0.0 {
        let wait = a.start_at - unix_us() as f64 / 1e6;
        if wait > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(wait));
        }
    }

    // ---- source + encoder processes
    let ff = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut src_cmd = ff(&[
        "ffmpeg",
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-re",
    ]);
    if std::path::Path::new(&a.src).exists() {
        src_cmd.extend(ff(&["-stream_loop", "-1", "-i", &a.src]));
    } else {
        src_cmd.extend(ff(&[
            "-f",
            "lavfi",
            "-i",
            &format!("{}=size={}:rate={}", a.src, a.size, a.fps),
        ]));
    }
    src_cmd.extend(ff(&[
        "-vf",
        &format!("scale={w}:{h},fps={}", a.fps),
        "-f",
        "rawvideo",
        "-pix_fmt",
        "yuv420p",
        "-",
    ]));
    let br = a.bitrate.to_string();
    let gop = a.gop.to_string();
    let mut enc_cmd = ff(&[
        "ffmpeg",
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "yuv420p",
        "-s",
        &a.size,
        "-r",
        &a.fps.to_string(),
        "-i",
        "-",
    ]);
    match a.encoder.as_str() {
        "hw" => enc_cmd.extend(ff(&["-c:v", "h264_v4l2m2m", "-b:v", &br, "-g", &gop])),
        "x264" => enc_cmd.extend(ff(&[
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-b:v",
            &br,
            "-maxrate",
            &br,
            "-bufsize",
            &(a.bitrate / 2).to_string(),
            "-g",
            &gop,
            "-keyint_min",
            &gop,
            "-sc_threshold",
            "0",
            "-threads",
            "2",
        ])),
        x => bail!("unknown encoder {x}"),
    }
    enc_cmd.extend(ff(&[
        "-bsf:v",
        "dump_extra=freq=keyframe,h264_metadata=aud=insert",
        "-flush_packets",
        "1",
        "-f",
        "h264",
        "-",
    ]));
    eprintln!(
        "source: {}\nencoder: {}",
        src_cmd.join(" "),
        enc_cmd.join(" ")
    );
    let mut src = spawn(&src_cmd, Stdio::null())?;
    let mut enc = spawn(&enc_cmd, Stdio::piped())?;

    let stop = Arc::new(AtomicBool::new(false));
    let caps: Arc<Mutex<VecDeque<(u32, u64)>>> = Arc::default();
    let t_run = Instant::now();
    let cap_done = Arc::new(AtomicBool::new(false));
    let source_stalled = Arc::new(AtomicBool::new(false));

    // ---- capture thread: raw frames -> stamp -> encoder
    let cap_thread = {
        let (mut sout, mut ein) = (src.stdout.take().unwrap(), enc.stdin.take().unwrap());
        let (stop, caps, secs, done) = (stop.clone(), caps.clone(), a.secs, cap_done.clone());
        let label = format!("FSK {}k #", prof.bitrate / 1000);
        std::thread::spawn(move || -> Result<u32> {
            let mut buf = vec![0u8; w * h * 3 / 2];
            let mut n = 0u32;
            while !stop.load(Ordering::Relaxed) && t_run.elapsed().as_secs_f64() < secs {
                if sout.read_exact(&mut buf).is_err() {
                    break;
                }
                let t = unix_us();
                let s = (w / 80).max(1);
                stamp::draw_text(&mut buf, w, h, 2, 2, &local_clock(t), s);
                stamp::draw_text(&mut buf, w, h, 2, 2 + 8 * s, &format!("{label}{n}"), s);
                stamp::write_barcode(&mut buf, w, h, (t / 1000) as u32);
                caps.lock().unwrap().push_back((n, t));
                if ein.write_all(&buf).is_err() {
                    break;
                }
                n += 1;
            }
            done.store(true, Ordering::Relaxed);
            Ok(n) // dropping `ein` closes the encoder input -> encoder flushes and exits
        })
    };

    // ---- watchdog: the capture thread checks --secs and `stop` only between frames, so
    // a source that stays up but sends nothing would block it, and the run, forever.
    // Two seconds past either, kill the source; its pipe closes and the read returns.
    let watchdog = {
        let (done, stop, stalled, secs) = (
            cap_done.clone(),
            stop.clone(),
            source_stalled.clone(),
            a.secs,
        );
        let pid = src.id() as libc::pid_t;
        std::thread::spawn(move || {
            let grace = Duration::from_secs(2);
            let mut stopped_at: Option<Instant> = None;
            while !done.load(Ordering::Relaxed) {
                if stop.load(Ordering::Relaxed) {
                    stopped_at.get_or_insert_with(Instant::now);
                }
                let overdue = t_run.elapsed().as_secs_f64() > secs + grace.as_secs_f64()
                    || stopped_at.is_some_and(|t| t.elapsed() > grace);
                if overdue {
                    eprintln!("video source sent no frame for {grace:?}, killing it");
                    stalled.store(true, Ordering::Relaxed);
                    // SAFETY: main reaps the source only after joining this thread, so the
                    // pid still names it.
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })
    };

    // ---- encoder output reader
    let (ctx, crx) = mpsc::channel::<Vec<u8>>();
    let rd_thread = {
        let mut eout = enc.stdout.take().unwrap();
        std::thread::spawn(move || {
            let mut b = vec![0u8; 65536];
            while let Ok(n) = eout.read(&mut b) {
                if n == 0 || ctx.send(b[..n].to_vec()).is_err() {
                    break;
                }
            }
        })
    };

    // ---- radio thread
    let q: Arc<(Mutex<VecDeque<TxObj>>, Condvar)> = Arc::default();
    let done = Arc::new(AtomicBool::new(false));
    let max_age = a.max_age_ms * 1000;
    let tx_thread = {
        let (q, done, stop) = (q.clone(), done.clone(), stop.clone());
        std::thread::spawn(move || -> (TxStats, Link, f64, bool) {
            let mut st = TxStats::default();
            let mut dropped_group: Option<u16> = None;
            let mut busy = Duration::ZERO;
            let mut fatal = false;
            loop {
                let fr = {
                    let (m, cv) = &*q;
                    let mut g = m.lock().unwrap();
                    loop {
                        st.max_queue_frames = st
                            .max_queue_frames
                            .max(g.iter().map(|o| o.frames.len()).sum());
                        let Some(o) = g.front_mut() else {
                            if done.load(Ordering::Relaxed) {
                                break None;
                            }
                            g = cv.wait_timeout(g, Duration::from_millis(20)).unwrap().0;
                            continue;
                        };
                        if !o.started {
                            if dropped_group == Some(o.group)
                                || unix_us().saturating_sub(o.cap_us) > max_age
                            {
                                if dropped_group != Some(o.group) {
                                    st.groups_dropped += 1;
                                    dropped_group = Some(o.group);
                                }
                                st.objects_dropped += 1;
                                g.pop_front();
                                continue;
                            }
                            o.started = true;
                        }
                        let f = o.frames.pop_front().unwrap();
                        if o.frames.is_empty() {
                            g.pop_front();
                        }
                        break Some(f);
                    }
                };
                let Some(f) = fr else { break };
                let t = Instant::now();
                let mut errs = link.take_stick_errors();
                if errs > 0 {
                    eprintln!("tx error: CatWAN reported {errs}");
                }
                if let Err(e) = link.send(&f) {
                    errs += 1;
                    eprintln!("tx error: {e}");
                }
                st.tx_errors += errs;
                if errs > 0 && st.tx_errors > 20 {
                    // Give up, but let the run wind down and print its summary:
                    // stopping capture closes the encoder, which ends the main loop.
                    eprintln!("too many TX errors, stopping");
                    fatal = true;
                    stop.store(true, Ordering::Relaxed);
                    break;
                }
                busy += t.elapsed();
                st.frames += 1;
                st.bytes += f.len() as u64;
            }
            (st, link, busy.as_secs_f64(), fatal)
        })
    };

    // ---- assembler: AUs -> objects -> frames
    let mut log = a
        .log
        .as_ref()
        .map(std::fs::File::create)
        .transpose()?
        .map(std::io::BufWriter::new);
    let sealer = match &a.key_file {
        Some(kf) => {
            let c = seal::Cipher::parse(&a.cipher).context("--cipher")?;
            Some(seal::open_sealer(std::path::Path::new(kf), c).map_err(|e| anyhow::anyhow!(e))?)
        }
        None => None,
    };
    let mut session = random_u32();
    let (mut last_frame_no, mut uncaptured) = (0u32, 0u64);
    let mut seal_s = 0f64;
    let mut obj_bytes = 0u64;
    let mut frag = Fragmenter::new(a.fec_k).map_err(|x| anyhow::anyhow!("{x:?}"))?;
    frag.min_frame = a.min_frame;
    let mut sp = annexb::AuSplitter::default();
    let (mut group, mut obj, mut objects, mut au_bytes, mut keys) =
        (u16::MAX, 0u8, 0u64, 0u64, 0u64);
    // The closure borrows the assembler state. It has to end before that state
    // is read again below; a block drops it without `mem::drop` on a non-Drop type.
    {
        let mut handle = |au: Vec<u8>| -> Result<()> {
            let now = unix_us();
            let (frame_no, cap_us, captured) =
                next_capture(&mut caps.lock().unwrap(), &mut last_frame_no, now);
            if !captured {
                uncaptured += 1;
                eprintln!("access unit with no capture record, after frame {frame_no}");
            }
            let key = annexb::is_key(&au);
            if key || obj == u8::MAX {
                group = group.wrapping_add(1);
                if group == 0 && objects > 0 {
                    // nonces stay unique across a group-id wrap, including a repeated draw
                    session = seal::session_after_wrap(session, random_u32());
                }
                obj = 0;
                keys += key as u64;
            } else {
                obj += 1;
            }
            let oh = ObjHdr {
                frame_no,
                cap_us,
                enc_us: now.saturating_sub(cap_us).min(u32::MAX as u64) as u32,
                key,
            };
            let mut data = oh.encode().to_vec();
            data.extend_from_slice(&au);
            if let Some(sl) = &sealer {
                let t = Instant::now();
                data = sl.seal(session, group, obj, &data);
                seal_s += t.elapsed().as_secs_f64();
            }
            obj_bytes += data.len() as u64;
            let frames = match frag.fragment(group, obj, &data) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("object {group}/{obj} skipped: {e:?}");
                    return Ok(());
                }
            };
            if let Some(l) = log.as_mut() {
                writeln!(l, "{{\"t_us\":{now},\"group\":{group},\"obj\":{obj},\"frame_no\":{frame_no},\"has_capture\":{captured},\"key\":{key},\"au_bytes\":{},\"frames\":{},\"cap_us\":{cap_us},\"enc_us\":{}}}", au.len(), frames.len(), oh.enc_us)?;
            }
            objects += 1;
            au_bytes += au.len() as u64;
            let (m, cv) = &*q;
            m.lock().unwrap().push_back(TxObj {
                group,
                cap_us,
                started: false,
                frames: frames.into(),
            });
            cv.notify_one();
            Ok(())
        };
        loop {
            match crx.recv_timeout(Duration::from_millis(4)) {
                Ok(chunk) => {
                    for au in sp.push(&chunk) {
                        handle(au)?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(au) = sp.flush() {
                        handle(au)?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if let Some(au) = sp.flush() {
                        handle(au)?;
                    }
                    break;
                }
            }
        }
    }
    let wall = t_run.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    done.store(true, Ordering::Relaxed);
    q.1.notify_all();
    let captured = cap_thread.join().unwrap().unwrap_or(0);
    let _ = watchdog.join(); // before reaping the source, whose pid it may still kill
    let source_stalled = source_stalled.load(Ordering::Relaxed);
    let _ = src.kill();
    let _ = src.wait();
    // The source is killed on purpose, so its exit status says nothing; a source that
    // never produced a frame shows up as `captured == 0`. The encoder exits on its own
    // once its input closes, so a failure there is real.
    let enc_ok = match enc.wait() {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!("waiting for the encoder: {e}");
            false
        }
    };
    let _ = rd_thread.join();
    let (mut st, mut link, busy, mut tx_fatal) = tx_thread.join().unwrap();
    #[cfg(target_os = "linux")]
    if let Link::Radio(r) = &mut link {
        r.idle();
    }
    if let Link::Loop { f, .. } = &mut link {
        f.flush()?;
    }
    if let Link::Catwan { w, .. } = &mut link {
        std::thread::sleep(Duration::from_millis(300)); // let the stick drain its queue
        let _ = w.write_all(b"S\n");
    }
    // Errors the stick reports while draining its queue count toward the same limit.
    st.tx_errors += link.take_stick_errors();
    if st.tx_errors > 20 && !tx_fatal {
        eprintln!("too many TX errors ({})", st.tx_errors);
        tx_fatal = true;
    }
    drop(log);
    let (cpu_self, cpu_child) = (cpu_s(libc::RUSAGE_SELF), cpu_s(libc::RUSAGE_CHILDREN));
    let objs = objects.max(1) as f64;
    println!(
        "{{\"role\":\"send\",\"profile\":\"{}\",\"freq_hz\":{freq_hz},\"power_dbm\":{},\"size\":\"{}\",\"fps\":{},\"gop\":{},\"encoder\":\"{}\",\"bitrate_target\":{},\"fec_k\":{},\"min_frame\":{},\"wall_s\":{wall:.2},\"captured\":{captured},\"objects\":{objects},\"keyframes\":{keys},\"video_bps\":{:.0},\"frames_tx\":{},\"bytes_tx\":{},\"radio_busy_pct\":{:.1},\"air_bps\":{:.0},\"objects_dropped\":{},\"groups_dropped\":{},\"tx_errors\":{},\"max_queue_frames\":{},\"cipher\":\"{}\",\"au_bytes\":{au_bytes},\"object_bytes\":{obj_bytes},\"overhead_per_object\":{{\"objhdr\":{},\"aead\":{},\"frame_headers_crc\":{:.1},\"padding_parity\":{:.1},\"total_air\":{:.1}}},\"seal_us_per_object\":{:.1},\"cpu_pct_sender\":{:.1},\"cpu_pct_ffmpeg\":{:.1},\"cpu_pct_total\":{:.1},\"cpu_cores\":{},\"link\":\"{}\",\"tx_fatal\":{tx_fatal},\"encoder_ok\":{enc_ok},\"source_stalled\":{source_stalled},\"aus_without_capture\":{uncaptured}}}",
        a.profile, a.power, a.size, a.fps, a.gop, a.encoder, a.bitrate, a.fec_k, a.min_frame, au_bytes as f64 * 8.0 / wall, st.frames, st.bytes,
        100.0 * busy / wall, st.bytes as f64 * 8.0 / wall, st.objects_dropped, st.groups_dropped, st.tx_errors, st.max_queue_frames,
        sealer.as_ref().map(|x| x.cipher.name()).unwrap_or("none"),
        moq_lora_wire::OBJ_HDR_LEN,
        if sealer.is_some() { seal::OVERHEAD } else { 0 },
        st.frames as f64 * moq_lora_wire::HDR_LEN as f64 / objs,
        (st.bytes as f64 - st.frames as f64 * moq_lora_wire::HDR_LEN as f64 - obj_bytes as f64) / objs,
        (st.bytes as f64 - au_bytes as f64) / objs,
        1e6 * seal_s / objs,
        100.0 * cpu_self / wall, 100.0 * cpu_child / wall, 100.0 * (cpu_self + cpu_child) / wall,
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        if a.loopback.is_some() { "loopback" } else if a.catwan.is_some() { "catwan" } else { "sx1276" }
    );
    // An AU with no capture record means the encoder and the capture count disagree,
    // so the run's latency and loss numbers can't be trusted.
    if tx_fatal || captured == 0 || !enc_ok || source_stalled || uncaptured > 0 {
        // The summary above is the last line on stdout (the harness reads it with
        // `tail -1`); report the failure through the exit status only.
        eprintln!("run failed: tx_fatal={tx_fatal} captured={captured} encoder_ok={enc_ok} source_stalled={source_stalled} aus_without_capture={uncaptured}");
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::next_capture;
    use std::collections::VecDeque;

    #[test]
    fn uncaptured_au_reuses_the_last_frame_number() {
        let mut caps = VecDeque::from([(7, 100), (8, 200)]);
        let mut last = 0;
        assert_eq!(next_capture(&mut caps, &mut last, 999), (7, 100, true));
        assert_eq!(next_capture(&mut caps, &mut last, 999), (8, 200, true));
        assert_eq!(next_capture(&mut caps, &mut last, 999), (8, 999, false));
        // before any capture: frame 0, not u32::MAX
        assert_eq!(next_capture(&mut VecDeque::new(), &mut 0, 5), (0, 5, false));
    }
}
