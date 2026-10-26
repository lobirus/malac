//! A small QR code encoder for pairing links: byte mode, error correction
//! level M, any version, mask chosen by the standard penalty rules. It
//! follows the structure of Project Nayuki's qrcodegen. Codes render as
//! Unicode half blocks for terminals and as SVG for the web UI.

/// Error correction codewords per block, for level M, by version (index 0 unused).
const ECC_PER_BLOCK: [usize; 41] = [
    0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28,
    28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
];
/// Error correction blocks, for level M, by version.
const BLOCKS: [usize; 41] = [
    0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23,
    25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49,
];
/// Level M in the format information.
const FORMAT_M: u32 = 0;
/// Pure black and white in the 256-color palette.
pub const BLACK: u8 = 16;
pub const WHITE: u8 = 231;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrCode {
    size: usize,
    /// Row-major; `true` is dark.
    modules: Vec<bool>,
}

impl QrCode {
    /// Encodes `text` in the smallest version that holds it; `None` when it
    /// is longer than a QR code can carry.
    pub fn encode(text: &str) -> Option<Self> {
        let data = text.as_bytes();
        let version = (1..=40).find(|&v| {
            let count_bits = if v <= 9 { 8 } else { 16 };
            data.len() < (1 << count_bits) && 4 + count_bits + data.len() * 8 <= data_bits(v)
        })?;
        let mut builder = Builder::new(version);
        builder.draw_function_patterns();
        let codewords = add_ecc_and_interleave(version, &data_codewords(version, data));
        builder.draw_codewords(&codewords);
        let mask = (0..8)
            .min_by_key(|&mask| {
                builder.apply_mask(mask);
                builder.draw_format_bits(mask);
                let penalty = builder.penalty();
                builder.apply_mask(mask);
                penalty
            })
            .unwrap_or(0);
        builder.apply_mask(mask);
        builder.draw_format_bits(mask);
        Some(Self {
            size: builder.size,
            modules: builder.modules,
        })
    }

    /// Whether the module at column `x`, row `y` is dark; outside is light.
    pub fn dark(&self, x: isize, y: isize) -> bool {
        let size = self.size as isize;
        (0..size).contains(&x) && (0..size).contains(&y) && self.modules[(y * size + x) as usize]
    }

    /// Pairs of module rows for half-block rendering, with a light border of
    /// `quiet` modules: each cell is (upper dark, lower dark).
    pub fn half_rows(&self, quiet: usize) -> Vec<Vec<(bool, bool)>> {
        let quiet = quiet as isize;
        let side = self.size as isize + 2 * quiet;
        (0..side)
            .step_by(2)
            .map(|y| {
                (0..side)
                    .map(|x| {
                        (
                            self.dark(x - quiet, y - quiet),
                            self.dark(x - quiet, y + 1 - quiet),
                        )
                    })
                    .collect()
            })
            .collect()
    }

    /// Lines of text for a terminal: `▀` in black and white from the
    /// 256-color cube (16 and 231), which themes do not change, so the code
    /// reads the same on dark and light backgrounds.
    pub fn ansi(&self) -> Vec<String> {
        self.half_rows(2)
            .into_iter()
            .map(|row| {
                let mut line = String::new();
                for (upper, lower) in row {
                    let fg = if upper { BLACK } else { WHITE };
                    let bg = if lower { BLACK } else { WHITE };
                    line.push_str(&format!("\x1b[38;5;{fg};48;5;{bg}m▀"));
                }
                line.push_str("\x1b[0m");
                line
            })
            .collect()
    }

    /// An SVG image with a four-module quiet zone, one unit per module.
    pub fn svg(&self) -> String {
        let side = self.size + 8;
        let mut path = String::new();
        for y in 0..self.size {
            for x in 0..self.size {
                if self.dark(x as isize, y as isize) {
                    path.push_str(&format!("M{},{}h1v1h-1z", x + 4, y + 4));
                }
            }
        }
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {side} {side}" shape-rendering="crispEdges"><rect width="{side}" height="{side}" fill="#fff"/><path d="{path}" fill="#000"/></svg>"##
        )
    }
}

/// Modules available for data and error correction in a version.
fn raw_data_modules(version: usize) -> usize {
    let mut result = (16 * version + 128) * version + 64;
    if version >= 2 {
        let aligns = version / 7 + 2;
        result -= (25 * aligns - 10) * aligns - 55;
        if version >= 7 {
            result -= 36;
        }
    }
    result
}

