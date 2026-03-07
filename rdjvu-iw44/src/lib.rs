use rdjvu_core::{Error, Pixmap};
use rdjvu_zp::ZPDecoder;

// Band-to-bucket mapping: (from, to) inclusive
const BAND_BUCKETS: [(usize, usize); 10] = [
    (0, 0),
    (1, 1),
    (2, 2),
    (3, 3),
    (4, 7),
    (8, 11),
    (12, 15),
    (16, 31),
    (32, 47),
    (48, 63),
];

const QUANT_LO_INIT: [u32; 16] = [
    0x004000, 0x008000, 0x008000, 0x010000, 0x010000,
    0x010000, 0x010000, 0x010000, 0x010000, 0x010000,
    0x010000, 0x010000, 0x020000, 0x020000, 0x020000, 0x020000,
];

const QUANT_HI_INIT: [u32; 10] = [
    0, 0x020000, 0x020000, 0x040000, 0x040000,
    0x040000, 0x080000, 0x040000, 0x040000, 0x080000,
];

// Coefficient state flags
const ZERO: u8 = 1;
const ACTIVE: u8 = 2;
const NEW: u8 = 4;
const UNK: u8 = 8;

// Zigzag mapping: coefficient index (0..1023) → (row, col) within 32×32 block
// Derived from interleaved bit-reversal:
//   col = bit0*16 + bit2*8 + bit4*4 + bit6*2 + bit8
//   row = bit1*16 + bit3*8 + bit5*4 + bit7*2 + bit9
const fn zigzag_row(i: usize) -> u8 {
    let b1 = ((i >> 1) & 1) as u8;
    let b3 = ((i >> 3) & 1) as u8;
    let b5 = ((i >> 5) & 1) as u8;
    let b7 = ((i >> 7) & 1) as u8;
    let b9 = ((i >> 9) & 1) as u8;
    b1 * 16 + b3 * 8 + b5 * 4 + b7 * 2 + b9
}

const fn zigzag_col(i: usize) -> u8 {
    let b0 = (i & 1) as u8;
    let b2 = ((i >> 2) & 1) as u8;
    let b4 = ((i >> 4) & 1) as u8;
    let b6 = ((i >> 6) & 1) as u8;
    let b8 = ((i >> 8) & 1) as u8;
    b0 * 16 + b2 * 8 + b4 * 4 + b6 * 2 + b8
}

static ZIGZAG_ROW: [u8; 1024] = {
    let mut table = [0u8; 1024];
    let mut i = 0;
    while i < 1024 {
        table[i] = zigzag_row(i);
        i += 1;
    }
    table
};

static ZIGZAG_COL: [u8; 1024] = {
    let mut table = [0u8; 1024];
    let mut i = 0;
    while i < 1024 {
        table[i] = zigzag_col(i);
        i += 1;
    }
    table
};

fn normalize(val: i16) -> i32 {
    let v = ((val as i32) + 32) >> 6;
    v.clamp(-128, 127)
}

/// Per-channel wavelet decoder. Holds block coefficients and progressive decoding state.
struct IWDecoder {
    width: usize,
    height: usize,
    block_cols: usize,
    blocks: Vec<[i16; 1024]>,
    quant_lo: [u32; 16],
    quant_hi: [u32; 10],
    curband: usize,
    // ZP contexts (persistent across slices)
    decode_bucket_ctx: [u8; 1],
    decode_coef_ctx: [u8; 80],
    activate_coef_ctx: [u8; 16],
    increase_coef_ctx: [u8; 1],
    // Per-block temporary state
    coeffstate: [[u8; 16]; 16],
    bucketstate: [u8; 16],
    bbstate: u8,
}

impl IWDecoder {
    fn new(width: usize, height: usize) -> Self {
        let block_cols = (width + 31) / 32;
        let block_rows = (height + 31) / 32;
        let block_count = block_cols * block_rows;
        IWDecoder {
            width,
            height,
            block_cols,
            blocks: vec![[0i16; 1024]; block_count],
            quant_lo: QUANT_LO_INIT,
            quant_hi: QUANT_HI_INIT,
            curband: 0,
            decode_bucket_ctx: [0; 1],
            decode_coef_ctx: [0; 80],
            activate_coef_ctx: [0; 16],
            increase_coef_ctx: [0; 1],
            coeffstate: [[0; 16]; 16],
            bucketstate: [0; 16],
            bbstate: 0,
        }
    }

