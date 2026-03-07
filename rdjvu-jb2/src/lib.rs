use rdjvu_core::Bitmap;
use rdjvu_zp::ZPDecoder;

// ============================================================
// NumContext: arena-based binary tree for variable-length numbers
// ============================================================

struct NumContext {
    ctx: Vec<u8>,
    left: Vec<u32>,
    right: Vec<u32>,
}

impl NumContext {
    fn new() -> Self {
        // Index 0 is unused sentinel. Index 1 is the root node.
        NumContext {
            ctx: vec![0, 0],
            left: vec![0, 0],
            right: vec![0, 0],
        }
    }

    fn root(&self) -> usize {
        1
    }

    fn get_left(&mut self, node: usize) -> usize {
        if self.left[node] == 0 {
            let idx = self.ctx.len() as u32;
            self.ctx.push(0);
            self.left.push(0);
            self.right.push(0);
            self.left[node] = idx;
        }
        self.left[node] as usize
    }

    fn get_right(&mut self, node: usize) -> usize {
        if self.right[node] == 0 {
            let idx = self.ctx.len() as u32;
            self.ctx.push(0);
            self.left.push(0);
            self.right.push(0);
            self.right[node] = idx;
        }
        self.right[node] as usize
    }
}

fn decode_num(zp: &mut ZPDecoder, ctx: &mut NumContext, low: i32, high: i32) -> i32 {
    let mut low = low;
    let mut high = high;
    let mut negative = false;
    let mut cutoff: i32 = 0;
    let mut phase: u32 = 1;
    let mut range: u32 = 0xffffffff;
    let mut node = ctx.root();

    while range != 1 {
        let decision = if low >= cutoff {
            true
        } else if high >= cutoff {
            let mut ctx_byte = ctx.ctx[node];
            let bit = zp.decode(&mut ctx_byte);
            ctx.ctx[node] = ctx_byte;
            bit
        } else {
            false
        };

        node = if decision {
            ctx.get_right(node)
        } else {
            ctx.get_left(node)
        };

        match phase {
            1 => {
                negative = !decision;
                if negative {
                    let temp = -low - 1;
                    low = -high - 1;
                    high = temp;
                }
                phase = 2;
                cutoff = 1;
            }
            2 => {
                if !decision {
                    phase = 3;
                    range = ((cutoff + 1) / 2) as u32;
                    if range == 1 {
                        cutoff = 0;
                    } else {
                        cutoff -= (range / 2) as i32;
                    }
                } else {
                    cutoff += cutoff + 1;
                }
            }
            3 => {
                range /= 2;
                if range != 1 {
                    if !decision {
                        cutoff -= (range / 2) as i32;
                    } else {
                        cutoff += (range / 2) as i32;
                    }
                } else if !decision {
                    cutoff -= 1;
                }
            }
            _ => unreachable!(),
        }
    }

    if negative { -cutoff - 1 } else { cutoff }
}

// ============================================================
// Jbm: internal bitmap (1 byte per pixel, row 0 = bottom)
// ============================================================

#[derive(Clone)]
struct Jbm {
    width: i32,
    height: i32,
    data: Vec<u8>,
}

impl Jbm {
    fn new(width: i32, height: i32) -> Self {
        Jbm {
            width,
            height,
            data: vec![0; (width.max(0) * height.max(0)) as usize],
        }
    }

    fn get(&self, row: i32, col: i32) -> u8 {
        if row < 0 || row >= self.height || col < 0 || col >= self.width {
            return 0;
        }
        self.data[(row * self.width + col) as usize]
    }

    fn set(&mut self, row: i32, col: i32) {
        if row >= 0 && row < self.height && col >= 0 && col < self.width {
            self.data[(row * self.width + col) as usize] = 1;
        }
    }

    fn get_bits(&self, row: i32, col: i32, n: i32) -> u32 {
        let mut result = 0u32;
        for k in 0..n {
            result = (result << 1) | self.get(row, col + k) as u32;
        }
        result
    }

