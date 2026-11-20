//! The moq-lora side of the old-stack demo (`demo/oldstack`).
//!
//! Payloads go through the real `moq-lora-wire` code: object header, per-object
//! ChaCha20-Poly1305 seal, 255-byte radio frames and XOR parity. The radio is replaced by
//! UDP datagrams on localhost, one datagram per radio frame, so this measures the software
//! path only. Airtime is modelled separately by the Python harness.
//!
//! Payload file (little endian), one record per payload:
//! `u32 offset_us | u8 flags (bit0 = starts a group) | u32 len | len bytes`.
//! Delivered file: `u32 id | u64 recv_unix_ns | u32 len | len bytes`.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use moq_lora_wire::seal::{Cipher, ObjectSealer, SALT_LEN};
use moq_lora_wire::{Event, Fragmenter, ObjHdr, Reassembler, OBJ_HDR_LEN};
use std::io::{BufWriter, Read, Write};
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Demo-only key. Never use a published key for anything real.
const DEMO_KEY_HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

#[derive(Parser)]
#[command(about = "moq-lora path for the old-stack comparison demo")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Send a payload file as moq-lora objects over UDP "air"
    Tx {
        #[arg(long)]
        payloads: PathBuf,
        #[arg(long)]
        air: String,
        /// JSON summary written at the end
        #[arg(long)]
        summary: PathBuf,
        /// Send-time log: one `id send_unix_ns` line per payload
        #[arg(long)]
        log: PathBuf,
        /// Drop this fraction of radio frames (seeded, independent per frame)
        #[arg(long, default_value_t = 0.0)]
        loss: f64,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Ignore the schedule and send as fast as possible
        #[arg(long)]
        unpaced: bool,
        #[arg(long, default_value_t = 8)]
        fec_k: u8,
        #[arg(long, default_value_t = 96)]
        min_frame: usize,
    },
    /// Receive moq-lora frames from UDP "air" and write delivered payloads
    Rx {
        #[arg(long)]
        air: String,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        summary: PathBuf,
        /// Created once the socket is bound
        #[arg(long)]
        ready: PathBuf,
        /// Exit after this long with no frames (once at least one arrived)
        #[arg(long, default_value_t = 1500)]
        idle_exit_ms: u64,
    },
    /// Answer every command object with an ack object (the far end of `ping`)
    Echo {
        #[arg(long)]
        listen: String,
        #[arg(long)]
        reply: String,
        #[arg(long)]
        ready: PathBuf,
        #[arg(long, default_value_t = 1500)]
        idle_exit_ms: u64,
    },
    /// Send commands one at a time and time the ack round trip
    Ping {
        #[arg(long)]
        to: String,
        #[arg(long)]
        listen: String,
        #[arg(long, default_value_t = 20)]
        count: u32,
        #[arg(long, default_value_t = 8)]
        size: usize,
        #[arg(long, default_value_t = 200)]
        interval_ms: u64,
        #[arg(long)]
        summary: PathBuf,
    },
}

fn unix_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// One payload from the shared workload file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payload {
    pub offset_us: u32,
    pub group_start: bool,
    pub data: Vec<u8>,
}

pub fn parse_payloads(b: &[u8]) -> Result<Vec<Payload>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b.len() - i < 9 {
            bail!("truncated record header at byte {i}");
        }
        let off = u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        let flags = b[i + 4];
        let len = u32::from_le_bytes(b[i + 5..i + 9].try_into().unwrap()) as usize;
        i += 9;
        if b.len() - i < len {
            bail!("truncated payload at byte {i}");
        }
        out.push(Payload {
            offset_us: off,
            group_start: flags & 1 != 0,
            data: b[i..i + len].to_vec(),
        });
        i += len;
    }
    Ok(out)
}