    fn decode_slice(&mut self, zp: &mut ZPDecoder) {
        if !self.is_null_slice() {
            for block_idx in 0..self.blocks.len() {
                self.preliminary_flag_computation(block_idx);
                if self.block_band_decoding_pass(zp) {
                    self.bucket_decoding_pass(zp, block_idx);
                    self.newly_active_coefficient_decoding_pass(zp, block_idx);
                }
                self.previously_active_coefficient_decoding_pass(zp, block_idx);
            }
        }
        self.finish_code_slice();
    }

    fn is_null_slice(&mut self) -> bool {
        if self.curband == 0 {
            let mut is_null = true;
            for i in 0..16 {
                let threshold = self.quant_lo[i];
                self.coeffstate[0][i] = ZERO;
                if threshold > 0 && threshold < 0x8000 {
                    self.coeffstate[0][i] = UNK;
                    is_null = false;
                }
            }
            is_null
        } else {
            let threshold = self.quant_hi[self.curband];
            !(threshold > 0 && threshold < 0x8000)
        }
    }

    fn preliminary_flag_computation(&mut self, block_idx: usize) {
        self.bbstate = 0;
        let (from, to) = BAND_BUCKETS[self.curband];

        if self.curband != 0 {
            let mut boff = 0;
            for j in from..=to {
                let mut bstatetmp: u8 = 0;
                for k in 0..16 {
                    if self.blocks[block_idx][(j << 4) | k] == 0 {
                        self.coeffstate[boff][k] = UNK;
                    } else {
                        self.coeffstate[boff][k] = ACTIVE;
                    }
                    bstatetmp |= self.coeffstate[boff][k];
                }
                self.bucketstate[boff] = bstatetmp;
                self.bbstate |= bstatetmp;
                boff += 1;
            }
        } else {
            let mut bstatetmp: u8 = 0;
            for k in 0..16 {
                if self.coeffstate[0][k] != ZERO {
                    if self.blocks[block_idx][k] == 0 {
                        self.coeffstate[0][k] = UNK;
                    } else {
                        self.coeffstate[0][k] = ACTIVE;
                    }
                }
                bstatetmp |= self.coeffstate[0][k];
            }
            self.bucketstate[0] = bstatetmp;
            self.bbstate |= bstatetmp;
        }
    }

    fn block_band_decoding_pass(&mut self, zp: &mut ZPDecoder) -> bool {
        let (from, to) = BAND_BUCKETS[self.curband];
        let bcount = to - from + 1;
        if bcount < 16 || (self.bbstate & ACTIVE) != 0 {
            self.bbstate |= NEW;
        } else if (self.bbstate & UNK) != 0 {
            if zp.decode(&mut self.decode_bucket_ctx[0]) {
                self.bbstate |= NEW;
            }
        }
        (self.bbstate & NEW) != 0
    }

    fn bucket_decoding_pass(&mut self, zp: &mut ZPDecoder, block_idx: usize) {
        let (from, to) = BAND_BUCKETS[self.curband];
        let mut boff = 0;
        for i in from..=to {
            if (self.bucketstate[boff] & UNK) == 0 {
                boff += 1;
                continue;
            }
            let mut n: usize = 0;
            if self.curband != 0 {
                let t = 4 * i;
                for j in t..t + 4 {
                    if self.blocks[block_idx][j] != 0 {
                        n += 1;
                    }
                }
                if n == 4 {
                    n = 3;
                }
            }
            if (self.bbstate & ACTIVE) != 0 {
                n |= 4;
            }
            if zp.decode(&mut self.decode_coef_ctx[n + self.curband * 8]) {
                self.bucketstate[boff] |= NEW;
            }
            boff += 1;
        }
    }

    fn newly_active_coefficient_decoding_pass(&mut self, zp: &mut ZPDecoder, block_idx: usize) {
        let (from, to) = BAND_BUCKETS[self.curband];
        let mut boff = 0;
        let mut step = self.quant_hi[self.curband];
        for i in from..=to {
            if (self.bucketstate[boff] & NEW) != 0 {
                let shift: usize = if (self.bucketstate[boff] & ACTIVE) != 0 { 8 } else { 0 };
                let mut np: usize = 0;
                for j in 0..16 {
                    if (self.coeffstate[boff][j] & UNK) != 0 {
                        np += 1;
                    }
                }
                for j in 0..16 {
                    if (self.coeffstate[boff][j] & UNK) != 0 {
                        let ip = np.min(7);
                        if zp.decode(&mut self.activate_coef_ctx[shift + ip]) {
                            let sign = if zp.decode_iw() { -1i32 } else { 1i32 };
                            np = 0;
                            if self.curband == 0 {
                                step = self.quant_lo[j];
                            }
                            let s = step as i32;
                            let val = sign * (s + (s >> 1) - (s >> 3));
                            self.blocks[block_idx][(i << 4) | j] = val as i16;
                        }
                        if np > 0 {
                            np -= 1;
                        }
                    }
                }
            }
            boff += 1;
        }
    }