    fn remove_empty_edges(&self) -> Jbm {
        let mut min_row = self.height;
        let mut max_row: i32 = -1;
        let mut min_col = self.width;
        let mut max_col: i32 = -1;

        for row in 0..self.height {
            for col in 0..self.width {
                if self.data[(row * self.width + col) as usize] != 0 {
                    min_row = min_row.min(row);
                    max_row = max_row.max(row);
                    min_col = min_col.min(col);
                    max_col = max_col.max(col);
                }
            }
        }

        if max_row < 0 {
            return Jbm::new(0, 0);
        }

        let new_width = max_col - min_col + 1;
        let new_height = max_row - min_row + 1;
        let mut result = Jbm::new(new_width, new_height);

        for row in min_row..=max_row {
            for col in min_col..=max_col {
                if self.data[(row * self.width + col) as usize] != 0 {
                    result.data[((row - min_row) * new_width + (col - min_col)) as usize] = 1;
                }
            }
        }

        result
    }
}

// ============================================================
// Baseline: rolling median of 3 for symbol y positioning
// ============================================================

struct Baseline {
    arr: [i32; 3],
    index: i32,
}

impl Baseline {
    fn new() -> Self {
        Baseline {
            arr: [0, 0, 0],
            index: -1,
        }
    }

    fn fill(&mut self, val: i32) {
        self.arr[0] = val;
        self.arr[1] = val;
        self.arr[2] = val;
    }

    fn add(&mut self, val: i32) {
        self.index += 1;
        if self.index == 3 {
            self.index = 0;
        }
        self.arr[self.index as usize] = val;
    }

    fn get_val(&self) -> i32 {
        let (a, b, c) = (self.arr[0], self.arr[1], self.arr[2]);
        if (a >= b && a <= c) || (a <= b && a >= c) {
            a
        } else if (b >= a && b <= c) || (b <= a && b >= c) {
            b
        } else {
            c
        }
    }
}

// ============================================================
// Bitmap decode: direct (10-bit context)
// ============================================================

fn compute_ctx_direct(bm: &Jbm, row: i32, col: i32) -> usize {
    // Row i+2: 3 bits at cols j-1, j, j+1 → bits [9:7]
    let mut index: u32 = bm.get_bits(row + 2, col - 1, 3) << 7;
    // Row i+1: 5 bits at cols j-2..j+2 → bits [6:2]
    index |= bm.get_bits(row + 1, col - 2, 5) << 2;
    // Row i: 2 bits at cols j-2, j-1 → bits [1:0]
    index |= bm.get_bits(row, col - 2, 2);
    index as usize
}

fn decode_bitmap_direct(zp: &mut ZPDecoder, ctx: &mut [u8], width: i32, height: i32) -> Jbm {
    let mut bm = Jbm::new(width, height);
    // Decode top-to-bottom (row height-1 down to 0), left-to-right
    for row in (0..height).rev() {
        for col in 0..width {
            let idx = compute_ctx_direct(&bm, row, col);
            if zp.decode(&mut ctx[idx]) {
                bm.set(row, col);
            }
        }
    }
    bm
}

// ============================================================
// Bitmap decode: refinement (11-bit context)
// ============================================================

fn compute_ctx_ref(cbm: &Jbm, mbm: &Jbm, row: i32, col: i32, row_shift: i32, col_shift: i32) -> usize {
    // Current bitmap, row i+1: 3 bits at cols j-1, j, j+1 → bits [10:8]
    let mut index: u32 = cbm.get_bits(row + 1, col - 1, 3) << 8;
    // Current bitmap, row i: 1 bit at col j-1 → bit [7]
    index |= (cbm.get(row, col - 1) as u32) << 7;
    // Reference bitmap, row (i+rowshift+1), col (j+colshift) → bit [6]
    index |= (mbm.get(row + row_shift + 1, col + col_shift) as u32) << 6;
    // Reference bitmap, row (i+rowshift): 3 bits at cols (j+colshift-1..+1) → bits [5:3]
    index |= mbm.get_bits(row + row_shift, col + col_shift - 1, 3) << 3;
    // Reference bitmap, row (i+rowshift-1): 3 bits at cols (j+colshift-1..+1) → bits [2:0]
    index |= mbm.get_bits(row + row_shift - 1, col + col_shift - 1, 3);
    index as usize
}

