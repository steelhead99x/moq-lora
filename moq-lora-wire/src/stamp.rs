//! In-picture timestamps on a YUV420p / gray frame:
//! * a 40-bit barcode in the bottom 16 rows (2 rows x 20 blocks; 32-bit value + 8-bit check)
//!   that survives low-bitrate H.264 and is read back after decoding (glass-to-glass latency);
//! * a human-readable clock / counter in a 3x5 pixel font.

pub const BAR_ROWS: usize = 16;

fn bits(val: u32) -> [bool; 40] {
    let b = val.to_be_bytes();
    let chk = b[0] ^ b[1] ^ b[2] ^ b[3] ^ 0xA5;
    let v = (val as u64) << 8 | chk as u64;
    let mut out = [false; 40];
    for (i, o) in out.iter_mut().enumerate() {
        *o = (v >> (39 - i)) & 1 == 1;
    }
    out
}

/// Draw `val` into the luma plane (`y` = w*h bytes) and neutralize chroma if given.
pub fn write_barcode(frame: &mut [u8], w: usize, h: usize, val: u32) {
    let bw = w / 20;
    for (i, &bit) in bits(val).iter().enumerate() {
        let (row, col) = (i / 20, i % 20);
        let v = if bit { 235 } else { 16 };
        for yy in h - BAR_ROWS + row * 8..h - BAR_ROWS + row * 8 + 8 {
            frame[yy * w + col * bw..yy * w + col * bw + bw].fill(v);
        }
    }
    if frame.len() >= w * h * 3 / 2 {
        let (cw, ch) = (w / 2, h / 2);
        for plane in 0..2 {
            let base = w * h + plane * cw * ch;
            for yy in ch - BAR_ROWS / 2..ch {
                frame[base + yy * cw..base + yy * cw + cw].fill(128);
            }
        }
    }
}

/// Read the barcode back from a decoded luma plane.
pub fn read_barcode(y: &[u8], w: usize, h: usize) -> Option<u32> {
    let bw = w / 20;
    let mut v: u64 = 0;
    for i in 0..40 {
        let (row, col) = (i / 20, i % 20);
        let mut sum = 0u32;
        let mut n = 0u32;
        for yy in h - BAR_ROWS + row * 8 + 2..h - BAR_ROWS + row * 8 + 6 {
            for xx in col * bw + bw / 4..col * bw + bw * 3 / 4 {
                sum += y[yy * w + xx] as u32;
                n += 1;
            }
        }
        v = v << 1 | (sum / n > 125) as u64;
    }
    let val = (v >> 8) as u32;
    (bits(val) == bits_of(v)).then_some(val)
}

fn bits_of(v: u64) -> [bool; 40] {
    let mut out = [false; 40];
    for (i, o) in out.iter_mut().enumerate() {
        *o = (v >> (39 - i)) & 1 == 1;
    }
    out
}

const FONT: &[(char, [u8; 5])] = &[
    ('0', [7, 5, 5, 5, 7]), ('1', [2, 6, 2, 2, 7]), ('2', [7, 1, 7, 4, 7]), ('3', [7, 1, 7, 1, 7]),
    ('4', [5, 5, 7, 1, 1]), ('5', [7, 4, 7, 1, 7]), ('6', [7, 4, 7, 5, 7]), ('7', [7, 1, 1, 1, 1]),
    ('8', [7, 5, 7, 5, 7]), ('9', [7, 5, 7, 1, 7]), (':', [0, 2, 0, 2, 0]), ('.', [0, 0, 0, 0, 2]),
    ('-', [0, 0, 7, 0, 0]), ('#', [5, 7, 5, 7, 5]), ('P', [7, 5, 7, 4, 4]), ('S', [7, 4, 7, 1, 7]),
    ('T', [7, 2, 2, 2, 2]), ('k', [4, 5, 6, 5, 5]), ('F', [7, 4, 6, 4, 4]), ('K', [5, 5, 6, 5, 5]),
    (' ', [0, 0, 0, 0, 0]),
];

/// White text on a black box into the luma plane, `s` = pixel scale.
pub fn draw_text(y: &mut [u8], w: usize, x0: usize, y0: usize, text: &str, s: usize) {
    let cw = 4 * s;
    let bw = text.chars().count() * cw + s;
    for yy in y0..y0 + 7 * s {
        for xx in x0..(x0 + bw).min(w) {
            y[yy * w + xx] = 16;
        }
    }
    for (ci, c) in text.chars().enumerate() {
        let g = FONT.iter().find(|f| f.0 == c).map(|f| f.1).unwrap_or([0; 5]);
        for (r, row) in g.iter().enumerate() {
            for col in 0..3 {
                if row >> (2 - col) & 1 == 1 {
                    for dy in 0..s {
                        for dx in 0..s {
                            let (px, py) = (x0 + s + ci * cw + col * s + dx, y0 + s + r * s + dy);
                            if px < w {
                                y[py * w + px] = 235;
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn barcode_roundtrip() {
        let (w, h) = (320, 240);
        let mut f = vec![90u8; w * h * 3 / 2];
        draw_text(&mut f, w, 4, 4, "12:34:56.789", 4);
        for v in [0u32, 1, 0xdead_beef, 123_456_789] {
            write_barcode(&mut f, w, h, v);
            assert_eq!(read_barcode(&f, w, h), Some(v));
        }
        f[(h - 4) * w + 10] ^= 0; // unchanged
        // corrupt one block -> check fails
        write_barcode(&mut f, w, h, 42);
        for yy in h - 8..h {
            for xx in 0..16 {
                f[yy * w + xx] = 235 - f[yy * w + xx] + 16;
            }
        }
        assert_eq!(read_barcode(&f, w, h), None);
    }
}