fn data_bits(version: usize) -> usize {
    (raw_data_modules(version) / 8 - ECC_PER_BLOCK[version] * BLOCKS[version]) * 8
}

/// The byte-mode segment, terminator and padding as codewords.
fn data_codewords(version: usize, data: &[u8]) -> Vec<u8> {
    let capacity = data_bits(version);
    let mut bits = Bits::default();
    bits.push(0b0100, 4);
    bits.push(data.len() as u32, if version <= 9 { 8 } else { 16 });
    for &byte in data {
        bits.push(u32::from(byte), 8);
    }
    let terminator = (capacity - bits.len).min(4);
    bits.push(0, terminator);
    bits.push(0, (8 - bits.len % 8) % 8);
    let mut bytes = bits.bytes;
    for pad in [0xEC, 0x11].into_iter().cycle() {
        if bytes.len() * 8 >= capacity {
            break;
        }
        bytes.push(pad);
    }
    bytes
}

#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    len: usize,
}

impl Bits {
    fn push(&mut self, value: u32, count: usize) {
        for i in (0..count).rev() {
            if self.len % 8 == 0 {
                self.bytes.push(0);
            }
            if (value >> i) & 1 == 1 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 0x80 >> (self.len % 8);
            }
            self.len += 1;
        }
    }
}

/// Splits the data into blocks, appends each block's error correction and
/// interleaves them.
fn add_ecc_and_interleave(version: usize, data: &[u8]) -> Vec<u8> {
    let blocks = BLOCKS[version];
    let ecc_len = ECC_PER_BLOCK[version];
    let raw = raw_data_modules(version) / 8;
    let short_blocks = blocks - raw % blocks;
    let short_len = raw / blocks;
    let divisor = rs_divisor(ecc_len);
    let mut split = Vec::with_capacity(blocks);
    let mut at = 0;
    for i in 0..blocks {
        let len = short_len - ecc_len + usize::from(i >= short_blocks);
        let mut block = data[at..at + len].to_vec();
        at += len;
        let ecc = rs_remainder(&block, &divisor);
        if i < short_blocks {
            block.push(0);
        }
        block.extend(ecc);
        split.push(block);
    }
    let mut result = Vec::with_capacity(raw);
    for i in 0..split[0].len() {
        for (j, block) in split.iter().enumerate() {
            // Short blocks have a placeholder where long ones have data.
            if i != short_len - ecc_len || j >= short_blocks {
                result.push(block[i]);
            }
        }
    }
    result
}

fn rs_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = gf_multiply(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = gf_multiply(root, 0x02);
    }
    result
}

fn rs_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for &byte in data {
        let factor = byte ^ result.remove(0);
        result.push(0);
        for (r, &d) in result.iter_mut().zip(divisor) {
            *r ^= gf_multiply(d, factor);
        }
    }
    result
}

/// Multiplication in GF(2^8) modulo x^8 + x^4 + x^3 + x^2 + 1.
fn gf_multiply(x: u8, y: u8) -> u8 {
    let mut z = 0u8;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x1D);
        z ^= ((y >> i) & 1) * x;
    }
    z
}

struct Builder {
    version: usize,
    size: usize,
    modules: Vec<bool>,
    function: Vec<bool>,
}

impl Builder {
    fn new(version: usize) -> Self {
        let size = version * 4 + 17;
        Self {
            version,
            size,
            modules: vec![false; size * size],
            function: vec![false; size * size],
        }
    }

    fn get(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }

    fn set_function(&mut self, x: usize, y: usize, dark: bool) {
        self.modules[y * self.size + x] = dark;
        self.function[y * self.size + x] = true;
    }

    fn draw_function_patterns(&mut self) {
        let size = self.size;
        for i in 0..size {
            self.set_function(6, i, i % 2 == 0);
            self.set_function(i, 6, i % 2 == 0);
        }
        self.draw_finder(3, 3);
        self.draw_finder(size - 4, 3);
        self.draw_finder(3, size - 4);
        let positions = self.alignment_positions();
        let last = positions.len().saturating_sub(1);
        for (i, &x) in positions.iter().enumerate() {
            for (j, &y) in positions.iter().enumerate() {
                // The finder patterns sit in three of the corners.
                if ![(0, 0), (0, last), (last, 0)].contains(&(i, j)) {
                    self.draw_alignment(x, y);
                }
            }
        }
        // Reserve the format areas; the real bits come with the mask.
        self.draw_format_bits(0);
        self.draw_version();
    }

