//! Per-object authenticated encryption with [rind](https://github.com/kixel-ai/rind).
//!
//! Each sender run draws a random 96-bit salt and encrypts under a subkey derived from the
//! pre-shared key and that salt (HKDF-SHA256, the salt in its info), so two runs under one
//! key never share a keystream (a collision needs two equal 96-bit salts). Sealed object:
//!
//! * object 0 of a group: `salt (12, clear) | AEAD(ObjHdr + access unit) | tag (16)`,
//!   [`OVERHEAD_FIRST`] = 28 bytes, about once a second;
//! * any other object: `salt[..4] (clear) | AEAD(..) | tag (16)`, [`OVERHEAD`] = 20 bytes.
//!
//! * nonce (12 B) = `salt[..4] | group (2) | obj (1) | 0 (5)`, unique per object within a
//!   salt; the sender draws a new salt whenever the 16-bit group id wraps.
//! * AAD = `group (2) | obj (1)`: an object cannot be replayed under another position.
//!
//! The receiver learns a salt from an object 0 and keeps the subkeys of the last two salts
//! that authenticated, so it can join mid-run at the next group. Lost radio frames are
//! handled below this layer (FEC / group gate); a frame that fails authentication is
//! dropped like a lost object.
use rind::aead::{Key, Nonce, AES_128_GCM, AES_256_GCM, CHACHA20_POLY1305};
use rind::hkdf::{Prk, HKDF_SHA256};

pub const TAG_LEN: usize = 16;
/// Length of a run's salt.
pub const SALT_LEN: usize = 12;
/// Bytes of the salt every object carries, to find its subkey.
pub const PREFIX_LEN: usize = 4;
/// What sealing adds to an object other than a group's first.
pub const OVERHEAD: usize = PREFIX_LEN + TAG_LEN;
/// What sealing adds to object 0 of a group, which carries the whole salt.
pub const OVERHEAD_FIRST: usize = SALT_LEN + TAG_LEN;
/// HKDF info label, so these subkeys are never confused with any other use of the key.
const INFO: &[u8] = b"moq-lora object key v1";

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

/// Seals and opens objects under per-run subkeys of one pre-shared key.
pub struct ObjectSealer {
    /// The pre-shared key as an HKDF pseudorandom key (it is already uniformly random, so
    /// HKDF-Extract is skipped); rind wipes it on drop.
    prk: Prk,
    pub cipher: Cipher,
    /// Subkeys of the last salts used or authenticated, newest first.
    keys: Vec<([u8; SALT_LEN], Key)>,
}

fn nonce(prefix: &[u8], group: u16, obj: u8) -> Nonce {
    let mut n = [0u8; 12];
    n[..PREFIX_LEN].copy_from_slice(prefix);
    n[4..6].copy_from_slice(&group.to_be_bytes());
    n[6] = obj;
    Nonce::assume_unique_for_key(n)
}

