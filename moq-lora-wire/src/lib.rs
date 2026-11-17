//! Wire format for streaming live video over a <=255-byte-frame radio link (SX1276 FSK).
//!
//! The model follows MoQ (Media over QUIC Transport): a **track** is a sequence of
//! **groups**, a group starts at a keyframe (one H.264 GOP) and holds **objects**
//! (one encoded access unit / video frame each). Objects are split into radio frames
//! with a 12-byte header; optional XOR parity adds one repair frame per `k` data frames.
//! A receiver that loses an object it cannot repair drops the rest of that group and
//! resumes at the next group, exactly like a MoQ subscriber joining at a group boundary.
//!
//! Radio frame (big endian):
//! ```text
//!  0..2  seq     u16  link frame counter (loss accounting)
//!  2..4  group   u16  group id (GOP)
//!  4     obj     u8   object id within the group (0 = keyframe)
//!  5     idx     u8   data fragment index, or parity block index if PARITY
//!  6     nfrag   u8   number of data fragments in the object
//!  7     flags   u8   bit0 PARITY, bits1..3 version (2), bits4..7 k (FEC block, 0 = none)
//!  8..10 objlen  u16  object length in bytes
//! 10..12 crc     u16  CRC-16/CCITT-FALSE over the whole frame with this field zeroed
//! 12..   payload     <= 243 bytes; an object is cut into `nfrag = ceil(len/243)` equal-size
//!                    chunks, frames may be zero-padded up to a minimum length (see below)
//! ```
//! Object payload: 17-byte [`ObjHdr`] followed by the H.264 Annex-B access unit.

pub mod annexb;
pub mod stamp;
pub mod seal;

pub const MAX_FRAME: usize = 255;
pub const HDR_LEN: usize = 12;
pub const MAX_CHUNK: usize = MAX_FRAME - HDR_LEN;
pub const VERSION: u8 = 3;
pub const MAX_OBJECT: usize = 255 * MAX_CHUNK; // 61965 B (fits u16)

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHdr {
    pub seq: u16,
    pub group: u16,
    pub obj: u8,
    pub idx: u8,
    pub nfrag: u8,
    pub parity: bool,
    pub k: u8,
    pub objlen: u16,
}

/// CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF).
pub fn crc16(data: &[u8]) -> u16 {
    let mut c: u16 = 0xFFFF;
    for &b in data {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x1021 } else { c << 1 };
        }
    }
    c
}

/// End-to-end frame check. The radio's own CRC is not enough: when a packet fails CRC
/// the SX1276 silently clears its FIFO, and a polled reader that already drained part of
/// it can splice the start of the bad packet onto the next good one.
pub fn seal(frame: &mut [u8]) {
    frame[10] = 0;
    frame[11] = 0;
    let c = crc16(frame);
    frame[10..12].copy_from_slice(&c.to_be_bytes());
}

impl FrameHdr {
    /// Header with a zero CRC field; call [`seal`] once the payload is appended.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.group.to_be_bytes());
        out.extend_from_slice(&[self.obj, self.idx, self.nfrag, (self.parity as u8) | (VERSION << 1) | (self.k << 4)]);
        out.extend_from_slice(&self.objlen.to_be_bytes());
        out.extend_from_slice(&[0, 0]);
    }

    /// Parse and sanity-check a radio frame. `None` for anything that is not ours.
    pub fn decode(b: &[u8]) -> Option<(FrameHdr, &[u8])> {
        if b.len() < HDR_LEN + 1 || (b[7] >> 1) & 7 != VERSION {
            return None;
        }
        let mut z = b.to_vec();
        z[10] = 0;
        z[11] = 0;
        if crc16(&z) != u16::from_be_bytes([b[10], b[11]]) {
            return None;
        }
        let h = FrameHdr {
            seq: u16::from_be_bytes([b[0], b[1]]),
            group: u16::from_be_bytes([b[2], b[3]]),
            obj: b[4],
            idx: b[5],
            nfrag: b[6],
            parity: b[7] & 1 != 0,
            k: b[7] >> 4,
            objlen: u16::from_be_bytes([b[8], b[9]]),
        };
        if h.nfrag == 0 || (h.objlen as usize).div_ceil(MAX_CHUNK) != h.nfrag as usize {
            return None;
        }
        if h.parity {
            if h.k == 0 || h.idx as usize >= (h.nfrag as usize).div_ceil(h.k as usize) {
                return None;
            }
        } else if h.idx >= h.nfrag {
            return None;
        }
        Some((h, &b[HDR_LEN..]))
    }

    /// Data chunks are equal-sized (`ceil(objlen / nfrag)`), so no fragment is much shorter
    /// than the others: a very short frame right behind a long one is the frame most likely
    /// to be missed by a receiver that is still busy with the long one.
    pub fn chunk_size(&self) -> usize {
        (self.objlen as usize).div_ceil(self.nfrag as usize)
    }
    pub fn chunk_len(&self, idx: usize) -> usize {
        let cs = self.chunk_size();
        (self.objlen as usize - idx * cs).min(cs)
    }
}

