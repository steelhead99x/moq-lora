//! Object records: how `moq-lora-recv` hands the objects it delivers to another process,
//! such as `moq-lora-publish`, over a pipe.
//!
//! One record per delivered object, in delivery order:
//!
//! ```text
//! "ML" ver=1 | group u16 | obj u8 | flags u8 (bit 0 = keyframe) | frame_no u32 | cap_us u64 | len u32 | payload
//! ```
//!
//! All integers are big-endian. `payload` is the H.264 access unit (Annex-B), without the
//! object header that travelled over the radio.

use std::io::{self, Read};

const MAGIC: [u8; 2] = *b"ML";
const VERSION: u8 = 1;
/// Bytes before the payload.
pub const RECORD_HDR_LEN: usize = 23;

/// One delivered object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// moq-lora group id (one per GOP); wraps at 65536.
    pub group: u16,
    /// Object id inside the group (one per frame); object 0 is the keyframe.
    pub obj: u8,
    pub key: bool,
    /// Source frame number.
    pub frame_no: u32,
    /// Capture time, sender wall clock, unix microseconds.
    pub cap_us: u64,
    pub payload: Vec<u8>,
}

impl Record {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(RECORD_HDR_LEN + self.payload.len());
        b.extend_from_slice(&MAGIC);
        b.push(VERSION);
        b.extend_from_slice(&self.group.to_be_bytes());
        b.push(self.obj);
        b.push(self.key as u8);
        b.extend_from_slice(&self.frame_no.to_be_bytes());
        b.extend_from_slice(&self.cap_us.to_be_bytes());
        b.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        b.extend_from_slice(&self.payload);
        b
    }
}

/// Read the next record. `Ok(None)` at a clean end of stream (no partial record).
/// A bad magic, version or a payload over [`crate::MAX_OBJECT`] is `InvalidData`.
pub fn read_record(r: &mut impl Read) -> io::Result<Option<Record>> {
    let mut h = [0u8; RECORD_HDR_LEN];
    let mut got = 0;
    while got < h.len() {
        match r.read(&mut h[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    if h[0..2] != MAGIC {
        return Err(bad("not an object record"));
    }
    if h[2] != VERSION {
        return Err(bad("unknown object record version"));
    }
    let len = u32::from_be_bytes(h[19..23].try_into().unwrap()) as usize;
    if len > crate::MAX_OBJECT {
        return Err(bad("object record too large"));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok(Some(Record {
        group: u16::from_be_bytes([h[3], h[4]]),
        obj: h[5],
        key: h[6] & 1 == 1,
        frame_no: u32::from_be_bytes(h[7..11].try_into().unwrap()),
        cap_us: u64::from_be_bytes(h[11..19].try_into().unwrap()),
        payload,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(group: u16, obj: u8, n: usize) -> Record {
        Record {
            group,
            obj,
            key: obj == 0,
            frame_no: 1000 + obj as u32,
            cap_us: 1_700_000_000_000_000 + obj as u64,
            payload: (0..n).map(|i| i as u8).collect(),
        }
    }

    #[test]
    fn roundtrip_a_stream() {
        let recs = [rec(65535, 0, 0), rec(65535, 1, 300), rec(0, 0, 61965)];
        let bytes: Vec<u8> = recs.iter().flat_map(|r| r.encode()).collect();
        let mut rd = &bytes[..];
        for r in &recs {
            assert_eq!(read_record(&mut rd).unwrap().as_ref(), Some(r));
        }
        assert_eq!(read_record(&mut rd).unwrap(), None);
    }

    #[test]
    fn rejects_truncated_bad_magic_and_oversize() {
        let b = rec(1, 2, 10).encode();
        assert_eq!(
            read_record(&mut &b[..5]).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(
            read_record(&mut &b[..b.len() - 1]).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        let mut m = b.clone();
        m[0] = b'X';
        assert_eq!(
            read_record(&mut &m[..]).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let mut big = b.clone();
        big[19..23].copy_from_slice(&((crate::MAX_OBJECT + 1) as u32).to_be_bytes());
        assert_eq!(
            read_record(&mut &big[..]).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