    fn previously_active_coefficient_decoding_pass(&mut self, zp: &mut ZPDecoder, block_idx: usize) {
        let (from, to) = BAND_BUCKETS[self.curband];
        let mut boff = 0;
        let mut step = self.quant_hi[self.curband];
        for i in from..=to {
            for j in 0..16 {
                if (self.coeffstate[boff][j] & ACTIVE) != 0 {
                    if self.curband == 0 {
                        step = self.quant_lo[j];
                    }
                    let coef = self.blocks[block_idx][(i << 4) | j];
                    let mut abs_coef = coef.unsigned_abs() as i32;
                    let s = step as i32;
                    let des = if abs_coef <= 3 * s {
                        let d = zp.decode(&mut self.increase_coef_ctx[0]);
                        abs_coef += s >> 2;
                        d
                    } else {
                        zp.decode_iw()
                    };
                    if des {
                        abs_coef += s >> 1;
                    } else {
                        abs_coef += -s + (s >> 1);
                    }
                    self.blocks[block_idx][(i << 4) | j] = if coef < 0 {
                        -abs_coef as i16
                    } else {
                        abs_coef as i16
                    };
                }
            }
            boff += 1;
        }
    }

    fn finish_code_slice(&mut self) {
        self.quant_hi[self.curband] >>= 1;
        if self.curband == 0 {
            for i in 0..16 {
                self.quant_lo[i] >>= 1;
            }
        }
        self.curband += 1;
        if self.curband == 10 {
            self.curband = 0;
        }
    }

    fn get_bytemap(&self, subsample: usize) -> Bytemap {
        let full_width = ((self.width + 31) / 32) * 32;
        let full_height = ((self.height + 31) / 32) * 32;
        let block_rows = (self.height + 31) / 32;
        let mut bm = Bytemap {
            data: vec![0i16; full_width * full_height],
            stride: full_width,
        };

        for r in 0..block_rows {
            for c in 0..self.block_cols {
                let block = &self.blocks[r * self.block_cols + c];
                let row_base = r << 5;
                let col_base = c << 5;
                for i in 0..1024 {
                    let row = ZIGZAG_ROW[i] as usize + row_base;
                    let col = ZIGZAG_COL[i] as usize + col_base;
                    bm.data[row * full_width + col] = block[i];
                }
            }
        }

        inverse_wavelet_transform(&mut bm, self.width, self.height, subsample);
        bm
    }
}

struct Bytemap {
    data: Vec<i16>,
    stride: usize,
}

impl Bytemap {
    #[inline]
    fn get(&self, row: usize, col: usize) -> i32 {
        self.data[row * self.stride + col] as i32
    }

    #[inline]
    fn add(&mut self, row: usize, col: usize, val: i32) {
        self.data[row * self.stride + col] =
            (self.data[row * self.stride + col] as i32 + val) as i16;
    }

    #[inline]
    fn sub(&mut self, row: usize, col: usize, val: i32) {
        self.data[row * self.stride + col] =
            (self.data[row * self.stride + col] as i32 - val) as i16;
    }
}