    fn draw_finder(&mut self, x: usize, y: usize) {
        for dy in -4isize..=4 {
            for dx in -4isize..=4 {
                let distance = dx.abs().max(dy.abs());
                let (xx, yy) = (x as isize + dx, y as isize + dy);
                if (0..self.size as isize).contains(&xx) && (0..self.size as isize).contains(&yy) {
                    self.set_function(xx as usize, yy as usize, distance != 2 && distance != 4);
                }
            }
        }
    }

    fn draw_alignment(&mut self, x: usize, y: usize) {
        for dy in -2isize..=2 {
            for dx in -2isize..=2 {
                let dark = dx.abs().max(dy.abs()) != 1;
                self.set_function((x as isize + dx) as usize, (y as isize + dy) as usize, dark);
            }
        }
    }

    fn alignment_positions(&self) -> Vec<usize> {
        if self.version == 1 {
            return Vec::new();
        }
        let aligns = self.version / 7 + 2;
        let step = (self.version * 8 + aligns * 3 + 5) / (aligns * 4 - 4) * 2;
        let mut result: Vec<usize> = (0..aligns - 1).map(|i| self.size - 7 - i * step).collect();
        result.push(6);
        result.reverse();
        result
    }

    fn draw_format_bits(&mut self, mask: u32) {
        let data = (FORMAT_M << 3) | mask;
        let mut rem = data;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let bits = ((data << 10) | rem) ^ 0x5412;
        let bit = |i: usize| (bits >> i) & 1 == 1;
        for i in 0..=5 {
            self.set_function(8, i, bit(i));
        }
        self.set_function(8, 7, bit(6));
        self.set_function(8, 8, bit(7));
        self.set_function(7, 8, bit(8));
        for i in 9..15 {
            self.set_function(14 - i, 8, bit(i));
        }
        let size = self.size;
        for i in 0..8 {
            self.set_function(size - 1 - i, 8, bit(i));
        }
        for i in 8..15 {
            self.set_function(8, size - 15 + i, bit(i));
        }
        self.set_function(8, size - 8, true);
    }

    fn draw_version(&mut self) {
        if self.version < 7 {
            return;
        }
        let mut rem = self.version as u32;
        for _ in 0..12 {
            rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
        }
        let bits = ((self.version as u32) << 12) | rem;
        for i in 0..18 {
            let dark = (bits >> i) & 1 == 1;
            let a = self.size - 11 + i % 3;
            let b = i / 3;
            self.set_function(a, b, dark);
            self.set_function(b, a, dark);
        }
    }

    /// Places the codewords in the zigzag order, skipping function modules.
    fn draw_codewords(&mut self, data: &[u8]) {
        let size = self.size as isize;
        let mut i = 0;
        let mut right = size - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            for vertical in 0..size {
                for j in 0..2 {
                    let x = (right - j) as usize;
                    let upward = (right + 1) & 2 == 0;
                    let y = if upward {
                        size - 1 - vertical
                    } else {
                        vertical
                    } as usize;
                    if !self.function[y * self.size + x] && i < data.len() * 8 {
                        self.modules[y * self.size + x] = (data[i >> 3] >> (7 - (i & 7))) & 1 == 1;
                        i += 1;
                    }
                }
            }
            right -= 2;
        }
    }

    /// XORs a mask pattern into the data modules; applying it twice undoes it.
    fn apply_mask(&mut self, mask: u32) {
        for y in 0..self.size {
            for x in 0..self.size {
                let invert = match mask {
                    0 => (x + y) % 2 == 0,
                    1 => y % 2 == 0,
                    2 => x % 3 == 0,
                    3 => (x + y) % 3 == 0,
                    4 => (x / 3 + y / 2) % 2 == 0,
                    5 => x * y % 2 + x * y % 3 == 0,
                    6 => (x * y % 2 + x * y % 3) % 2 == 0,
                    _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
                };
                let at = y * self.size + x;
                if invert && !self.function[at] {
                    self.modules[at] = !self.modules[at];
                }
            }
        }
    }

    /// The standard penalty: long runs, 2x2 blocks, finder-like patterns
    /// and imbalance between dark and light.
    fn penalty(&self) -> i32 {
        let size = self.size;
        let mut result = 0;
        for vertical in [false, true] {
            for a in 0..size {
                let mut color = false;
                let mut run = 0;
                let mut history = Runs::new(size as i32);
                for b in 0..size {
                    let dark = if vertical {
                        self.get(a, b)
                    } else {
                        self.get(b, a)
                    };
                    if dark == color {
                        run += 1;
                        result += match run {
                            5 => 3,
                            6.. => 1,
                            _ => 0,
                        };
                    } else {
                        history.add(run);
                        if !color {
                            result += history.finder_like() * 40;
                        }
                        color = dark;
                        run = 1;
                    }
                }
                result += history.finish(color, run) * 40;
            }
        }
        for y in 0..size - 1 {
            for x in 0..size - 1 {
                let c = self.get(x, y);
                if c == self.get(x + 1, y) && c == self.get(x, y + 1) && c == self.get(x + 1, y + 1)
                {
                    result += 3;
                }
            }
        }
        let dark = self.modules.iter().filter(|m| **m).count() as i32;
        let total = (size * size) as i32;
        let k = ((dark * 20 - total * 10).abs() + total - 1) / total - 1;
        result + k * 10
    }
}

