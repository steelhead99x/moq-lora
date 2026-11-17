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
use moq_lora_wire::{annexb, stamp, Fragmenter, ObjHdr};
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
    #[arg(long, default_value = "hw")]
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
    /// Drop (MoQ-style, whole rest of group) objects older than this before they start TX
    #[arg(long, default_value_t = 1000)]
    max_age_ms: u64,
    /// Stop after this many seconds
    #[arg(long, default_value_t = 330.0)]
    secs: f64,
    /// Start at this unix time (sync with the receiver)
    #[arg(long, default_value_t = 0.0)]
    start_at: f64,
    /// Per-object JSON lines log
    #[arg(long)]
    log: Option<String>,
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
}
impl Link {
    fn send(&mut self, fr: &[u8]) -> Result<()> {
        match self {
            #[cfg(target_os = "linux")]
            Link::Radio(r) => r.send(fr),
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
    let mut frag = Fragmenter::new(a.fec_k).map_err(|x| anyhow::anyhow!("{x:?}"))?;
    let mut sp = annexb::AuSplitter::default();
    let (mut group, mut obj, mut objects, mut au_bytes, mut keys) = (u16::MAX, 0u8, 0u64, 0u64, 0u64);
    let mut handle = |au: Vec<u8>| -> Result<()> {
        let now = unix_us();
        let (frame_no, cap_us) = caps.lock().unwrap().pop_front().unwrap_or((u32::MAX, now));
        let key = annexb::is_key(&au);
        if key || obj == u8::MAX {
            group = group.wrapping_add(1);
            obj = 0;
            keys += key as u64;
        } else {
            obj += 1;
        }
        let oh = ObjHdr { frame_no, cap_us, enc_us: now.saturating_sub(cap_us).min(u32::MAX as u64) as u32, key };
        let mut data = oh.encode().to_vec();
        data.extend_from_slice(&au);
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
    drop(log);
    println!(
        "{{\"role\":\"send\",\"profile\":\"{}\",\"freq_hz\":{freq_hz},\"power_dbm\":{},\"size\":\"{}\",\"fps\":{},\"gop\":{},\"encoder\":\"{}\",\"bitrate_target\":{},\"fec_k\":{},\"wall_s\":{wall:.2},\"captured\":{captured},\"objects\":{objects},\"keyframes\":{keys},\"video_bps\":{:.0},\"frames_tx\":{},\"bytes_tx\":{},\"radio_busy_pct\":{:.1},\"air_bps\":{:.0},\"objects_dropped\":{},\"groups_dropped\":{},\"tx_errors\":{},\"max_queue_frames\":{}}}",
        a.profile, a.power, a.size, a.fps, a.gop, a.encoder, a.bitrate, a.fec_k, au_bytes as f64 * 8.0 / wall, st.frames, st.bytes,
        100.0 * busy / wall, st.bytes as f64 * 8.0 / wall, st.objects_dropped, st.groups_dropped, st.tx_errors, st.max_queue_frames
    );
    Ok(())
}