/// xorshift64*, the same generator the Python side uses for its loss draws.
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    pub fn next_f64(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn sealer() -> ObjectSealer {
    let key = moq_lora_wire::seal::parse_key_hex(DEMO_KEY_HEX).expect("demo key");
    ObjectSealer::new(Cipher::ChaCha20Poly1305, &key).expect("demo key length")
}

fn run_salt() -> [u8; SALT_LEN] {
    // Demo only: unique per run, not secret. The real sender draws a random salt.
    let mut s = [0u8; SALT_LEN];
    s[..8].copy_from_slice(&unix_ns().to_le_bytes());
    s[8..].copy_from_slice(&std::process::id().to_le_bytes());
    s
}

/// Turns payloads into sealed, fragmented radio frames.
pub struct Encoder {
    sealer: ObjectSealer,
    salt: [u8; SALT_LEN],
    frag: Fragmenter,
    group: u16,
    obj: u8,
    started: bool,
}

impl Encoder {
    pub fn new(fec_k: u8, min_frame: usize) -> Result<Self> {
        let mut frag = Fragmenter::new(fec_k).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        frag.min_frame = min_frame;
        Ok(Encoder {
            sealer: sealer(),
            salt: run_salt(),
            frag,
            group: 0,
            obj: 0,
            started: false,
        })
    }

    /// Frames for payload `id`. A new group starts when asked, or when a group is full.
    pub fn encode(&mut self, id: u32, group_start: bool, data: &[u8]) -> Result<Vec<Vec<u8>>> {
        if !self.started {
            self.started = true;
        } else if group_start || self.obj == u8::MAX {
            self.group = self.group.wrapping_add(1);
            self.obj = 0;
        } else {
            self.obj += 1;
        }
        let hdr = ObjHdr {
            frame_no: id,
            cap_us: unix_ns() / 1000,
            enc_us: 0,
            key: self.obj == 0,
        };
        let mut plain = Vec::with_capacity(OBJ_HDR_LEN + data.len());
        plain.extend_from_slice(&hdr.encode());
        plain.extend_from_slice(data);
        let sealed = self.sealer.seal(&self.salt, self.group, self.obj, &plain);
        self.frag
            .fragment(self.group, self.obj, &sealed)
            .map_err(|e| anyhow::anyhow!("{e:?}"))
    }
}

/// Reassembles frames and opens objects back into `(id, payload)`.
pub struct Decoder {
    re: Reassembler,
    sealer: ObjectSealer,
    pub lost_objects: u64,
    pub auth_failures: u64,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder {
            re: Reassembler::new(),
            sealer: sealer(),
            lost_objects: 0,
            auth_failures: 0,
        }
    }
}

impl Decoder {
    fn event(&mut self, e: Event, out: &mut Vec<(u32, Vec<u8>)>) {
        match e {
            Event::Object {
                group, obj, data, ..
            } => match self.sealer.open(group, obj, &data) {
                Some(plain) => match ObjHdr::decode(&plain) {
                    Some((h, body)) => out.push((h.frame_no, body.to_vec())),
                    None => self.auth_failures += 1,
                },
                None => self.auth_failures += 1,
            },
            Event::Lost { .. } => self.lost_objects += 1,
        }
    }
    pub fn push(&mut self, frame: &[u8]) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        if frame.len() < moq_lora_wire::HDR_LEN {
            return out;
        }
        for e in self.re.push(frame) {
            self.event(e, &mut out);
        }
        out
    }
    pub fn flush(&mut self) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        if let Some(e) = self.re.flush() {
            self.event(e, &mut out);
        }
        out
    }
    pub fn finish(&mut self) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        if let Some(e) = self.re.finish() {
            self.event(e, &mut out);
        }
        out
    }
    pub fn fec_recovered(&self) -> u64 {
        self.re.fec_recovered
    }
}

fn touch(p: &Path) -> Result<()> {
    std::fs::write(p, b"ok\n").with_context(|| format!("write {}", p.display()))
}

#[allow(clippy::too_many_arguments)]
fn tx(
    payloads: &Path,
    air: &str,
    summary: &Path,
    log: &Path,
    loss: f64,
    seed: u64,
    unpaced: bool,
    fec_k: u8,
    min_frame: usize,
) -> Result<()> {
    let mut raw = Vec::new();
    std::fs::File::open(payloads)?.read_to_end(&mut raw)?;
    let pls = parse_payloads(&raw)?;
    let sock = UdpSocket::bind("127.0.0.1:0")?;
    sock.connect(air)?;
    let mut enc = Encoder::new(fec_k, min_frame)?;
    let mut rng = Rng::new(seed);
    let mut logw = BufWriter::new(std::fs::File::create(log)?);
    let (mut frames, mut bytes, mut dropped) = (0u64, 0u64, 0u64);
    let t0 = Instant::now();
    for (id, p) in pls.iter().enumerate() {
        if !unpaced {
            let due = Duration::from_micros(u64::from(p.offset_us));
            let now = t0.elapsed();
            if due > now {
                std::thread::sleep(due - now);
            }
        }
        writeln!(logw, "{id} {}", unix_ns())?;
        for f in enc.encode(id as u32, p.group_start, &p.data)? {
            frames += 1;
            bytes += f.len() as u64;
            if loss > 0.0 && rng.next_f64() < loss {
                dropped += 1;
                continue;
            }
            // localhost UDP can briefly refuse when the receiver lags; retry, never drop
            while let Err(e) = sock.send(&f) {
                if e.kind() != std::io::ErrorKind::WouldBlock {
                    std::thread::sleep(Duration::from_micros(50));
                    if e.kind() == std::io::ErrorKind::ConnectionRefused {
                        continue;
                    }
                }
            }
        }
    }
    logw.flush()?;
    let wall = t0.elapsed().as_secs_f64();
    std::fs::write(
        summary,
        format!(
            "{{\"stack\":\"moq-lora\",\"payloads\":{},\"air_frames\":{frames},\"air_bytes\":{bytes},\"air_frames_dropped\":{dropped},\"send_wall_s\":{wall:.4},\"fec_k\":{fec_k},\"min_frame\":{min_frame},\"cipher\":\"chacha20poly1305\"}}\n",
            pls.len()
        ),
    )?;
    Ok(())
}

