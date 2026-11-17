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
use moq_lora_wire::{annexb, seal, stamp, Fragmenter, ObjHdr};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
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
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros() as u64
}

struct FskProfile {
    bitrate: u32,
    fdev: u32,
    pre: u16,
    unchecked: bool,
}
fn parse_profile(s: &str) -> Result<FskProfile> {
    let p: Vec<&str> = s.split(':').collect();
    if p.len() < 3 || p.len() > 4 || (p.len() == 4 && p[3] != "x") {
        bail!("profile must be BITRATE:FDEV:PREAMBLE[:x]");
    }
    Ok(FskProfile { bitrate: p[0].parse()?, fdev: p[1].parse()?, pre: p[2].parse()?, unchecked: p.len() == 4 })
}

/// Frame sink: the SX1276, or a loopback file.
enum Link {
    #[cfg(target_os = "linux")]
    Radio(radio::Radio),
    Loop { f: std::io::BufWriter<std::fs::File>, toa: Duration, t0: Instant },
    /// CatWAN USB CDC: `Q <hex>` per frame; the firmware stops reading USB while its
    /// 12-frame TX queue is full, so writes block = natural backpressure.
    Catwan { w: std::fs::File },
}

fn open_serial(path: &str) -> Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let f = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOCTTY).open(path).with_context(|| format!("open {path}"))?;
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