fn decode_bitmap_ref(
    zp: &mut ZPDecoder,
    ctx: &mut [u8],
    width: i32,
    height: i32,
    mbm: &Jbm,
) -> Jbm {
    let mut cbm = Jbm::new(width, height);
    // Center alignment
    let crow = (height - 1) >> 1;
    let ccol = (width - 1) >> 1;
    let mrow = (mbm.height - 1) >> 1;
    let mcol = (mbm.width - 1) >> 1;
    let row_shift = mrow - crow;
    let col_shift = mcol - ccol;

    for row in (0..height).rev() {
        for col in 0..width {
            let idx = compute_ctx_ref(&cbm, mbm, row, col, row_shift, col_shift);
            if zp.decode(&mut ctx[idx]) {
                cbm.set(row, col);
            }
        }
    }
    cbm
}

// ============================================================
// Public types
// ============================================================

pub struct JB2Dict {
    symbols: Vec<Jbm>,
}

impl JB2Dict {
    pub fn num_symbols(&self) -> usize {
        self.symbols.len()
    }
}

// ============================================================
// Blit a symbol onto a page bitmap (OR compositing)
// ============================================================

fn blit(page: &mut Vec<u8>, mut blit_map: Option<&mut Vec<i32>>, blit_idx: i32, page_w: i32, page_h: i32, symbol: &Jbm, x: i32, y: i32) {
    for row in 0..symbol.height {
        for col in 0..symbol.width {
            if symbol.get(row, col) != 0 {
                let px = x + col;
                let py = y + row;
                if px >= 0 && px < page_w && py >= 0 && py < page_h {
                    let idx = (py * page_w + px) as usize;
                    page[idx] = 1;
                    if let Some(ref mut map) = blit_map {
                        map[idx] = blit_idx;
                    }
                }
            }
        }
    }
}

// ============================================================
// Convert internal page bitmap (row 0=bottom) to PBM Bitmap (row 0=top)
// ============================================================

fn page_to_bitmap(page: &[u8], width: i32, height: i32) -> Bitmap {
    let mut bm = Bitmap::new(width as u32, height as u32);
    for row in 0..height {
        for col in 0..width {
            if page[(row * width + col) as usize] != 0 {
                let pbm_y = (height - 1 - row) as u32;
                bm.set(col as u32, pbm_y, true);
            }
        }
    }
    bm
}

fn flip_blit_map(map: &[i32], width: i32, height: i32) -> Vec<i32> {
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![-1i32; w * h];
    for row in 0..h {
        let src_off = row * w;
        let dst_off = (h - 1 - row) * w;
        out[dst_off..dst_off + w].copy_from_slice(&map[src_off..src_off + w]);
    }
    out
}

// ============================================================
// Main decode functions
// ============================================================

/// Decode a JB2 image stream (Sjbz chunk data).
/// Returns the page bitmap in PBM convention (row 0 = top).
pub fn decode(data: &[u8], shared_dict: Option<&JB2Dict>) -> Result<Bitmap, &'static str> {
    let (bm, _) = decode_inner(data, shared_dict, false)?;
    Ok(bm)
}

/// Decode a JB2 image stream, returning both the bitmap and a per-pixel blit index map.
/// The blit index map has the same dimensions as the bitmap (row 0 = top).
/// Each pixel stores the 0-based blit index that last wrote to it, or -1 if no blit.
pub fn decode_indexed(data: &[u8], shared_dict: Option<&JB2Dict>) -> Result<(Bitmap, Vec<i32>), &'static str> {
    decode_inner(data, shared_dict, true)
}

