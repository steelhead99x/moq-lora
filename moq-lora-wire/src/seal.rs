//! Per-object authenticated encryption with [rind](https://github.com/kixel-ai/rind).
//!
//! Sealed object = `session u32 (clear) | AEAD(ObjHdr + access unit) | tag (16)`, so the
//! overhead is [`OVERHEAD`] = 20 bytes per object (one video frame), independent of size.
//!
//! * nonce (12 B) = `session (4) | group (2) | obj (1) | 0 (5)`
//! * AAD = `group (2) | obj (1)`: an object cannot be replayed under another position.
//! * `session` is random per sender run and re-drawn whenever the 16-bit group id wraps,
//!   so a (key, nonce) pair is never reused unless two sessions draw the same 32-bit id
//!   (birthday bound ~2^16 sessions per key: rotate the key long before that).
//!
//! Lost radio frames are handled below this layer (FEC / group gate); a frame that fails
//! authentication is dropped like a lost object.
use rind::aead::{Key, Nonce, CHACHA20_POLY1305, AES_128_GCM, AES_256_GCM};

pub const TAG_LEN: usize = 16;
pub const OVERHEAD: usize = 4 + TAG_LEN;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cipher {
    ChaCha20Poly1305,
    Aes128Gcm,
    Aes256Gcm,
}

impl Cipher {
    pub fn parse(s: &str) -> Option<Cipher> {
        match s {
            "chacha20poly1305" | "chacha" => Some(Cipher::ChaCha20Poly1305),
            "aes128gcm" | "aes128" => Some(Cipher::Aes128Gcm),
            "aes256gcm" | "aes256" => Some(Cipher::Aes256Gcm),
            _ => None,
        }
    }
    pub fn key_len(self) -> usize {
        match self {
            Cipher::Aes128Gcm => 16,
            _ => 32,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Cipher::ChaCha20Poly1305 => "chacha20poly1305",
            Cipher::Aes128Gcm => "aes128gcm",
            Cipher::Aes256Gcm => "aes256gcm",
        }
    }
}

pub struct ObjectSealer {
    key: Key,
    pub cipher: Cipher,
}

fn nonce(session: u32, group: u16, obj: u8) -> Nonce {
    let mut n = [0u8; 12];
    n[..4].copy_from_slice(&session.to_be_bytes());
    n[4..6].copy_from_slice(&group.to_be_bytes());
    n[6] = obj;
    Nonce::assume_unique_for_key(n)
}

impl ObjectSealer {
    pub fn new(cipher: Cipher, key: &[u8]) -> Result<Self, &'static str> {
        let alg = match cipher {
            Cipher::ChaCha20Poly1305 => &CHACHA20_POLY1305,
            Cipher::Aes128Gcm => &AES_128_GCM,
            Cipher::Aes256Gcm => &AES_256_GCM,
        };
        Ok(ObjectSealer { key: Key::new(alg, key).map_err(|_| "bad key length")?, cipher })
    }

    /// Encrypt `plain` for position (`group`, `obj`) in `session`.
    pub fn seal(&self, session: u32, group: u16, obj: u8, plain: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(plain.len() + OVERHEAD);
        out.extend_from_slice(&session.to_be_bytes());
        out.extend_from_slice(plain);
        let aad = [(group >> 8) as u8, group as u8, obj];
        let tag = self.key.seal_in_place_separate_tag(nonce(session, group, obj), &aad, &mut out[4..]).expect("seal");
        out.extend_from_slice(tag.as_ref());
        out
    }

    /// Authenticate and decrypt; `None` if the object was forged or corrupted.
    pub fn open(&self, group: u16, obj: u8, sealed: &[u8]) -> Option<Vec<u8>> {
        if sealed.len() < OVERHEAD {
            return None;
        }
        let session = u32::from_be_bytes(sealed[..4].try_into().unwrap());
        let mut buf = sealed[4..].to_vec();
        let aad = [(group >> 8) as u8, group as u8, obj];
        let n = self.key.open_in_place(nonce(session, group, obj), &aad, &mut buf).ok()?.len();
        buf.truncate(n);
        Some(buf)
    }
}

/// Parse a hex key (whitespace ignored).
pub fn parse_key_hex(s: &str) -> Option<Vec<u8>> {
    let h: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if h.len() % 2 != 0 {
        return None;
    }
    (0..h.len() / 2).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_and_tamper() {
        for c in [Cipher::ChaCha20Poly1305, Cipher::Aes128Gcm, Cipher::Aes256Gcm] {
            let s = ObjectSealer::new(c, &vec![7u8; c.key_len()]).unwrap();
            let p = b"an access unit".to_vec();
            let x = s.seal(0xdead_beef, 300, 4, &p);
            assert_eq!(x.len(), p.len() + OVERHEAD);
            assert_eq!(s.open(300, 4, &x), Some(p.clone()));
            assert_eq!(s.open(300, 5, &x), None); // wrong position
            let mut t = x.clone();
            t[6] ^= 1;
            assert_eq!(s.open(300, 4, &t), None);
        }
    }
}