fn inverse_wavelet_transform(bm: &mut Bytemap, width: usize, height: usize, subsample: usize) {
    let mut s_degree: u32 = 4;
    let mut s = 16usize;
    while s >= subsample {
        // Columns first
        let kmax = (height - 1) >> s_degree;
        let border = if kmax >= 3 { kmax - 3 } else { 0 };
        for i in (0..width).step_by(s) {
            // Lifting (even samples)
            let mut prev1: i32 = 0;
            let mut next1: i32 = 0;
            let mut next3: i32 = if 1 > kmax { 0 } else { bm.get(1 << s_degree, i) };
            let mut prev3: i32;
            let mut k = 0;
            while k <= kmax {
                prev3 = prev1;
                prev1 = next1;
                next1 = next3;
                next3 = if k + 3 > kmax { 0 } else { bm.get((k + 3) << s_degree, i) };
                let a = prev1 + next1;
                let c = prev3 + next3;
                bm.sub(k << s_degree, i, ((a << 3) + a - c + 16) >> 5);
                k += 2;
            }

            // Prediction (odd samples)
            k = 1;
            prev1 = bm.get((k - 1) << s_degree, i);
            if k + 1 <= kmax {
                next1 = bm.get((k + 1) << s_degree, i);
                bm.add(k << s_degree, i, (prev1 + next1 + 1) >> 1);
            } else {
                bm.add(k << s_degree, i, prev1);
            }

            if border >= 3 {
                next3 = bm.get((k + 3) << s_degree, i);
            }

            k = 3;
            while k <= border {
                prev3 = prev1;
                prev1 = next1;
                next1 = next3;
                next3 = bm.get((k + 3) << s_degree, i);
                let a = prev1 + next1;
                bm.add(
                    k << s_degree,
                    i,
                    ((a << 3) + a - (prev3 + next3) + 8) >> 4,
                );
                k += 2;
            }

            while k <= kmax {
                prev1 = next1;
                next1 = next3;
                next3 = 0;
                if k + 1 <= kmax {
                    bm.add(k << s_degree, i, (prev1 + next1 + 1) >> 1);
                } else {
                    bm.add(k << s_degree, i, prev1);
                }
                k += 2;
            }
        }

        // Rows
        let kmax = (width - 1) >> s_degree;
        let border = if kmax >= 3 { kmax - 3 } else { 0 };
        for i in (0..height).step_by(s) {
            // Lifting (even samples)
            let mut prev1: i32 = 0;
            let mut next1: i32 = 0;
            let mut next3: i32 = if 1 > kmax { 0 } else { bm.get(i, 1 << s_degree) };
            let mut prev3: i32;
            let mut k = 0;
            while k <= kmax {
                prev3 = prev1;
                prev1 = next1;
                next1 = next3;
                next3 = if k + 3 > kmax { 0 } else { bm.get(i, (k + 3) << s_degree) };
                let a = prev1 + next1;
                let c = prev3 + next3;
                bm.sub(i, k << s_degree, ((a << 3) + a - c + 16) >> 5);
                k += 2;
            }

            // Prediction (odd samples)
            k = 1;
            prev1 = bm.get(i, (k - 1) << s_degree);
            if k + 1 <= kmax {
                next1 = bm.get(i, (k + 1) << s_degree);
                bm.add(i, k << s_degree, (prev1 + next1 + 1) >> 1);
            } else {
                bm.add(i, k << s_degree, prev1);
            }

            if border >= 3 {
                next3 = bm.get(i, (k + 3) << s_degree);
            }

            k = 3;
            while k <= border {
                prev3 = prev1;
                prev1 = next1;
                next1 = next3;
                next3 = bm.get(i, (k + 3) << s_degree);
                let a = prev1 + next1;
                bm.add(
                    i,
                    k << s_degree,
                    ((a << 3) + a - (prev3 + next3) + 8) >> 4,
                );
                k += 2;
            }

            while k <= kmax {
                prev1 = next1;
                next1 = next3;
                next3 = 0;
                if k + 1 <= kmax {
                    bm.add(i, k << s_degree, (prev1 + next1 + 1) >> 1);
                } else {
                    bm.add(i, k << s_degree, prev1);
                }
                k += 2;
            }
        }

        s >>= 1;
        if s_degree > 0 {
            s_degree -= 1;
        }
    }
}

/// IW44 progressive wavelet image decoder.
pub struct IW44Image {
    width: u16,
    height: u16,
    is_color: bool,
    delay: u8,
    y_codec: Option<IWDecoder>,
    cb_codec: Option<IWDecoder>,
    cr_codec: Option<IWDecoder>,
    cslice: usize,
}