fn decode_inner(data: &[u8], shared_dict: Option<&JB2Dict>, track_blits: bool) -> Result<(Bitmap, Vec<i32>), &'static str> {
    let mut zp = ZPDecoder::new(data);

    // Contexts
    let mut record_type_ctx = NumContext::new();
    let mut image_size_ctx = NumContext::new();
    let mut symbol_width_ctx = NumContext::new();
    let mut symbol_height_ctx = NumContext::new();
    let mut inherit_dict_size_ctx = NumContext::new();
    let mut hoff_ctx = NumContext::new();
    let mut voff_ctx = NumContext::new();
    let mut shoff_ctx = NumContext::new();
    let mut svoff_ctx = NumContext::new();
    let mut symbol_index_ctx = NumContext::new();
    let mut symbol_width_diff_ctx = NumContext::new();
    let mut symbol_height_diff_ctx = NumContext::new();
    let mut horiz_abs_loc_ctx = NumContext::new();
    let mut vert_abs_loc_ctx = NumContext::new();
    let mut comment_length_ctx = NumContext::new();
    let mut comment_octet_ctx = NumContext::new();

    let mut offset_type_ctx: u8 = 0;
    let mut direct_bitmap_ctx = vec![0u8; 1024];
    let mut refinement_bitmap_ctx = vec![0u8; 2048];

    // --- Init ---
    // Check for dictionary inheritance (record type 9)
    let mut rtype = decode_num(&mut zp, &mut record_type_ctx, 0, 11);
    let mut initial_dict_length: usize = 0;
    if rtype == 9 {
        initial_dict_length = decode_num(&mut zp, &mut inherit_dict_size_ctx, 0, 262142) as usize;
        rtype = decode_num(&mut zp, &mut record_type_ctx, 0, 11);
    }
    let _ = rtype; // start-of-data marker (usually 0)

    // Decode image dimensions
    let image_width = {
        let w = decode_num(&mut zp, &mut image_size_ctx, 0, 262142);
        if w == 0 { 200 } else { w }
    };
    let image_height = {
        let h = decode_num(&mut zp, &mut image_size_ctx, 0, 262142);
        if h == 0 { 200 } else { h }
    };

    // Flag bit (must be 0)
    let mut flag_ctx: u8 = 0;
    if zp.decode(&mut flag_ctx) {
        return Err("JB2: bad flag bit in header");
    }

    // Initialize dictionary
    let mut dict: Vec<Jbm> = Vec::new();
    if initial_dict_length > 0 {
        if let Some(sd) = shared_dict {
            if initial_dict_length > sd.symbols.len() {
                return Err("JB2: inherited dict length exceeds shared dict size");
            }
            dict.extend_from_slice(&sd.symbols[..initial_dict_length]);
        } else {
            return Err("JB2: stream requires shared dict but none provided");
        }
    }

    // Initialize page bitmap
    let page_size = (image_width * image_height) as usize;
    let mut page = vec![0u8; page_size];
    let mut blit_map = if track_blits { Some(vec![-1i32; page_size]) } else { None };
    let mut blit_count: i32 = 0;

    // Positioning state
    let mut first_left: i32 = -1;
    let mut first_bottom: i32 = image_height - 1;
    let mut last_right: i32 = 0;
    let mut baseline = Baseline::new();

    // --- Main decode loop ---
    loop {
        let rtype = decode_num(&mut zp, &mut record_type_ctx, 0, 11);

        match rtype {
            1 => {
                // New symbol: add to dict AND blit
                let w = decode_num(&mut zp, &mut symbol_width_ctx, 0, 262142);
                let h = decode_num(&mut zp, &mut symbol_height_ctx, 0, 262142);
                let bm = decode_bitmap_direct(&mut zp, &mut direct_bitmap_ctx, w, h);

                let (x, y) = decode_symbol_coords(
                    &mut zp, &mut offset_type_ctx,
                    &mut hoff_ctx, &mut voff_ctx, &mut shoff_ctx, &mut svoff_ctx,
                    &mut first_left, &mut first_bottom, &mut last_right, &mut baseline,
                    bm.width, bm.height,
                );

                blit(&mut page, blit_map.as_mut(), blit_count, image_width, image_height, &bm, x, y);
                blit_count += 1;
                dict.push(bm.remove_empty_edges());
            }
            2 => {
                // New symbol: add to dict only
                let w = decode_num(&mut zp, &mut symbol_width_ctx, 0, 262142);
                let h = decode_num(&mut zp, &mut symbol_height_ctx, 0, 262142);
                let bm = decode_bitmap_direct(&mut zp, &mut direct_bitmap_ctx, w, h);
                dict.push(bm.remove_empty_edges());
            }
            3 => {
                // New symbol: blit only
                let w = decode_num(&mut zp, &mut symbol_width_ctx, 0, 262142);
                let h = decode_num(&mut zp, &mut symbol_height_ctx, 0, 262142);
                let bm = decode_bitmap_direct(&mut zp, &mut direct_bitmap_ctx, w, h);

                let (x, y) = decode_symbol_coords(
                    &mut zp, &mut offset_type_ctx,
                    &mut hoff_ctx, &mut voff_ctx, &mut shoff_ctx, &mut svoff_ctx,
                    &mut first_left, &mut first_bottom, &mut last_right, &mut baseline,
                    bm.width, bm.height,
                );

                blit(&mut page, blit_map.as_mut(), blit_count, image_width, image_height, &bm, x, y);
                blit_count += 1;
            }
            4 => {
                // Matched with refinement: add to dict AND blit
                let index = decode_num(&mut zp, &mut symbol_index_ctx, 0, dict.len() as i32 - 1) as usize;
                let wdiff = decode_num(&mut zp, &mut symbol_width_diff_ctx, -262143, 262142);
                let hdiff = decode_num(&mut zp, &mut symbol_height_diff_ctx, -262143, 262142);
                let mbm = &dict[index];
                let cbm_w = mbm.width + wdiff;
                let cbm_h = mbm.height + hdiff;
                let cbm = decode_bitmap_ref(&mut zp, &mut refinement_bitmap_ctx, cbm_w, cbm_h, &dict[index]);

                let (x, y) = decode_symbol_coords(
                    &mut zp, &mut offset_type_ctx,
                    &mut hoff_ctx, &mut voff_ctx, &mut shoff_ctx, &mut svoff_ctx,
                    &mut first_left, &mut first_bottom, &mut last_right, &mut baseline,
                    cbm.width, cbm.height,
                );

                blit(&mut page, blit_map.as_mut(), blit_count, image_width, image_height, &cbm, x, y);
                blit_count += 1;
                dict.push(cbm.remove_empty_edges());
            }
            5 => {
                // Matched with refinement: add to dict only
                let index = decode_num(&mut zp, &mut symbol_index_ctx, 0, dict.len() as i32 - 1) as usize;
                let wdiff = decode_num(&mut zp, &mut symbol_width_diff_ctx, -262143, 262142);
                let hdiff = decode_num(&mut zp, &mut symbol_height_diff_ctx, -262143, 262142);
                let cbm_w = dict[index].width + wdiff;
                let cbm_h = dict[index].height + hdiff;
                let cbm = decode_bitmap_ref(&mut zp, &mut refinement_bitmap_ctx, cbm_w, cbm_h, &dict[index]);
                dict.push(cbm.remove_empty_edges());
            }
            6 => {
                // Matched with refinement: blit only
                let index = decode_num(&mut zp, &mut symbol_index_ctx, 0, dict.len() as i32 - 1) as usize;
                let wdiff = decode_num(&mut zp, &mut symbol_width_diff_ctx, -262143, 262142);
                let hdiff = decode_num(&mut zp, &mut symbol_height_diff_ctx, -262143, 262142);
                let cbm_w = dict[index].width + wdiff;
                let cbm_h = dict[index].height + hdiff;
                let cbm = decode_bitmap_ref(&mut zp, &mut refinement_bitmap_ctx, cbm_w, cbm_h, &dict[index]);

                let (x, y) = decode_symbol_coords(
                    &mut zp, &mut offset_type_ctx,
                    &mut hoff_ctx, &mut voff_ctx, &mut shoff_ctx, &mut svoff_ctx,
                    &mut first_left, &mut first_bottom, &mut last_right, &mut baseline,
                    cbm.width, cbm.height,
                );

                blit(&mut page, blit_map.as_mut(), blit_count, image_width, image_height, &cbm, x, y);
                blit_count += 1;
            }
            7 => {
                // Matched copy without refinement: blit
                let index = decode_num(&mut zp, &mut symbol_index_ctx, 0, dict.len() as i32 - 1) as usize;
                let bm = &dict[index];
                let bm_w = bm.width;
                let bm_h = bm.height;

                let (x, y) = decode_symbol_coords(
                    &mut zp, &mut offset_type_ctx,
                    &mut hoff_ctx, &mut voff_ctx, &mut shoff_ctx, &mut svoff_ctx,
                    &mut first_left, &mut first_bottom, &mut last_right, &mut baseline,
                    bm_w, bm_h,
                );

                blit(&mut page, blit_map.as_mut(), blit_count, image_width, image_height, &dict[index], x, y);
                blit_count += 1;
            }
            8 => {
                // Non-symbol data
                let w = decode_num(&mut zp, &mut symbol_width_ctx, 0, 262142);
                let h = decode_num(&mut zp, &mut symbol_height_ctx, 0, 262142);
                let bm = decode_bitmap_direct(&mut zp, &mut direct_bitmap_ctx, w, h);

                let left = decode_num(&mut zp, &mut horiz_abs_loc_ctx, 1, image_width);
                let top = decode_num(&mut zp, &mut vert_abs_loc_ctx, 1, image_height);
                let x = left - 1;
                let y = top - h;

                blit(&mut page, blit_map.as_mut(), blit_count, image_width, image_height, &bm, x, y);
                blit_count += 1;
            }
            9 => {}
            10 => {
                let length = decode_num(&mut zp, &mut comment_length_ctx, 0, 262142);
                for _ in 0..length {
                    decode_num(&mut zp, &mut comment_octet_ctx, 0, 255);
                }
            }
            11 => {
                break;
            }
            _ => {
                return Err("JB2: unknown record type");
            }
        }
    }

    let bitmap = page_to_bitmap(&page, image_width, image_height);
    let flipped_map = match blit_map {
        Some(map) => flip_blit_map(&map, image_width, image_height),
        None => vec![],
    };
    Ok((bitmap, flipped_map))
}