impl ObjectSealer {
    /// `key` is the pre-shared key, `cipher.key_len()` bytes.
    pub fn new(cipher: Cipher, key: &[u8]) -> Result<Self, &'static str> {
        if key.len() != cipher.key_len() {
            return Err("bad key length");
        }
        Ok(ObjectSealer {
            prk: Prk::new(&HKDF_SHA256, key),
            cipher,
            keys: Vec::with_capacity(2),
        })
    }

    fn derive(&self, salt: &[u8; SALT_LEN]) -> Key {
        let alg = match self.cipher {
            Cipher::ChaCha20Poly1305 => &CHACHA20_POLY1305,
            Cipher::Aes128Gcm => &AES_128_GCM,
            Cipher::Aes256Gcm => &AES_256_GCM,
        };
        let mut k = [0u8; 32];
        let k = &mut k[..self.cipher.key_len()];
        self.prk
            .fill(&[INFO, self.cipher.name().as_bytes(), salt], k)
            .expect("hkdf length");
        let key = Key::new(alg, k).expect("subkey length");
        wipe(k);
        key
    }

    fn remember(&mut self, salt: [u8; SALT_LEN], key: Key) {
        self.keys.retain(|(s, _)| *s != salt);
        self.keys.insert(0, (salt, key));
        self.keys.truncate(2);
    }

    /// Encrypt `plain` for position (`group`, `obj`) under the run's `salt`.
    pub fn seal(&mut self, salt: &[u8; SALT_LEN], group: u16, obj: u8, plain: &[u8]) -> Vec<u8> {
        if self.keys.first().is_none_or(|(s, _)| s != salt) {
            let key = self.derive(salt);
            self.remember(*salt, key);
        }
        let clear: &[u8] = if obj == 0 { salt } else { &salt[..PREFIX_LEN] };
        let mut out = Vec::with_capacity(clear.len() + plain.len() + TAG_LEN);
        out.extend_from_slice(clear);
        out.extend_from_slice(plain);
        let aad = [(group >> 8) as u8, group as u8, obj];
        let tag = self.keys[0]
            .1
            .seal_in_place_separate_tag(
                nonce(&salt[..PREFIX_LEN], group, obj),
                &aad,
                &mut out[clear.len()..],
            )
            .expect("seal");
        out.extend_from_slice(tag.as_ref());
        out
    }

    /// Authenticate and decrypt; `None` if the object was forged or corrupted, or its salt
    /// is unknown (no object 0 of its run has authenticated yet).
    pub fn open(&mut self, group: u16, obj: u8, sealed: &[u8]) -> Option<Vec<u8>> {
        let aad = [(group >> 8) as u8, group as u8, obj];
        if obj == 0 {
            if sealed.len() < OVERHEAD_FIRST {
                return None;
            }
            let salt: [u8; SALT_LEN] = sealed[..SALT_LEN].try_into().unwrap();
            // A salt is cached only once an object under it authenticates, so a forged
            // object 0 can't push the real subkey out. A replayed object 0 from an older
            // run does authenticate; two of them evict the live salt until its next
            // object 0. Replay is out of scope (REVIEW.md, "Replayed frames").
            let fresh = !self.keys.iter().any(|(s, _)| *s == salt);
            let derived = fresh.then(|| self.derive(&salt));
            let key = match &derived {
                Some(k) => k,
                None => &self.keys.iter().find(|(s, _)| *s == salt)?.1,
            };
            let mut buf = sealed[SALT_LEN..].to_vec();
            let n = key
                .open_in_place(nonce(&salt[..PREFIX_LEN], group, obj), &aad, &mut buf)
                .ok()?
                .len();
            buf.truncate(n);
            if let Some(k) = derived {
                self.remember(salt, k);
            }
            Some(buf)
        } else {
            if sealed.len() < OVERHEAD {
                return None;
            }
            let prefix = &sealed[..PREFIX_LEN];
            // Two cached salts can share a prefix by chance, so try each match.
            self.keys
                .iter()
                .filter(|(s, _)| &s[..PREFIX_LEN] == prefix)
                .find_map(|(_, key)| {
                    let mut buf = sealed[PREFIX_LEN..].to_vec();
                    let n = key
                        .open_in_place(nonce(prefix, group, obj), &aad, &mut buf)
                        .ok()?
                        .len();
                    buf.truncate(n);
                    Some(buf)
                })
        }
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

/// Longest key file accepted. A 32-byte key is 64 hex characters; the rest is room
/// for whitespace, not for reading an arbitrary file.
pub const MAX_KEY_FILE: usize = 1024;

/// Read a pre-shared key. On Unix the file must be mode 0600 (or stricter:
/// no group/other permission bits), checked on the open file, and at most
/// [`MAX_KEY_FILE`] bytes. The hex text is wiped before return;
/// the returned bytes are the raw key and should be wiped by the caller
/// after they are copied into the AEAD key.
pub fn read_key_file(path: &std::path::Path) -> Result<Vec<u8>, ReadKeyError> {
    use std::io::Read;
    // One handle for the check and the read, so the file cannot be swapped for a
    // looser one in between. Opened non-blocking so a FIFO path fails the type check
    // below instead of blocking until a writer appears.
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK);
    }
    let f = opts.open(path).map_err(ReadKeyError::Io)?;
    let meta = f.metadata().map_err(ReadKeyError::Io)?;
    if !meta.is_file() {
        return Err(ReadKeyError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "key path is not a file",
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(ReadKeyError::Perms { mode: mode & 0o777 });
        }
    }
    // Read into a buffer sized up front and capped: a growing Vec would leave
    // copies of the hex key in freed memory, and nothing bounds the file otherwise.
    let mut raw = Vec::with_capacity(MAX_KEY_FILE + 1);
    if let Err(e) = f.take(MAX_KEY_FILE as u64 + 1).read_to_end(&mut raw) {
        wipe(&mut raw);
        return Err(ReadKeyError::Io(e));
    }
    if raw.len() > MAX_KEY_FILE {
        wipe(&mut raw);
        return Err(ReadKeyError::Len);
    }
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
/// are wiped; rind keeps its own copy of the key, as an HKDF key.
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
            let key = vec![7u8; c.key_len()];
            let (mut tx, mut rx) = (
                ObjectSealer::new(c, &key).unwrap(),
                ObjectSealer::new(c, &key).unwrap(),
            );
            let salt = [9u8; SALT_LEN];
            let p = b"an access unit".to_vec();
            let first = tx.seal(&salt, 300, 0, &p);
            let x = tx.seal(&salt, 300, 4, &p);
            assert_eq!(
                (first.len(), x.len()),
                (p.len() + OVERHEAD_FIRST, p.len() + OVERHEAD)
            );
            // the receiver can't open a later object until it has seen the group's object 0
            assert_eq!(rx.open(300, 4, &x), None);
            assert_eq!(rx.open(300, 0, &first), Some(p.clone()));
            assert_eq!(rx.open(300, 4, &x), Some(p.clone()));
            assert_eq!(rx.open(300, 5, &x), None); // wrong position
            let mut t = x.clone();
            t[6] ^= 1;
            assert_eq!(rx.open(300, 4, &t), None);
            let mut pre = x.clone();
            pre[0] ^= 0x01; // the prefix picks the subkey and is in the nonce
            assert_eq!(rx.open(300, 4, &pre), None);
            assert_eq!(rx.open(300, 4, &x[..x.len() - 1]), None);
            assert!(rx.open(300, 4, &[]).is_none());
            assert!(rx.open(300, 0, &first[..OVERHEAD_FIRST - 1]).is_none());
        }
    }

    #[test]
    fn runs_with_different_salts_use_different_keystreams() {
        let key = [3u8; 32];
        let mut a = ObjectSealer::new(Cipher::ChaCha20Poly1305, &key).unwrap();
        let p = [0u8; 64];
        // same key, position and session prefix; only the salt differs
        let (s1, mut s2) = ([1u8; SALT_LEN], [1u8; SALT_LEN]);
        s2[11] = 2;
        let (x1, x2) = (a.seal(&s1, 5, 1, &p), a.seal(&s2, 5, 1, &p));
        assert_eq!(x1[..PREFIX_LEN], x2[..PREFIX_LEN]);
        assert_ne!(
            x1[PREFIX_LEN..PREFIX_LEN + 64],
            x2[PREFIX_LEN..PREFIX_LEN + 64]
        );
    }

    #[test]
    fn a_forged_object_zero_does_not_evict_the_real_salt() {
        let key = [3u8; 32];
        let mut tx = ObjectSealer::new(Cipher::ChaCha20Poly1305, &key).unwrap();
        let mut rx = ObjectSealer::new(Cipher::ChaCha20Poly1305, &key).unwrap();
        let salt = [4u8; SALT_LEN];
        assert!(rx.open(1, 0, &tx.seal(&salt, 1, 0, b"idr")).is_some());
        for i in 0..5u8 {
            let mut forged = vec![i; OVERHEAD_FIRST + 8]; // random salts, no valid tag
            forged[0] = 0xee;
            assert!(rx.open(2, 0, &forged).is_none());
        }
        assert_eq!(
            rx.open(1, 1, &tx.seal(&salt, 1, 1, b"p")).as_deref(),
            Some(&b"p"[..])
        );
        // a wrong-length key is refused
        assert!(ObjectSealer::new(Cipher::Aes128Gcm, &key).is_err());
    }

    #[test]
    fn the_previous_salt_still_opens_after_a_switch() {
        // late objects of the old run still decrypt after the sender redraws its salt
        let key = [5u8; 32];
        let mut tx = ObjectSealer::new(Cipher::Aes256Gcm, &key).unwrap();
        let mut rx = ObjectSealer::new(Cipher::Aes256Gcm, &key).unwrap();
        let (a, b, c) = ([1u8; SALT_LEN], [2u8; SALT_LEN], [3u8; SALT_LEN]);
        let late_a = tx.seal(&a, 9, 3, b"late");
        assert!(rx.open(9, 0, &tx.seal(&a, 9, 0, b"a0")).is_some());
        assert!(rx.open(0, 0, &tx.seal(&b, 0, 0, b"b0")).is_some());
        assert_eq!(rx.open(9, 3, &late_a).as_deref(), Some(&b"late"[..]));
        // a third salt pushes out the oldest
        assert!(rx.open(0, 0, &tx.seal(&c, 0, 0, b"c0")).is_some());
        assert!(rx.open(9, 3, &late_a).is_none());
        assert!(rx.open(0, 1, &tx.seal(&b, 0, 1, b"b1")).is_some());
    }

    #[test]
    fn salts_sharing_a_prefix_both_open() {
        let key = [6u8; 32];
        let mut tx = ObjectSealer::new(Cipher::ChaCha20Poly1305, &key).unwrap();
        let mut rx = ObjectSealer::new(Cipher::ChaCha20Poly1305, &key).unwrap();
        let a = [7u8; SALT_LEN];
        let mut b = a;
        b[SALT_LEN - 1] = 8; // same first 4 bytes
        assert!(rx.open(1, 0, &tx.seal(&a, 1, 0, b"a0")).is_some());
        assert!(rx.open(2, 0, &tx.seal(&b, 2, 0, b"b0")).is_some());
        assert_eq!(
            rx.open(1, 1, &tx.seal(&a, 1, 1, b"a1")).as_deref(),
            Some(&b"a1"[..])
        );
        assert_eq!(
            rx.open(2, 1, &tx.seal(&b, 2, 1, b"b1")).as_deref(),
            Some(&b"b1"[..])
        );
    }

    #[test]
    fn hex_keys() {
        assert_eq!(parse_key_hex("0a0b"), Some(vec![0x0a, 0x0b]));
        assert_eq!(parse_key_hex("0a 0b\n"), Some(vec![0x0a, 0x0b]));
        assert!(parse_key_hex("").is_none());
        assert!(parse_key_hex("abc").is_none());
        assert!(parse_key_hex("zz").is_none());
        assert!(parse_key_hex("a\u{e9}b").is_none()); // used to panic on a char boundary
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
        let mut s = open_sealer(&tight, Cipher::ChaCha20Poly1305).unwrap();
        let ct = s.seal(&[1; SALT_LEN], 2, 3, b"frame");
        assert_eq!(s.open(2, 3, &ct).as_deref(), Some(&b"frame"[..]));
        std::fs::write(&tight, "not-hex\n").unwrap();
        std::fs::set_permissions(&tight, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(read_key_file(&tight), Err(ReadKeyError::Hex)));
        // bounded: a huge file is refused, not read whole
        std::fs::write(&tight, "a".repeat(MAX_KEY_FILE + 1)).unwrap();
        assert!(matches!(read_key_file(&tight), Err(ReadKeyError::Len)));
        std::fs::write(&tight, format!("{hex}\n")).unwrap();
        assert!(read_key_file(&tight).is_ok());
        // a directory is refused through the same handle
        assert!(matches!(read_key_file(&dir), Err(ReadKeyError::Io(_))));
        // a FIFO is refused at once, not waited on for a writer
        let fifo = dir.join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(matches!(read_key_file(&fifo), Err(ReadKeyError::Io(_))));
        // CRLF line endings are whitespace
        std::fs::write(&tight, format!("{}\r\n{}\r\n", &hex[..32], &hex[32..])).unwrap();
        assert!(read_key_file(&tight).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