fn write_delivered(w: &mut impl Write, id: u32, t: u64, body: &[u8]) -> Result<()> {
    w.write_all(&id.to_le_bytes())?;
    w.write_all(&t.to_le_bytes())?;
    w.write_all(&(body.len() as u32).to_le_bytes())?;
    w.write_all(body)?;
    Ok(())
}

/// Shared receive loop: calls `on` for each delivered payload; returns (decoder, frames).
fn rx_loop(
    sock: &UdpSocket,
    idle_exit_ms: u64,
    mut on: impl FnMut(u32, Vec<u8>) -> Result<()>,
) -> Result<(Decoder, u64)> {
    sock.set_read_timeout(Some(Duration::from_millis(20)))?;
    let mut dec = Decoder::default();
    let mut buf = [0u8; 2048];
    let mut frames = 0u64;
    let mut last: Option<Instant> = None;
    let mut flushed = true;
    loop {
        match sock.recv(&mut buf) {
            Ok(n) => {
                frames += 1;
                last = Some(Instant::now());
                flushed = false;
                for (id, body) in dec.push(&buf[..n]) {
                    on(id, body)?;
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e.into()),
        }
        if let Some(l) = last {
            // same idle rule as moq-lora-recv: finalize the object in progress after 150 ms
            if !flushed && l.elapsed() > Duration::from_millis(150) {
                flushed = true;
                for (id, body) in dec.flush() {
                    on(id, body)?;
                }
            }
            if l.elapsed() > Duration::from_millis(idle_exit_ms) {
                break;
            }
        }
    }
    for (id, body) in dec.finish() {
        on(id, body)?;
    }
    Ok((dec, frames))
}

fn rx(air: &str, out: &Path, summary: &Path, ready: &Path, idle_exit_ms: u64) -> Result<()> {
    let sock = UdpSocket::bind(air)?;
    touch(ready)?;
    let mut w = BufWriter::new(std::fs::File::create(out)?);
    let mut delivered = 0u64;
    let (dec, frames) = rx_loop(&sock, idle_exit_ms, |id, body| {
        delivered += 1;
        write_delivered(&mut w, id, unix_ns(), &body)
    })?;
    w.flush()?;
    std::fs::write(
        summary,
        format!(
            "{{\"stack\":\"moq-lora\",\"frames_rx\":{frames},\"delivered\":{delivered},\"objects_lost\":{},\"auth_failures\":{},\"fec_recovered\":{}}}\n",
            dec.lost_objects, dec.auth_failures, dec.fec_recovered()
        ),
    )?;
    Ok(())
}

fn echo(listen: &str, reply: &str, ready: &Path, idle_exit_ms: u64) -> Result<()> {
    let sock = UdpSocket::bind(listen)?;
    let out = UdpSocket::bind("127.0.0.1:0")?;
    out.connect(reply)?;
    touch(ready)?;
    let mut enc = Encoder::new(0, 0)?;
    rx_loop(&sock, idle_exit_ms, |id, body| {
        // the ack carries the command id and echoes its first byte
        let ack = vec![0xAC, body.first().copied().unwrap_or(0)];
        for f in enc.encode(id, false, &ack)? {
            out.send(&f)?;
        }
        Ok(())
    })?;
    Ok(())
}

fn ping(
    to: &str,
    listen: &str,
    count: u32,
    size: usize,
    interval_ms: u64,
    summary: &Path,
) -> Result<()> {
    let sock = UdpSocket::bind(listen)?;
    sock.set_read_timeout(Some(Duration::from_millis(5)))?;
    let out = UdpSocket::bind("127.0.0.1:0")?;
    out.connect(to)?;
    let mut enc = Encoder::new(0, 0)?;
    let mut dec = Decoder::default();
    let mut buf = [0u8; 2048];
    let mut rtts = Vec::new();
    let mut timeouts = 0u32;
    for id in 0..count {
        let cmd = vec![id as u8; size.max(1)];
        let t = Instant::now();
        for f in enc.encode(id, false, &cmd)? {
            out.send(&f)?;
        }
        let mut got = false;
        while t.elapsed() < Duration::from_secs(3) && !got {
            let mut acks = match sock.recv(&mut buf) {
                Ok(n) => dec.push(&buf[..n]),
                Err(_) => Vec::new(),
            };
            // a one-frame reply object is released by the reassembler only on the next
            // frame or an idle flush; flush right away, the ack is always one frame
            acks.extend(dec.flush());
            got = acks.iter().any(|(aid, _)| *aid == id);
        }
        if got {
            rtts.push(t.elapsed().as_secs_f64() * 1000.0);
        } else {
            timeouts += 1;
        }
        std::thread::sleep(Duration::from_millis(interval_ms));
    }
    let list: Vec<String> = rtts.iter().map(|r| format!("{r:.4}")).collect();
    std::fs::write(
        summary,
        format!(
            "{{\"stack\":\"moq-lora\",\"count\":{count},\"timeouts\":{timeouts},\"rtt_ms\":[{}]}}\n",
            list.join(",")
        ),
    )?;
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Tx {
            payloads,
            air,
            summary,
            log,
            loss,
            seed,
            unpaced,
            fec_k,
            min_frame,
        } => tx(
            &payloads, &air, &summary, &log, loss, seed, unpaced, fec_k, min_frame,
        ),
        Cmd::Rx {
            air,
            out,
            summary,
            ready,
            idle_exit_ms,
        } => rx(&air, &out, &summary, &ready, idle_exit_ms),
        Cmd::Echo {
            listen,
            reply,
            ready,
            idle_exit_ms,
        } => echo(&listen, &reply, &ready, idle_exit_ms),
        Cmd::Ping {
            to,
            listen,
            count,
            size,
            interval_ms,
            summary,
        } => ping(&to, &listen, count, size, interval_ms, &summary),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(sizes: &[usize], fec_k: u8, drop_every: usize) -> (Vec<(u32, Vec<u8>)>, u64) {
        let mut enc = Encoder::new(fec_k, 96).unwrap();
        let mut dec = Decoder::default();
        let mut got = Vec::new();
        let mut n = 0usize;
        for (id, &s) in sizes.iter().enumerate() {
            let data: Vec<u8> = (0..s).map(|i| (i * 7 + id) as u8).collect();
            for f in enc.encode(id as u32, id % 15 == 0, &data).unwrap() {
                n += 1;
                if drop_every > 0 && n.is_multiple_of(drop_every) {
                    continue;
                }
                got.extend(dec.push(&f));
            }
        }
        got.extend(dec.finish());
        (got, dec.lost_objects)
    }

    fn expected(sizes: &[usize]) -> Vec<(u32, Vec<u8>)> {
        sizes
            .iter()
            .enumerate()
            .map(|(id, &s)| (id as u32, (0..s).map(|i| (i * 7 + id) as u8).collect()))
            .collect()
    }

    #[test]
    fn lossless_roundtrip_is_exact() {
        let sizes = [12, 4500, 1300, 1, 243, 244, 9000, 12];
        let (got, lost) = roundtrip(&sizes, 8, 0);
        assert_eq!(lost, 0);
        assert_eq!(got, expected(&sizes));
    }

    #[test]
    fn parity_repairs_one_loss_per_block() {
        // every 9th frame lost: with k = 8 that is at most one per FEC block
        let sizes = [4500, 1300, 1300, 4500, 1300];
        let (got, lost) = roundtrip(&sizes, 8, 9);
        assert_eq!(lost, 0);
        assert_eq!(got, expected(&sizes));
    }

    #[test]
    fn payload_file_parses() {
        let mut b = Vec::new();
        for (off, fl, d) in [(0u32, 1u8, &b"abc"[..]), (66_666, 0, &b""[..])] {
            b.extend_from_slice(&off.to_le_bytes());
            b.push(fl);
            b.extend_from_slice(&(d.len() as u32).to_le_bytes());
            b.extend_from_slice(d);
        }
        let p = parse_payloads(&b).unwrap();
        assert_eq!(p.len(), 2);
        assert!(p[0].group_start && !p[1].group_start);
        assert_eq!(p[0].data, b"abc");
        assert_eq!(p[1].offset_us, 66_666);
        assert!(parse_payloads(&b[..b.len() - 1]).is_err());
        assert!(parse_payloads(&b[..5]).is_err());
    }

    #[test]
    fn rng_matches_python_reference() {
        // first draws for seed 1, also asserted in tests/test_lorawan.py
        let mut r = Rng::new(1);
        assert_eq!(r.next_f64(), 0.28083505005035947);
        assert_eq!(r.next_f64(), 0.6711372530266764);
    }
}