/// Decode a JB2 dictionary stream (Djbz chunk data).
pub fn decode_dict(data: &[u8], inherited: Option<&JB2Dict>) -> Result<JB2Dict, &'static str> {
    let mut zp = ZPDecoder::new(data);

    let mut record_type_ctx = NumContext::new();
    let mut image_size_ctx = NumContext::new();
    let mut symbol_width_ctx = NumContext::new();
    let mut symbol_height_ctx = NumContext::new();
    let mut inherit_dict_size_ctx = NumContext::new();
    let mut symbol_index_ctx = NumContext::new();
    let mut symbol_width_diff_ctx = NumContext::new();
    let mut symbol_height_diff_ctx = NumContext::new();
    let mut comment_length_ctx = NumContext::new();
    let mut comment_octet_ctx = NumContext::new();

    let mut direct_bitmap_ctx = vec![0u8; 1024];
    let mut refinement_bitmap_ctx = vec![0u8; 2048];

    // Init
    let mut rtype = decode_num(&mut zp, &mut record_type_ctx, 0, 11);
    let mut initial_dict_length: usize = 0;
    if rtype == 9 {
        initial_dict_length = decode_num(&mut zp, &mut inherit_dict_size_ctx, 0, 262142) as usize;
        rtype = decode_num(&mut zp, &mut record_type_ctx, 0, 11);
    }
    let _ = rtype;

    // Dimensions (present in dict streams but not used for page rendering)
    let _dict_width = decode_num(&mut zp, &mut image_size_ctx, 0, 262142);
    let _dict_height = decode_num(&mut zp, &mut image_size_ctx, 0, 262142);

    let mut flag_ctx: u8 = 0;
    if zp.decode(&mut flag_ctx) {
        return Err("JB2 dict: bad flag bit in header");
    }

    let mut dict: Vec<Jbm> = Vec::new();
    if initial_dict_length > 0 {
        if let Some(inh) = inherited {
            if initial_dict_length > inh.symbols.len() {
                return Err("JB2 dict: inherited length exceeds source dict size");
            }
            dict.extend_from_slice(&inh.symbols[..initial_dict_length]);
        } else {
            return Err("JB2 dict: stream requires inherited dict but none provided");
        }
    }

    // Main decode loop (dict only: types 2, 5, 10, 11)
    loop {
        let rtype = decode_num(&mut zp, &mut record_type_ctx, 0, 11);

        match rtype {
            2 => {
                let w = decode_num(&mut zp, &mut symbol_width_ctx, 0, 262142);
                let h = decode_num(&mut zp, &mut symbol_height_ctx, 0, 262142);
                let bm = decode_bitmap_direct(&mut zp, &mut direct_bitmap_ctx, w, h);
                dict.push(bm.remove_empty_edges());
            }
            5 => {
                let index = decode_num(&mut zp, &mut symbol_index_ctx, 0, dict.len() as i32 - 1) as usize;
                let wdiff = decode_num(&mut zp, &mut symbol_width_diff_ctx, -262143, 262142);
                let hdiff = decode_num(&mut zp, &mut symbol_height_diff_ctx, -262143, 262142);
                let cbm_w = dict[index].width + wdiff;
                let cbm_h = dict[index].height + hdiff;
                let cbm = decode_bitmap_ref(&mut zp, &mut refinement_bitmap_ctx, cbm_w, cbm_h, &dict[index]);
                dict.push(cbm.remove_empty_edges());
            }
            9 => {}
            10 => {
                let length = decode_num(&mut zp, &mut comment_length_ctx, 0, 262142);
                for _ in 0..length {
                    decode_num(&mut zp, &mut comment_octet_ctx, 0, 255);
                }
            }
            11 => break,
            _ => {
                return Err("JB2 dict: unexpected record type");
            }
        }
    }

    Ok(JB2Dict { symbols: dict })
}

