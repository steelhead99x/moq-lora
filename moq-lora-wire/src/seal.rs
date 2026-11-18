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
use rind::aead::{Key, Nonce, AES_128_GCM, AES_256_GCM, CHACHA20_POLY1305};

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
        Ok(ObjectSealer {
            key: Key::new(alg, key).map_err(|_| "bad key length")?,
            cipher,
        })
    }

    /// Encrypt `plain` for position (`group`, `obj`) in `session`.
    pub fn seal(&self, session: u32, group: u16, obj: u8, plain: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(plain.len() + OVERHEAD);
        out.extend_from_slice(&session.to_be_bytes());
        out.extend_from_slice(plain);
        let aad = [(group >> 8) as u8, group as u8, obj];
        let tag = self
            .key
            .seal_in_place_separate_tag(nonce(session, group, obj), &aad, &mut out[4..])
            .expect("seal");
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
        let n = self
            .key
            .open_in_place(nonce(session, group, obj), &aad, &mut buf)
            .ok()?
            .len();
        buf.truncate(n);
        Some(buf)
    }
}

/// Parse a hex key (whitespace ignored).
pub fn parse_key_hex(s: &str) -> Option<Vec<u8>> {
    // Sized up front so it never reallocates, and wiped before it is freed: this is
    // a copy of the key in hex.
    let mut h = String::with_capacity(s.len());
    h.extend(s.chars().filter(|c| !c.is_whitespace()));
    let key = if h.is_empty() {
        None
    } else {
        crate::decode_hex(&h)
    };
    wipe(&mut h.into_bytes());
    key
}

/// Session id to use after the 16-bit group counter wraps.
///
/// `draw` is a fresh random id. If it collides with the session that just ended,
/// every nonce of that session would be reused under the same key, so the id is
/// bumped. This does not remove the birthday bound against older sessions; rotate
/// the key long before 2^16 sessions.
pub fn session_after_wrap(prev: u32, draw: u32) -> u32 {
    if draw == prev {
        prev.wrapping_add(1)
    } else {
        draw
    }
}

#[derive(Debug)]
pub enum ReadKeyError {
    Io(std::io::Error),
    /// Mode bits other than owner read/write were set (for example 0644).
    Perms {
        mode: u32,
    },
    Hex,
    Len,
}

impl std::fmt::Display for ReadKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadKeyError::Io(e) => write!(f, "read key file: {e}"),
            ReadKeyError::Perms { mode } => {
                write!(f, "key file mode is {mode:o}; it must not be readable by group or others (use 0600)")
            }
            ReadKeyError::Hex => write!(f, "key file must be hex"),
            ReadKeyError::Len => write!(f, "bad key length"),
        }
    }
}

impl std::error::Error for ReadKeyError {}

/// Overwrite `buf` with zeros in a way the compiler won't optimize away.
pub(crate) fn wipe(buf: &mut [u8]) {
    for b in buf {
        // SAFETY: `b` points at a byte of `buf` for this write.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Read a pre-shared key. On Unix the file must be mode 0600 (or stricter:
/// no group/other permission bits). The hex text is wiped before return;
/// the returned bytes are the raw key and should be wiped by the caller
/// after they are copied into the AEAD key.
pub fn read_key_file(path: &std::path::Path) -> Result<Vec<u8>, ReadKeyError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path).map_err(ReadKeyError::Io)?;
        if !meta.is_file() {
            return Err(ReadKeyError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "key path is not a file",
            )));
        }
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(ReadKeyError::Perms { mode: mode & 0o777 });
        }
    }
    let mut raw = std::fs::read(path).map_err(ReadKeyError::Io)?;
    let text = match std::str::from_utf8(&raw) {
        Ok(t) => t,
        Err(_) => {
            wipe(&mut raw);
            return Err(ReadKeyError::Hex);
        }
    };
    let key = match parse_key_hex(text) {
        Some(k) => k,
        None => {
            wipe(&mut raw);
            return Err(ReadKeyError::Hex);
        }
    };
    wipe(&mut raw);
    Ok(key)
}

/// Load `path` and build a sealer. The hex file and the temporary key bytes
/// are wiped; rind keeps its own copy of the key.
pub fn open_sealer(path: &std::path::Path, cipher: Cipher) -> Result<ObjectSealer, ReadKeyError> {
    let mut key = read_key_file(path)?;
    let built = ObjectSealer::new(cipher, &key).map_err(|_| ReadKeyError::Len);
    wipe(&mut key);
    built
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_and_tamper() {
        for c in [
            Cipher::ChaCha20Poly1305,
            Cipher::Aes128Gcm,
            Cipher::Aes256Gcm,
        ] {
            let s = ObjectSealer::new(c, &vec![7u8; c.key_len()]).unwrap();
            let p = b"an access unit".to_vec();
            let x = s.seal(0xdead_beef, 300, 4, &p);
            assert_eq!(x.len(), p.len() + OVERHEAD);
            assert_eq!(s.open(300, 4, &x), Some(p.clone()));
            assert_eq!(s.open(300, 5, &x), None); // wrong position
            let mut t = x.clone();
            t[6] ^= 1;
            assert_eq!(s.open(300, 4, &t), None);
            // session id sits in the clear prefix and in the nonce
            let mut sess = x.clone();
            sess[0] ^= 0x01;
            assert_eq!(s.open(300, 4, &sess), None);
            assert_eq!(s.open(300, 4, &x[..x.len() - 1]), None);
            assert!(s.open(300, 4, &[]).is_none());
        }
    }

    #[test]
    fn hex_and_session_wrap() {
        assert_eq!(parse_key_hex("0a0b"), Some(vec![0x0a, 0x0b]));
        assert_eq!(parse_key_hex("0a 0b\n"), Some(vec![0x0a, 0x0b]));
        assert!(parse_key_hex("").is_none());
        assert!(parse_key_hex("abc").is_none());
        assert!(parse_key_hex("zz").is_none());
        assert!(parse_key_hex("a\u{e9}b").is_none()); // used to panic on a char boundary
        assert_eq!(session_after_wrap(7, 7), 8);
        assert_eq!(session_after_wrap(u32::MAX, u32::MAX), 0);
        assert_eq!(session_after_wrap(7, 9), 9);
    }

    #[cfg(unix)]
    #[test]
    fn key_file_mode_and_roundtrip() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("moq-lora-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        let loose = dir.join("loose.hex");
        let tight = dir.join("tight.hex");
        let hex: String = (0..32).map(|i| format!("{i:02x}")).collect();
        std::fs::write(&loose, &hex).unwrap();
        std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            read_key_file(&loose),
            Err(ReadKeyError::Perms { .. })
        ));
        std::fs::write(&tight, &hex).unwrap();
        std::fs::set_permissions(&tight, std::fs::Permissions::from_mode(0o600)).unwrap();
        let s = open_sealer(&tight, Cipher::ChaCha20Poly1305).unwrap();
        let ct = s.seal(1, 2, 3, b"frame");
        assert_eq!(s.open(2, 3, &ct).as_deref(), Some(&b"frame"[..]));
        std::fs::write(&tight, "not-hex\n").unwrap();
        std::fs::set_permissions(&tight, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(read_key_file(&tight), Err(ReadKeyError::Hex)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