impl IW44Image {
    pub fn new() -> Self {
        IW44Image {
            width: 0,
            height: 0,
            is_color: false,
            delay: 0,
            y_codec: None,
            cb_codec: None,
            cr_codec: None,
            cslice: 0,
        }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    /// Decode one BG44/FG44 chunk. Call multiple times for progressive chunks.
    pub fn decode_chunk(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() < 2 {
            return Err("IW44 chunk too short");
        }
        let serial = data[0];
        let slices = data[1];
        let payload_start;

        if serial == 0 {
            if data.len() < 9 {
                return Err("IW44 first chunk header too short");
            }
            let majver = data[2];
            let is_grayscale = (majver >> 7) != 0;
            let w = u16::from_be_bytes([data[4], data[5]]);
            let h = u16::from_be_bytes([data[6], data[7]]);
            let delay_byte = data[8];
            let delay = delay_byte & 127;

            self.width = w;
            self.height = h;
            self.is_color = !is_grayscale;
            self.delay = delay;
            self.cslice = 0;
            self.y_codec = Some(IWDecoder::new(w as usize, h as usize));
            if self.is_color {
                self.cb_codec = Some(IWDecoder::new(w as usize, h as usize));
                self.cr_codec = Some(IWDecoder::new(w as usize, h as usize));
            }
            payload_start = 9;
        } else {
            if self.y_codec.is_none() {
                return Err("IW44 subsequent chunk before first chunk");
            }
            payload_start = 2;
        }

        let zp_data = &data[payload_start..];
        let mut zp = ZPDecoder::new(zp_data);

        for _ in 0..slices {
            self.cslice += 1;
            if let Some(ref mut y) = self.y_codec {
                y.decode_slice(&mut zp);
            }
            if self.is_color && self.cslice > self.delay as usize {
                if let Some(ref mut cb) = self.cb_codec {
                    cb.decode_slice(&mut zp);
                }
                if let Some(ref mut cr) = self.cr_codec {
                    cr.decode_slice(&mut zp);
                }
            }
        }

        Ok(())
    }

    /// Convert decoded image to a Pixmap. DjVu images are bottom-to-top; this flips to top-to-bottom.
    pub fn to_pixmap(&self) -> Result<Pixmap, Error> {
        self.to_pixmap_subsample(1)
    }

    /// Convert decoded image to a Pixmap at reduced resolution (subsample=1,2,4,8,16).
    pub fn to_pixmap_subsample(&self, subsample: u32) -> Result<Pixmap, Error> {
        if subsample == 0 {
            return Err(Error::Unsupported("subsample must be >= 1"));
        }
        let y_codec = self.y_codec.as_ref()
            .ok_or(Error::MissingChunk("BG44/FG44"))?;
        let sub = subsample as usize;
        let w = ((self.width as usize + sub - 1) / sub) as u32;
        let h = ((self.height as usize + sub - 1) / sub) as u32;

        let y_bm = y_codec.get_bytemap(sub);

        if self.is_color {
            let cb_bm = self.cb_codec.as_ref()
                .ok_or(Error::MissingChunk("BG44/FG44 Cb"))?.get_bytemap(sub);
            let cr_bm = self.cr_codec.as_ref()
                .ok_or(Error::MissingChunk("BG44/FG44 Cr"))?.get_bytemap(sub);
            let mut pm = Pixmap::new(w, h, 0, 0, 0, 255);
            for row in 0..h {
                let out_row = h - 1 - row;
                for col in 0..w {
                    let src_row = row as usize * sub;
                    let src_col = col as usize * sub;
                    let idx = src_row * y_bm.stride + src_col;
                    let y = normalize(y_bm.data[idx]);
                    let b = normalize(cb_bm.data[idx]);
                    let r = normalize(cr_bm.data[idx]);

                    let t2 = r + (r >> 1);
                    let t3 = y + 128 - (b >> 2);

                    let red = (y + 128 + t2).clamp(0, 255) as u8;
                    let green = (t3 - (t2 >> 1)).clamp(0, 255) as u8;
                    let blue = (t3 + (b << 1)).clamp(0, 255) as u8;
                    pm.set_rgb(col, out_row, red, green, blue);
                }
            }
            Ok(pm)
        } else {
            let mut pm = Pixmap::new(w, h, 0, 0, 0, 255);
            for row in 0..h {
                let out_row = h - 1 - row;
                for col in 0..w {
                    let src_row = row as usize * sub;
                    let src_col = col as usize * sub;
                    let idx = src_row * y_bm.stride + src_col;
                    let val = normalize(y_bm.data[idx]);
                    let gray = (127 - val) as u8;
                    pm.set_rgb(col, out_row, gray, gray, gray);
                }
            }
            Ok(pm)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets_path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("references/djvujs/library/assets")
    }

    fn golden_path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tests/golden/iw44")
    }

    fn extract_bg44_chunks<'a>(file: &'a rdjvu_iff::DjvuFile<'a>) -> Vec<&'a [u8]> {
        fn collect_from_djvu_form<'a>(chunk: &'a rdjvu_iff::Chunk<'a>) -> Option<Vec<&'a [u8]>> {
            match chunk {
                rdjvu_iff::Chunk::Form { secondary_id, children, .. } => {
                    if secondary_id == b"DJVU" {
                        let v = children
                            .iter()
                            .filter_map(|c| match c {
                                rdjvu_iff::Chunk::Leaf { id, data } if id == b"BG44" => Some(*data),
                                _ => None,
                            })
                            .collect::<Vec<_>>();
                        return Some(v);
                    }
                    for c in children {
                        if let Some(v) = collect_from_djvu_form(c) {
                            return Some(v);
                        }
                    }
                    None
                }
                _ => None,
            }
        }
        collect_from_djvu_form(&file.root).unwrap_or_default()
    }

    fn find_ppm_data_start(ppm: &[u8]) -> usize {
        let mut newlines = 0;
        for (i, &b) in ppm.iter().enumerate() {
            if b == b'\n' {
                newlines += 1;
                if newlines == 3 {
                    return i + 1;
                }
            }
        }
        0
    }

    fn assert_ppm_match(actual_ppm: &[u8], golden_file: &str) {
        let expected_ppm = std::fs::read(golden_path().join(golden_file)).unwrap();
        assert_eq!(
            actual_ppm.len(),
            expected_ppm.len(),
            "PPM size mismatch for {}: got {} expected {}",
            golden_file,
            actual_ppm.len(),
            expected_ppm.len()
        );
        if actual_ppm != expected_ppm {
            let header_end = find_ppm_data_start(actual_ppm);
            let actual_pixels = &actual_ppm[header_end..];
            let expected_pixels = &expected_ppm[header_end..];
            let total_pixels = actual_pixels.len() / 3;
            let diff_pixels = actual_pixels
                .chunks(3)
                .zip(expected_pixels.chunks(3))
                .filter(|(a, b)| a != b)
                .count();
            panic!(
                "{} pixel mismatch: {}/{} pixels differ ({:.1}%)",
                golden_file,
                diff_pixels,
                total_pixels,
                diff_pixels as f64 / total_pixels as f64 * 100.0
            );
        }
    }

    #[test]
    fn zigzag_table_spot_checks() {
        assert_eq!(ZIGZAG_ROW[0], 0);
        assert_eq!(ZIGZAG_COL[0], 0);
        assert_eq!(ZIGZAG_ROW[1], 0);
        assert_eq!(ZIGZAG_COL[1], 16);
        assert_eq!(ZIGZAG_ROW[2], 16);
        assert_eq!(ZIGZAG_COL[2], 0);
        assert_eq!(ZIGZAG_ROW[3], 16);
        assert_eq!(ZIGZAG_COL[3], 16);
    }

    #[test]
    fn iw44_decode_boy_bg() {
        let data = std::fs::read(assets_path().join("boy.djvu")).unwrap();
        let file = rdjvu_iff::parse(&data).unwrap();
        let chunks = extract_bg44_chunks(&file);
        assert_eq!(chunks.len(), 1);

        let mut img = IW44Image::new();
        for c in &chunks {
            img.decode_chunk(c).unwrap();
        }
        assert_eq!(img.width(), 192);
        assert_eq!(img.height(), 256);

        let pm = img.to_pixmap().unwrap();
        assert_ppm_match(&pm.to_ppm(), "boy_bg.ppm");
    }

    #[test]
    fn iw44_decode_big_scanned_sub4() {
        let data = std::fs::read(assets_path().join("big-scanned-page.djvu")).unwrap();
        let file = rdjvu_iff::parse(&data).unwrap();
        let chunks = extract_bg44_chunks(&file);
        assert_eq!(chunks.len(), 4);

        let mut img = IW44Image::new();
        for c in &chunks {
            img.decode_chunk(c).unwrap();
        }
        assert_eq!(img.width(), 6780);
        assert_eq!(img.height(), 9148);

        let pm = img.to_pixmap_subsample(4).unwrap();
        assert_ppm_match(&pm.to_ppm(), "big_scanned_sub4.ppm");
    }

    #[test]
    fn iw44_decode_chicken_bg() {
        let data = std::fs::read(assets_path().join("chicken.djvu")).unwrap();
        let file = rdjvu_iff::parse(&data).unwrap();
        let chunks = extract_bg44_chunks(&file);
        assert_eq!(chunks.len(), 3);

        let mut img = IW44Image::new();
        for c in &chunks {
            img.decode_chunk(c).unwrap();
        }
        assert_eq!(img.width(), 181);
        assert_eq!(img.height(), 240);

        let pm = img.to_pixmap().unwrap();
        assert_ppm_match(&pm.to_ppm(), "chicken_bg.ppm");
    }
}