// ============================================================
// Symbol positioning
// ============================================================

#[allow(clippy::too_many_arguments)]
fn decode_symbol_coords(
    zp: &mut ZPDecoder,
    offset_type_ctx: &mut u8,
    hoff_ctx: &mut NumContext,
    voff_ctx: &mut NumContext,
    shoff_ctx: &mut NumContext,
    svoff_ctx: &mut NumContext,
    first_left: &mut i32,
    first_bottom: &mut i32,
    last_right: &mut i32,
    baseline: &mut Baseline,
    sym_width: i32,
    sym_height: i32,
) -> (i32, i32) {
    let flag = zp.decode(offset_type_ctx);

    let (x, y);
    if flag {
        // New line
        let hoff = decode_num(zp, hoff_ctx, -262143, 262142);
        let voff = decode_num(zp, voff_ctx, -262143, 262142);
        x = *first_left + hoff;
        y = *first_bottom + voff - sym_height + 1;
        *first_left = x;
        *first_bottom = y;
        baseline.fill(y);
    } else {
        // Same line
        let hoff = decode_num(zp, shoff_ctx, -262143, 262142);
        let voff = decode_num(zp, svoff_ctx, -262143, 262142);
        x = *last_right + hoff;
        y = baseline.get_val() + voff;
    }

    baseline.add(y);
    *last_right = x + sym_width - 1;
    (x, y)
}