/// Per-object header carried at the start of every object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjHdr {
    /// Source frame number (counts every captured frame).
    pub frame_no: u32,
    /// Capture time, sender wall clock, unix microseconds.
    pub cap_us: u64,
    /// Encoder delay (encoder output time - capture time), microseconds.
    pub enc_us: u32,
    pub key: bool,
}
pub const OBJ_HDR_LEN: usize = 17;

impl ObjHdr {
    pub fn encode(&self) -> [u8; OBJ_HDR_LEN] {
        let mut b = [0u8; OBJ_HDR_LEN];
        b[0..4].copy_from_slice(&self.frame_no.to_be_bytes());
        b[4..12].copy_from_slice(&self.cap_us.to_be_bytes());
        b[12..16].copy_from_slice(&self.enc_us.to_be_bytes());
        b[16] = self.key as u8;
        b
    }
    pub fn decode(b: &[u8]) -> Option<(ObjHdr, &[u8])> {
        if b.len() < OBJ_HDR_LEN {
            return None;
        }
        Some((
            ObjHdr {
                frame_no: u32::from_be_bytes(b[0..4].try_into().unwrap()),
                cap_us: u64::from_be_bytes(b[4..12].try_into().unwrap()),
                enc_us: u32::from_be_bytes(b[12..16].try_into().unwrap()),
                key: b[16] != 0,
            },
            &b[OBJ_HDR_LEN..],
        ))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum WireError {
    Empty,
    TooBig(usize),
    BadK,
}

/// Splits objects into radio frames (+ XOR parity).
pub struct Fragmenter {
    pub seq: u16,
    /// FEC block size: one parity frame after every `k` data frames (0 = no FEC, max 15).
    pub k: u8,
    /// Zero-pad shorter frames to this many bytes (receivers truncate by `objlen`).
    pub min_frame: usize,
}

impl Fragmenter {
    pub fn new(k: u8) -> Result<Self, WireError> {
        if k > 15 {
            return Err(WireError::BadK);
        }
        Ok(Fragmenter { seq: 0, k, min_frame: 0 })
    }

    pub fn fragment(&mut self, group: u16, obj: u8, data: &[u8]) -> Result<Vec<Vec<u8>>, WireError> {
        if data.is_empty() {
            return Err(WireError::Empty);
        }
        if data.len() > MAX_OBJECT {
            return Err(WireError::TooBig(data.len()));
        }
        let cs = data.len().div_ceil(data.len().div_ceil(MAX_CHUNK));
        let chunks: Vec<&[u8]> = data.chunks(cs).collect();
        let nfrag = chunks.len() as u8;
        let blk = if self.k == 0 { chunks.len() } else { self.k as usize };
        let mut out = Vec::with_capacity(chunks.len() + chunks.len().div_ceil(blk));
        for (b, block) in chunks.chunks(blk).enumerate() {
            for (j, c) in block.iter().enumerate() {
                out.push(self.frame(group, obj, (b * blk + j) as u8, nfrag, false, data.len(), c));
            }
            if self.k > 0 {
                let mut p = vec![0u8; block[0].len()];
                for c in block {
                    for (x, y) in p.iter_mut().zip(c.iter()) {
                        *x ^= y;
                    }
                }
                out.push(self.frame(group, obj, b as u8, nfrag, true, data.len(), &p));
            }
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn frame(&mut self, group: u16, obj: u8, idx: u8, nfrag: u8, parity: bool, objlen: usize, pl: &[u8]) -> Vec<u8> {
        let mut f = Vec::with_capacity(HDR_LEN + pl.len());
        FrameHdr { seq: self.seq, group, obj, idx, nfrag, parity, k: self.k, objlen: objlen as u16 }.encode(&mut f);
        self.seq = self.seq.wrapping_add(1);
        f.extend_from_slice(pl);
        if f.len() < self.min_frame.min(MAX_FRAME) {
            f.resize(self.min_frame.min(MAX_FRAME), 0);
        }
        seal(&mut f);
        f
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// Complete object (`recovered` = data fragments rebuilt from parity).
    Object { group: u16, obj: u8, data: Vec<u8>, recovered: u32 },
    /// Object abandoned: `have` of `need` data fragments.
    Lost { group: u16, obj: u8, have: u32, need: u32 },
}

struct Partial {
    h: FrameHdr,
    chunks: Vec<Option<Vec<u8>>>,
    parity: Vec<Option<Vec<u8>>>,
    recovered: u32,
}

impl Partial {
    fn new(h: FrameHdr) -> Self {
        let nb = if h.k == 0 { 0 } else { (h.nfrag as usize).div_ceil(h.k as usize) };
        Partial { h, chunks: vec![None; h.nfrag as usize], parity: vec![None; nb], recovered: 0 }
    }
    fn try_recover(&mut self) {
        let k = self.h.k as usize;
        for b in 0..self.parity.len() {
            let Some(p) = &self.parity[b] else { continue };
            let range = b * k..((b + 1) * k).min(self.chunks.len());
            let missing: Vec<usize> = range.clone().filter(|&i| self.chunks[i].is_none()).collect();
            if missing.len() != 1 {
                continue;
            }
            let m = missing[0];
            let want = self.h.chunk_len(m);
            if p.len() < want {
                continue;
            }
            let mut r = p.clone();
            for i in range.filter(|&i| i != m) {
                for (x, y) in r.iter_mut().zip(self.chunks[i].as_ref().unwrap()) {
                    *x ^= y;
                }
            }
            r.truncate(want);
            self.chunks[m] = Some(r);
            self.recovered += 1;
        }
    }
    fn have(&self) -> u32 {
        self.chunks.iter().filter(|c| c.is_some()).count() as u32
    }
    fn complete(&self) -> bool {
        self.chunks.iter().all(|c| c.is_some())
    }
    fn into_event(mut self) -> Event {
        self.try_recover();
        if self.complete() {
            Event::Object { group: self.h.group, obj: self.h.obj, recovered: self.recovered, data: self.chunks.into_iter().flatten().flatten().collect() }
        } else {
            Event::Lost { group: self.h.group, obj: self.h.obj, have: self.have(), need: self.h.nfrag as u32 }
        }
    }
}

/// Rebuilds objects from radio frames. Objects arrive one after another from a single
/// sender, so a frame from a new object finalizes the previous one.
#[derive(Default)]
pub struct Reassembler {
    cur: Option<Partial>,
    done: Option<(u16, u8)>,
    last_seq: Option<u16>,
    pub frames_ok: u64,
    pub frames_missing: u64,
    pub frames_bad: u64,
    pub frames_dup: u64,
    pub fec_recovered: u64,
}

impl Reassembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, frame: &[u8]) -> Vec<Event> {
        let mut ev = Vec::new();
        let Some((h, pl)) = FrameHdr::decode(frame) else {
            self.frames_bad += 1;
            return ev;
        };
        if let Some(l) = self.last_seq {
            let d = h.seq.wrapping_sub(l);
            if d == 0 {
                self.frames_dup += 1;
                return ev;
            }
            if d < 0x8000 {
                self.frames_missing += d as u64 - 1;
            }
        }
        self.last_seq = Some(h.seq);
        self.frames_ok += 1;
        let key = (h.group, h.obj);
        if self.done == Some(key) {
            return ev; // trailing parity of an object already delivered
        }
        if self.cur.as_ref().is_some_and(|p| (p.h.group, p.h.obj) != key) {
            ev.push(self.cur.take().unwrap().into_event());
        }
        let p = self.cur.get_or_insert_with(|| Partial::new(h));
        if p.h.objlen != h.objlen || p.h.k != h.k {
            self.frames_bad += 1;
            return ev;
        }
        if h.parity {
            let n = pl.len().min(h.chunk_size());
            p.parity[h.idx as usize].get_or_insert_with(|| pl[..n].to_vec());
        } else {
            let n = h.chunk_len(h.idx as usize);
            if pl.len() < n {
                self.frames_bad += 1;
                return ev;
            }
            p.chunks[h.idx as usize].get_or_insert_with(|| pl[..n].to_vec());
        }
        p.try_recover();
        if p.complete() {
            let p = self.cur.take().unwrap();
            self.done = Some(key);
            self.fec_recovered += p.recovered as u64;
            ev.push(p.into_event());
        }
        ev
    }

    /// Finalize the object in progress (call when the link has been idle a while).
    pub fn flush(&mut self) -> Option<Event> {
        let p = self.cur.take()?;
        self.done = Some((p.h.group, p.h.obj));
        let e = p.into_event();
        if let Event::Object { recovered, .. } = &e {
            self.fec_recovered += *recovered as u64;
        }
        Some(e)
    }
}

/// MoQ-style delivery gate: passes objects in order, and after a lost object (or a gap in
/// object ids) drops the rest of that group until the next group's object 0.
#[derive(Default)]
pub struct GroupGate {
    next: Option<(u16, u8)>,
    broken: Option<u16>,
    pub groups_cut: u64,
    pub objects_skipped: u64,
}

impl GroupGate {
    /// Returns true when the object should be handed to the decoder.
    pub fn accept(&mut self, group: u16, obj: u8) -> bool {
        let ok = obj == 0 || (self.next == Some((group, obj)) && self.broken != Some(group));
        if !ok {
            if self.broken != Some(group) {
                self.groups_cut += 1;
            }
            self.broken = Some(group);
            self.objects_skipped += 1;
        } else if obj == 0 {
            self.broken = None;
        }
        self.next = Some((group, obj.wrapping_add(1)));
        ok
    }
    /// Report an unrecoverable object.
    pub fn lost(&mut self, group: u16, obj: u8) {
        if self.broken != Some(group) {
            self.groups_cut += 1;
        }
        self.broken = Some(group);
        self.next = Some((group, obj.wrapping_add(1)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(n: usize, seed: u8) -> Vec<u8> {
        (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
    }

    #[test]
    fn roundtrip_no_fec() {
        let mut f = Fragmenter::new(0).unwrap();
        let mut r = Reassembler::new();
        let mut got = vec![];
        for (o, n) in [1usize, 242, 243, 244, 5000, MAX_OBJECT].iter().enumerate() {
            let d = obj(*n, o as u8);
            for fr in f.fragment(3, o as u8, &d).unwrap() {
                assert!(fr.len() <= MAX_FRAME);
                got.extend(r.push(&fr));
            }
            assert_eq!(got.pop(), Some(Event::Object { group: 3, obj: o as u8, data: d, recovered: 0 }));
        }
        assert_eq!(r.frames_missing, 0);
    }

    #[test]
    fn fec_repairs_one_per_block() {
        for k in [1u8, 2, 4, 8] {
            for n in [10usize, 243, 700, 2000, 7000] {
                let d = obj(n, k);
                let mut f = Fragmenter::new(k).unwrap();
                let frames = f.fragment(9, 4, &d).unwrap();
                // drop the first data frame of every block (incl. short last chunks)
                let nfrag = n.div_ceil(MAX_CHUNK);
                let mut r = Reassembler::new();
                let mut ev = vec![];
                for fr in &frames {
                    let (h, _) = FrameHdr::decode(fr).unwrap();
                    if !h.parity && h.idx as usize % k as usize == 0 {
                        continue;
                    }
                    ev.extend(r.push(fr));
                }
                ev.extend(r.flush());
                match &ev[0] {
                    Event::Object { data, recovered, .. } => {
                        assert_eq!(data, &d, "k={k} n={n}");
                        assert_eq!(*recovered as usize, nfrag.div_ceil(k as usize));
                    }
                    e => panic!("k={k} n={n}: {e:?}"),
                }
            }
        }
    }

    #[test]
    fn fec_repairs_short_last_chunk() {
        let d = obj(MAX_CHUNK * 3 + 17, 1);
        let mut f = Fragmenter::new(4).unwrap();
        let mut r = Reassembler::new();
        let mut ev = vec![];
        for fr in f.fragment(0, 0, &d).unwrap() {
            let (h, _) = FrameHdr::decode(&fr).unwrap();
            if !h.parity && h.idx == 3 {
                continue;
            }
            ev.extend(r.push(&fr));
        }
        assert_eq!(ev, vec![Event::Object { group: 0, obj: 0, data: d, recovered: 1 }]);
    }

    #[test]
    fn loss_reported_and_seq_gap() {
        let mut f = Fragmenter::new(0).unwrap();
        let mut r = Reassembler::new();
        let a = f.fragment(1, 0, &obj(1000, 0)).unwrap();
        let b = f.fragment(1, 1, &obj(300, 1)).unwrap();
        let mut ev = vec![];
        for (i, fr) in a.iter().enumerate() {
            if i != 2 {
                ev.extend(r.push(fr));
            }
        }
        for fr in &b {
            ev.extend(r.push(fr));
        }
        assert_eq!(ev[0], Event::Lost { group: 1, obj: 0, have: 4, need: 5 });
        assert!(matches!(ev[1], Event::Object { obj: 1, .. }));
        assert_eq!(r.frames_missing, 1);
    }

    #[test]
    fn equal_chunks_and_padding() {
        let mut f = Fragmenter::new(2).unwrap();
        f.min_frame = 96;
        let mut r = Reassembler::new();
        for (o, n) in [1usize, 30, 246, 1000, 1100].iter().enumerate() {
            let d = obj(*n, 7);
            let frames = f.fragment(1, o as u8, &d).unwrap();
            let lens: Vec<usize> = frames.iter().map(|x| x.len()).collect();
            assert!(lens.iter().all(|&l| l >= 96 && l <= MAX_FRAME), "{lens:?}");
            assert!(lens.iter().max().unwrap() - lens.iter().filter(|&&l| l > 96).min().unwrap_or(&96) <= 1 || *n < 96, "{lens:?}");
            let mut ev = vec![];
            for (i, fr) in frames.iter().enumerate() {
                if i != 0 {
                    ev.extend(r.push(fr));
                }
            }
            ev.extend(r.flush());
            assert_eq!(ev.last(), Some(&Event::Object { group: 1, obj: o as u8, data: d, recovered: 1 }), "n={n}");
        }
    }

    #[test]
    fn crc_rejects_corruption() {
        let mut f = Fragmenter::new(0).unwrap();
        let fr = f.fragment(0, 0, &obj(300, 3)).unwrap();
        assert!(FrameHdr::decode(&fr[0]).is_some());
        let mut bad = fr[0].clone();
        bad[100] ^= 0x10;
        assert!(FrameHdr::decode(&bad).is_none());
        assert_eq!(crc16(b"123456789"), 0x29B1);
    }

    #[test]
    fn group_gate() {
        let mut g = GroupGate::default();
        assert!(g.accept(0, 0));
        assert!(g.accept(0, 1));
        g.lost(0, 2);
        assert!(!g.accept(0, 3));
        assert!(!g.accept(0, 4));
        assert!(g.accept(1, 0));
        assert!(g.accept(1, 1));
        assert!(!g.accept(1, 3)); // gap
        assert!(g.accept(5, 0)); // groups dropped at the sender
        assert_eq!(g.groups_cut, 2);
    }

    #[test]
    fn obj_hdr_roundtrip() {
        let h = ObjHdr { frame_no: 77, cap_us: 1_794_900_000_123_456, enc_us: 33_000, key: true };
        let mut b = h.encode().to_vec();
        b.extend_from_slice(b"xyz");
        assert_eq!(ObjHdr::decode(&b), Some((h, &b"xyz"[..])));
    }
}