/// The last seven run lengths of a row or column, to spot 1:1:3:1:1 patterns.
struct Runs {
    size: i32,
    history: [i32; 7],
}

impl Runs {
    fn new(size: i32) -> Self {
        Self {
            size,
            history: [0; 7],
        }
    }

    fn add(&mut self, mut run: i32) {
        if self.history[0] == 0 {
            // The light border before the first run.
            run += self.size;
        }
        self.history.copy_within(0..6, 1);
        self.history[0] = run;
    }

    fn finder_like(&self) -> i32 {
        let h = &self.history;
        let n = h[1];
        let core = n > 0 && h[2] == n && h[3] == n * 3 && h[4] == n && h[5] == n;
        i32::from(core && h[0] >= n * 4 && h[6] >= n)
            + i32::from(core && h[6] >= n * 4 && h[0] >= n)
    }

    fn finish(mut self, color: bool, mut run: i32) -> i32 {
        if color {
            self.add(run);
            run = 0;
        }
        self.add(run + self.size);
        self.finder_like()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_smallest_version() {
        assert_eq!(QrCode::encode("hello").unwrap().size, 21);
        let link = "http://192.168.1.20:7700/#pair=ABCDE-FGHJK";
        assert_eq!(QrCode::encode(link).unwrap().size, 29, "version 3");
        assert!(QrCode::encode(&"x".repeat(2400)).is_none());
    }

    #[test]
    fn data_codewords_fill_the_capacity() {
        for version in [1, 5, 10, 27] {
            let words = data_codewords(version, b"malac");
            assert_eq!(words.len() * 8, data_bits(version));
            let header: &[u8] = if version <= 9 {
                &[0x40, 0x56]
            } else {
                &[0x40, 0x00]
            };
            assert_eq!(&words[..2], header, "mode 0100, then the length");
            let all = add_ecc_and_interleave(version, &words);
            assert_eq!(all.len(), raw_data_modules(version) / 8);
        }
    }

    #[test]
    fn reed_solomon_matches_the_standard_example() {
        // ISO 18004 annex: "01234567" in version 1-M.
        let data = [
            0x10, 0x20, 0x0C, 0x56, 0x61, 0x80, 0xEC, 0x11, 0xEC, 0x11, 0xEC, 0x11, 0xEC, 0x11,
            0xEC, 0x11,
        ];
        let ecc = rs_remainder(&data, &rs_divisor(10));
        assert_eq!(
            ecc,
            [0xA5, 0x24, 0xD4, 0xC1, 0xED, 0x36, 0xC7, 0x87, 0x2C, 0x55]
        );
    }

    #[test]
    fn finder_patterns_sit_in_three_corners() {
        let code = QrCode::encode("finder").unwrap();
        let last = code.size as isize - 1;
        for (x, y) in [(0, 0), (last, 0), (0, last)] {
            assert!(code.dark(x, y));
        }
        assert!(!code.dark(7, 7), "separator");
        assert!(code.dark(8, last - 7), "the dark module");
        assert!(!code.dark(-1, 0) && !code.dark(0, last + 1));
    }

    #[test]
    fn renders_half_blocks_and_svg() {
        let code = QrCode::encode("render").unwrap();
        let rows = code.half_rows(2);
        assert_eq!(rows.len(), 13, "25 rows of modules, two per line");
        assert_eq!(rows[0].len(), 25);
        assert_eq!(rows[1][2], (true, true), "the finder's corner");
        assert_eq!(code.ansi().len(), rows.len());
        let svg = code.svg();
        assert!(svg.starts_with("<svg") && svg.contains("viewBox=\"0 0 29 29\""));
    }
}