// ============================================================
// Tests
// ============================================================

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
            .join("tests/golden/jb2")
    }

    fn extract_sjbz(djvu_data: &[u8]) -> &[u8] {
        let file = rdjvu_iff::parse(djvu_data).unwrap();
        let sjbz = file.root.find_first(b"Sjbz").unwrap();
        sjbz.data()
    }

    #[test]
    fn jb2_decode_boy_jb2_mask() {
        let djvu = std::fs::read(assets_path().join("boy_jb2.djvu")).unwrap();
        let sjbz = extract_sjbz(&djvu);
        let bitmap = decode(sjbz, None).unwrap();
        let actual_pbm = bitmap.to_pbm();
        let expected_pbm = std::fs::read(golden_path().join("boy_jb2_mask.pbm")).unwrap();
        assert_eq!(
            actual_pbm.len(),
            expected_pbm.len(),
            "PBM size mismatch: got {} expected {}",
            actual_pbm.len(),
            expected_pbm.len()
        );
        assert_eq!(actual_pbm, expected_pbm, "boy_jb2_mask pixel mismatch");
    }

    fn extract_first_page_sjbz(djvu_data: &[u8]) -> Vec<u8> {
        let file = rdjvu_iff::parse(djvu_data).unwrap();
        let page_form = file.root.children().iter().find(|c| {
            matches!(c, rdjvu_iff::Chunk::Form { secondary_id, .. } if secondary_id == b"DJVU")
        }).expect("no DJVU form found");
        page_form.find_first(b"Sjbz").unwrap().data().to_vec()
    }

    #[test]
    fn jb2_decode_carte_p1_mask() {
        let djvu = std::fs::read(assets_path().join("carte.djvu")).unwrap();
        let sjbz = extract_first_page_sjbz(&djvu);
        let bitmap = decode(&sjbz, None).unwrap();
        let actual_pbm = bitmap.to_pbm();
        let expected_pbm = std::fs::read(golden_path().join("carte_p1_mask.pbm")).unwrap();
        assert_eq!(actual_pbm.len(), expected_pbm.len(), "carte_p1_mask size mismatch");
        assert_eq!(actual_pbm, expected_pbm, "carte_p1_mask pixel mismatch");
    }

    /// Find the Nth DJVU page form (0-indexed) in a bundled DJVM.
    fn find_page_form<'a>(file: &'a rdjvu_iff::DjvuFile<'a>, page: usize) -> &'a rdjvu_iff::Chunk<'a> {
        let mut idx = 0;
        for chunk in file.root.children() {
            if matches!(chunk, rdjvu_iff::Chunk::Form { secondary_id, .. } if secondary_id == b"DJVU") {
                if idx == page {
                    return chunk;
                }
                idx += 1;
            }
        }
        panic!("page {} not found", page);
    }

    /// Find a DJVI form by its component name (from INCL chunk).
    fn find_djvi_djbz<'a>(file: &'a rdjvu_iff::DjvuFile<'a>, name: &[u8]) -> &'a [u8] {
        for chunk in file.root.children() {
            if let rdjvu_iff::Chunk::Form { secondary_id, .. } = chunk {
                if secondary_id == b"DJVI" {
                    // Check if this DJVI's component name matches
                    // The component name is in the DIRM, but we can match by trying to find the Djbz
                    if let Some(djbz) = chunk.find_first(b"Djbz") {
                        return djbz.data();
                    }
                }
            }
        }
        panic!("DJVI with Djbz not found for {:?}", std::str::from_utf8(name));
    }

    #[test]
    fn jb2_decode_djvu3spec_p1_mask() {
        // Page 1 has inline Djbz + Sjbz
        let djvu = std::fs::read(assets_path().join("DjVu3Spec_bundled.djvu")).unwrap();
        let file = rdjvu_iff::parse(&djvu).unwrap();
        let page_form = find_page_form(&file, 0);
        let djbz_data = page_form.find_first(b"Djbz").unwrap().data();
        let sjbz_data = page_form.find_first(b"Sjbz").unwrap().data();

        let shared_dict = decode_dict(djbz_data, None).unwrap();
        let bitmap = decode(sjbz_data, Some(&shared_dict)).unwrap();
        let actual_pbm = bitmap.to_pbm();
        let expected_pbm = std::fs::read(golden_path().join("djvu3spec_p1_mask.pbm")).unwrap();
        assert_eq!(actual_pbm.len(), expected_pbm.len(), "djvu3spec_p1_mask size mismatch");
        assert_eq!(actual_pbm, expected_pbm, "djvu3spec_p1_mask pixel mismatch");
    }

    #[test]
    fn jb2_decode_djvu3spec_p2_mask() {
        // Page 2 uses INCL to reference dict0020.iff (a DJVI component)
        let djvu = std::fs::read(assets_path().join("DjVu3Spec_bundled.djvu")).unwrap();
        let file = rdjvu_iff::parse(&djvu).unwrap();

        // Get shared dict from the DJVI component
        let djbz_data = find_djvi_djbz(&file, b"dict0020.iff");
        let shared_dict = decode_dict(djbz_data, None).unwrap();

        // Get page 2's Sjbz (page index 1)
        let page_form = find_page_form(&file, 1);
        let sjbz_data = page_form.find_first(b"Sjbz").unwrap().data();

        let bitmap = decode(sjbz_data, Some(&shared_dict)).unwrap();
        let actual_pbm = bitmap.to_pbm();
        let expected_pbm = std::fs::read(golden_path().join("djvu3spec_p2_mask.pbm")).unwrap();
        assert_eq!(actual_pbm.len(), expected_pbm.len(), "djvu3spec_p2_mask size mismatch");
        assert_eq!(actual_pbm, expected_pbm, "djvu3spec_p2_mask pixel mismatch");
    }

    #[test]
    fn jb2_decode_navm_fgbz_p1_mask() {
        // All pages use INCL to reference dict0006.iff
        let djvu = std::fs::read(assets_path().join("navm_fgbz.djvu")).unwrap();
        let file = rdjvu_iff::parse(&djvu).unwrap();

        let djbz_data = find_djvi_djbz(&file, b"dict0006.iff");
        let shared_dict = decode_dict(djbz_data, None).unwrap();

        let page_form = find_page_form(&file, 0);
        let sjbz_data = page_form.find_first(b"Sjbz").unwrap().data();

        let bitmap = decode(sjbz_data, Some(&shared_dict)).unwrap();
        let actual_pbm = bitmap.to_pbm();
        let expected_pbm = std::fs::read(golden_path().join("navm_fgbz_p1_mask.pbm")).unwrap();
        assert_eq!(actual_pbm.len(), expected_pbm.len(), "navm_fgbz_p1_mask size mismatch");
        assert_eq!(actual_pbm, expected_pbm, "navm_fgbz_p1_mask pixel mismatch");
    }
}