/// Configure the CatWAN for FSK TX; returns the write half. A reader thread drains the
/// stick's replies (B/D/ERR lines) so its output buffer never fills.
fn open_catwan(port: &str, freq_hz: u32, p: &FskProfile, power: i8) -> Result<std::fs::File> {
    use std::io::BufRead;
    let f = open_serial(port)?;
    let mut w = f.try_clone()?;
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut r = std::io::BufReader::new(f);
        let mut l = String::new();
        while r.read_line(&mut l).map(|n| n > 0).unwrap_or(false) {
            if tx.send(l.trim().to_string()).is_err() {
                // keep draining after setup
            }
            l.clear();
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
    let r = cmd(&format!("F {freq_hz} {} {} {power} {} 1 {}", p.bitrate, p.fdev, p.pre, p.unchecked as u8), "OK F")?;
    if !r.ends_with("None") {
        bail!("CatWAN config error: {r}");
    }
    eprintln!("CatWAN TX ready: {r}");
    Ok(w)
}
impl Link {
    fn send(&mut self, fr: &[u8]) -> Result<()> {
        match self {
            #[cfg(target_os = "linux")]
            Link::Radio(r) => r.send(fr),
            Link::Catwan { w } => {
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
    Command::new(&cmd[0]).args(&cmd[1..]).stdin(stdin).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().with_context(|| format!("spawn {}", cmd[0]))
}

#[allow(deprecated)]
fn cpu_s(who: i32) -> f64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(who, &mut ru) };
    let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    tv(ru.ru_utime) + tv(ru.ru_stime)
}

fn random_u32() -> u32 {
    let mut b = [0u8; 4];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).expect("/dev/urandom");
    u32::from_be_bytes(b)
}

#[allow(deprecated)]
fn local_clock(us: u64) -> String {
    // wall clock in the Pi's local time zone
    let t = (us / 1_000_000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:02}:{:02}:{:02}.{:03}", tm.tm_hour, tm.tm_min, tm.tm_sec, (us / 1000) % 1000)
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
    if w % 40 != 0 || h % 2 != 0 || h < 64 {
        bail!("width must be a multiple of 40");
    }
    let prof = parse_profile(&a.profile)?;
    let freq_hz = (a.freq * 1e6).round() as u32;
    let frame_toa_us = (prof.pre as u64 + 2 + 1 + 255 + 2) * 8_000_000 / prof.bitrate as u64;

    let mut link = match &a.loopback {
        None if a.catwan.is_some() => {
            if !a.i_have_an_antenna {
                bail!("refusing to transmit without --i-have-an-antenna");
            }
            Link::Catwan { w: open_catwan(a.catwan.as_ref().unwrap(), freq_hz, &prof, a.power)? }
        }
        Some(p) => Link::Loop { f: std::io::BufWriter::new(std::fs::File::create(p)?), toa: Duration::from_micros(frame_toa_us), t0: Instant::now() },
        None => {
            #[cfg(target_os = "linux")]
            {
                if !a.i_have_an_antenna {
                    bail!("refusing to transmit without --i-have-an-antenna");
                }
                let cfg = sx1276::fsk::FskConfig { bitrate_bps: prof.bitrate, fdev_hz: prof.fdev, preamble_bytes: prof.pre, unchecked: prof.unchecked, ..Default::default() };
                cfg.validate().map_err(|x| anyhow::anyhow!("fsk config: {x:?}"))?;
                Link::Radio(radio::Radio::open(a.spi_hz, freq_hz, &cfg, a.power)?)
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
    let mut src_cmd = ff(&["ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin", "-re"]);
    if std::path::Path::new(&a.src).exists() {
        src_cmd.extend(ff(&["-stream_loop", "-1", "-i", &a.src]));
    } else {
        src_cmd.extend(ff(&["-f", "lavfi", "-i", &format!("{}=size={}:rate={}", a.src, a.size, a.fps)]));
    }
    src_cmd.extend(ff(&["-vf", &format!("scale={w}:{h},fps={}", a.fps), "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"]));
    let br = a.bitrate.to_string();
    let gop = a.gop.to_string();
    let mut enc_cmd = ff(&["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-s", &a.size, "-r", &a.fps.to_string(), "-i", "-"]);
    match a.encoder.as_str() {
        "hw" => enc_cmd.extend(ff(&["-c:v", "h264_v4l2m2m", "-b:v", &br, "-g", &gop])),
        "x264" => enc_cmd.extend(ff(&[
            "-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency", "-b:v", &br, "-maxrate", &br,
            "-bufsize", &(a.bitrate / 2).to_string(), "-g", &gop, "-keyint_min", &gop, "-sc_threshold", "0", "-threads", "2",
        ])),
        x => bail!("unknown encoder {x}"),
    }
    enc_cmd.extend(ff(&["-bsf:v", "dump_extra=freq=keyframe,h264_metadata=aud=insert", "-flush_packets", "1", "-f", "h264", "-"]));
    eprintln!("source: {}\nencoder: {}", src_cmd.join(" "), enc_cmd.join(" "));
    let mut src = spawn(&src_cmd, Stdio::null())?;
    let mut enc = spawn(&enc_cmd, Stdio::piped())?;

    let stop = Arc::new(AtomicBool::new(false));
    let caps: Arc<Mutex<VecDeque<(u32, u64)>>> = Arc::default();
    let t_run = Instant::now();

    // ---- capture thread: raw frames -> stamp -> encoder
    let cap_thread = {
        let (mut sout, mut ein) = (src.stdout.take().unwrap(), enc.stdin.take().unwrap());
        let (stop, caps, secs) = (stop.clone(), caps.clone(), a.secs);
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
                stamp::draw_text(&mut buf, w, 2, 2, &local_clock(t), s);
                stamp::draw_text(&mut buf, w, 2, 2 + 8 * s, &format!("{label}{n}"), s);
                stamp::write_barcode(&mut buf, w, h, (t / 1000) as u32);
                caps.lock().unwrap().push_back((n, t));
                if ein.write_all(&buf).is_err() {
                    break;
                }
                n += 1;
            }
            Ok(n) // dropping `ein` closes the encoder input -> encoder flushes and exits
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
        let (q, done) = (q.clone(), done.clone());
        std::thread::spawn(move || -> Result<(TxStats, Link, f64)> {
            let mut st = TxStats::default();
            let mut dropped_group: Option<u16> = None;
            let mut busy = Duration::ZERO;
            loop {
                let fr = {
                    let (m, cv) = &*q;
                    let mut g = m.lock().unwrap();
                    loop {
                        st.max_queue_frames = st.max_queue_frames.max(g.iter().map(|o| o.frames.len()).sum());
                        let Some(o) = g.front_mut() else {
                            if done.load(Ordering::Relaxed) {
                                break None;
                            }
                            g = cv.wait_timeout(g, Duration::from_millis(20)).unwrap().0;
                            continue;
                        };
                        if !o.started {
                            if dropped_group == Some(o.group) || unix_us().saturating_sub(o.cap_us) > max_age {
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
                if let Err(e) = link.send(&f) {
                    st.tx_errors += 1;
                    eprintln!("tx error: {e}");
                    if st.tx_errors > 20 {
                        bail!("too many TX errors");
                    }
                }
                busy += t.elapsed();
                st.frames += 1;
                st.bytes += f.len() as u64;
            }
            Ok((st, link, busy.as_secs_f64()))
        })
    };

    // ---- assembler: AUs -> objects -> frames
    let mut log = a.log.as_ref().map(|p| std::fs::File::create(p)).transpose()?.map(std::io::BufWriter::new);
    let sealer = match &a.key_file {
        Some(kf) => {
            let c = seal::Cipher::parse(&a.cipher).context("--cipher")?;
            let k = seal::parse_key_hex(&std::fs::read_to_string(kf)?).context("key file must be hex")?;
            Some(seal::ObjectSealer::new(c, &k).map_err(|x| anyhow::anyhow!(x))?)
        }
        None => None,
    };
    let mut session = random_u32();
    let mut seal_s = 0f64;
    let mut obj_bytes = 0u64;
    let mut frag = Fragmenter::new(a.fec_k).map_err(|x| anyhow::anyhow!("{x:?}"))?;
    frag.min_frame = a.min_frame;
    let mut sp = annexb::AuSplitter::default();
    let (mut group, mut obj, mut objects, mut au_bytes, mut keys) = (u16::MAX, 0u8, 0u64, 0u64, 0u64);
    let mut handle = |au: Vec<u8>| -> Result<()> {
        let now = unix_us();
        let (frame_no, cap_us) = caps.lock().unwrap().pop_front().unwrap_or((u32::MAX, now));
        let key = annexb::is_key(&au);
        if key || obj == u8::MAX {
            group = group.wrapping_add(1);
            if group == 0 && objects > 0 {
                session = random_u32(); // nonces stay unique across a group-id wrap
            }
            obj = 0;
            keys += key as u64;
        } else {
            obj += 1;
        }
        let oh = ObjHdr { frame_no, cap_us, enc_us: now.saturating_sub(cap_us).min(u32::MAX as u64) as u32, key };
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
            writeln!(l, "{{\"t_us\":{now},\"group\":{group},\"obj\":{obj},\"frame_no\":{frame_no},\"key\":{key},\"au_bytes\":{},\"frames\":{},\"cap_us\":{cap_us},\"enc_us\":{}}}", au.len(), frames.len(), oh.enc_us)?;
        }
        objects += 1;
        au_bytes += au.len() as u64;
        let (m, cv) = &*q;
        m.lock().unwrap().push_back(TxObj { group, cap_us, started: false, frames: frames.into() });
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
    let wall = t_run.elapsed().as_secs_f64();
    drop(handle);
    stop.store(true, Ordering::Relaxed);
    done.store(true, Ordering::Relaxed);
    q.1.notify_all();
    let captured = cap_thread.join().unwrap().unwrap_or(0);
    let _ = src.kill();
    let _ = src.wait();
    let _ = enc.wait();
    let _ = rd_thread.join();
    let (st, mut link, busy) = tx_thread.join().unwrap()?;
    #[cfg(target_os = "linux")]
    if let Link::Radio(r) = &mut link {
        r.idle();
    }
    if let Link::Loop { f, .. } = &mut link {
        f.flush()?;
    }
    if let Link::Catwan { w } = &mut link {
        std::thread::sleep(Duration::from_millis(300)); // let the stick drain its queue
        let _ = w.write_all(b"S\n");
    }
    drop(log);
    let (cpu_self, cpu_child) = (cpu_s(libc::RUSAGE_SELF), cpu_s(libc::RUSAGE_CHILDREN));
    let objs = objects.max(1) as f64;
    println!(
        "{{\"role\":\"send\",\"profile\":\"{}\",\"freq_hz\":{freq_hz},\"power_dbm\":{},\"size\":\"{}\",\"fps\":{},\"gop\":{},\"encoder\":\"{}\",\"bitrate_target\":{},\"fec_k\":{},\"min_frame\":{},\"wall_s\":{wall:.2},\"captured\":{captured},\"objects\":{objects},\"keyframes\":{keys},\"video_bps\":{:.0},\"frames_tx\":{},\"bytes_tx\":{},\"radio_busy_pct\":{:.1},\"air_bps\":{:.0},\"objects_dropped\":{},\"groups_dropped\":{},\"tx_errors\":{},\"max_queue_frames\":{},\"cipher\":\"{}\",\"au_bytes\":{au_bytes},\"object_bytes\":{obj_bytes},\"overhead_per_object\":{{\"objhdr\":{},\"aead\":{},\"frame_headers_crc\":{:.1},\"padding_parity\":{:.1},\"total_air\":{:.1}}},\"seal_us_per_object\":{:.1},\"cpu_pct_sender\":{:.1},\"cpu_pct_ffmpeg\":{:.1},\"cpu_pct_total\":{:.1},\"cpu_cores\":4}}",
        a.profile, a.power, a.size, a.fps, a.gop, a.encoder, a.bitrate, a.fec_k, a.min_frame, au_bytes as f64 * 8.0 / wall, st.frames, st.bytes,
        100.0 * busy / wall, st.bytes as f64 * 8.0 / wall, st.objects_dropped, st.groups_dropped, st.tx_errors, st.max_queue_frames,
        sealer.as_ref().map(|x| x.cipher.name()).unwrap_or("none"),
        moq_lora_wire::OBJ_HDR_LEN,
        if sealer.is_some() { seal::OVERHEAD } else { 0 },
        st.frames as f64 * moq_lora_wire::HDR_LEN as f64 / objs,
        (st.bytes as f64 - st.frames as f64 * moq_lora_wire::HDR_LEN as f64 - obj_bytes as f64) / objs,
        (st.bytes as f64 - au_bytes as f64) / objs,
        1e6 * seal_s / objs,
        100.0 * cpu_self / wall, 100.0 * cpu_child / wall, 100.0 * (cpu_self + cpu_child) / wall
    );
    Ok(())
}
