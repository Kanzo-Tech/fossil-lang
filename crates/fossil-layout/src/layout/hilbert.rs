//! Hilbert codes, and the quantisation that gets coordinates into one.
//!
//! [`hilbert2`] is a quantised position's index on the order-16 curve,
//! [`hilbert_decode`] is its inverse over the placement's block grid, and
//! [`hilbert_ranks`] turns codes into the global `dense_id`. `/docs/design/position`
//! has why the curve is Hilbert's and not Morton's.
//!
//! # The same curve as `DuckDB`'s `ST_Hilbert`
//!
//! [`hilbert_codes`] is `ST_Hilbert(x, y, box)` of the spatial extension, with
//! `box` the positions' own bounding box: each axis is taken to `[0, 1]` over
//! the box by a scale of `65535 / (hi - lo)` and **truncated**, in binary64
//! from the binary32 the columns hold. So `ORDER BY ST_Hilbert(x, y, …)` over a written corpus
//! reproduces the `dense_id` order a writer chose, and a fixture can write a
//! corpus in SQL. It rounded in binary32 until 2026-09-30, which agreed with
//! `ST_Hilbert` on 504 of 2,000 points; this agrees on all of them.
//! `tests::the_codes_are_duckdbs` holds published `ST_Hilbert` outputs, and
//! `tests::the_codes_are_duckdbs_live` asks a `duckdb` binary when there is one.

/// Rank each vertex by its Hilbert code — its `dense_id` — and the ranking's
/// inverse.
///
/// Both, because both are used and the sort produces both. `rank[old] = new` is
/// what an edge's endpoints are remapped through; `order[new] = old` is the
/// gather that puts the vertex rows in the new order, and it is the sorted array
/// itself.
///
/// Ties are broken by the index a vertex had going in, so the ranking is total
/// and the same input yields the same numbering on every run. Two vertices
/// sharing a code is the common case rather than an edge case: the codes
/// quantise to 16 bits per axis, and a community packs many vertices into far
/// less than one bucket.
#[must_use]
pub fn hilbert_ranks(codes: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let mut order: Vec<u32> = (0..codes.len() as u32).collect();
    order.sort_unstable_by_key(|&i| (codes[i as usize], i));
    let mut rank = vec![0u32; codes.len()];
    for (new_id, &old_id) in order.iter().enumerate() {
        rank[old_id as usize] = new_id as u32;
    }
    (rank, order)
}

/// One coordinate onto the `u16` grid over `[lo, hi]`, as `ST_Hilbert` does
/// it: binary64, `(v - lo) · (65535 / (hi - lo))`, truncated. The order of the
/// operations is part of it — dividing first and multiplying after puts the
/// box's own maximum on 65535 where `ST_Hilbert` puts it on 65534, and a live
/// run found that one point in two thousand. A degenerate axis maps to 0 rather
/// than dividing by zero.
fn quantize(v: f64, lo: f64, hi: f64) -> u16 {
    if hi <= lo {
        return 0;
    }
    let scale = f64::from(u16::MAX) / (hi - lo);
    ((v - lo) * scale).clamp(0.0, f64::from(u16::MAX)) as u16
}

/// The bounding box of a position list, in binary64. `None` for an empty one.
fn extent_of(positions: &[(f32, f32)]) -> Option<[f64; 4]> {
    if positions.is_empty() {
        return None;
    }
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for &(x, y) in positions {
        let (x, y) = (f64::from(x), f64::from(y));
        b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    }
    Some(b)
}

/// Hilbert codes for a position list, quantised over the list's own bounding
/// box — `ST_Hilbert(x, y, box)` for each. Index-aligned with `positions`.
#[must_use]
pub fn hilbert_codes(positions: &[(f32, f32)]) -> Vec<u32> {
    let Some([xlo, ylo, xhi, yhi]) = extent_of(positions) else {
        return Vec::new();
    };
    positions
        .iter()
        .map(|&(x, y)| {
            hilbert2(
                quantize(f64::from(x), xlo, xhi),
                quantize(f64::from(y), ylo, yhi),
            )
        })
        .collect()
}

/// The index of a cell on the order-16 Hilbert curve over the `u16` grid — the
/// order `dense_id` is a rank in.
///
/// Wikipedia's `xy2d` at `n = 2^16`, which is the spelling a port is most likely
/// to start from, and the curve `DuckDB`'s `ST_Hilbert` walks.
///
/// **A quaternary, like the Z-curve it replaced**: every aligned `2^k` square is
/// one interval of `4^k` codes, so a group's buddy block is one run of ids.
/// What differs is an interval that is NOT an aligned square. A Morton interval
/// that crosses a quad boundary jumps diagonally across the parent, so a row
/// group's box could be most of the extent; a Hilbert interval is always
/// edge-connected, so its box is never far from its area.
///
/// The accumulator is `u32` and never overflows: the largest step adds
/// `3 · 2^30`, and the sum is at most `4^16 − 1`.
#[must_use]
pub fn hilbert2(x: u16, y: u16) -> u32 {
    let (mut x, mut y) = (u32::from(x), u32::from(y));
    let mut d = 0u32;
    let mut s = 1u32 << 15;
    while s > 0 {
        let rx = u32::from(x & s != 0);
        let ry = u32::from(y & s != 0);
        d += s * s * ((3 * rx) ^ ry);
        if ry == 0 {
            if rx == 1 {
                x ^= 0xffff;
                y ^= 0xffff;
            }
            std::mem::swap(&mut x, &mut y);
        }
        s >>= 1;
    }
    d
}

/// The inverse of [`hilbert2`]: the cell at index `d` of the order-16 curve.
///
/// Used to walk the placement's block grid in the order the codes will sort
/// it: consecutive blocks are then consecutive ids, and consecutive is exactly
/// what `order_by_hierarchy` arranges for siblings.
pub(super) const fn hilbert_decode(index: u32) -> (u32, u32) {
    let (mut col, mut row) = (0u32, 0u32);
    let mut rest = index;
    let mut side = 1u32;
    while side < (1 << 16) {
        let rx = 1 & (rest / 2);
        let ry = 1 & (rest ^ rx);
        if ry == 0 {
            if rx == 1 {
                col = side - 1 - col;
                row = side - 1 - row;
            }
            std::mem::swap(&mut col, &mut row);
        }
        col += side * rx;
        row += side * ry;
        rest /= 4;
        side <<= 1;
    }
    (col, row)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(x, y, ST_Hilbert(x, y, box))` with `box` the sixteen points' own
    /// extent, from `duckdb` 1.5.3 with the spatial extension, the columns
    /// `FLOAT` — what a reader of a written corpus computes.
    const DUCKDB: [(f32, f32, u32); 16] = [
        (-3.25, -2.5, 0),
        (11.75, 5.5, 2_863_311_530),
        (3.535_693_4, 1.978_179_1, 2_136_006_830),
        (10.613_159, 1.225_200_5, 3_253_068_912),
        (4.367_619, 2.199_078_6, 2_167_576_919),
        (-0.480_094_85, 1.595_269, 1_157_239_006),
        (6.198_240_8, 3.843_815, 2_657_573_546),
        (-1.838_148_1, -0.072_789_9, 957_966_223),
        (-1.889_941_9, 3.977_156_2, 1_371_578_246),
        (7.151_577_5, -2.164_957_3, 4_016_541_658),
        (11.482_902, 5.218_062_4, 2_865_883_883),
        (6.558_838, 2.424_501_7, 2_280_052_526),
        (-0.887_588_56, -2.379_994_2, 248_498_364),
        (4.675_719, -2.023_591, 3_930_103_795),
        (-0.396_876_07, -0.564_455_87, 173_216_254),
        (-2.798_761_1, 1.211_475_7, 1_065_426_848),
    ];

    #[test]
    fn the_codes_are_duckdbs() {
        let positions: Vec<(f32, f32)> = DUCKDB.iter().map(|&(x, y, _)| (x, y)).collect();
        let want: Vec<u32> = DUCKDB.iter().map(|&(_, _, h)| h).collect();
        assert_eq!(hilbert_codes(&positions), want);
    }

    /// The same claim against a live `duckdb`, over positions it did not see
    /// when the table above was written. Skipped, and says so, when there is no
    /// `duckdb` on the path or it cannot load `spatial` offline.
    #[test]
    fn the_codes_are_duckdbs_live() {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 40) as f32 / (1u64 << 24) as f32
        };
        let positions: Vec<(f32, f32)> = (0..2_000)
            .map(|_| (next() * 167.0 - 20.0, next() * 93.0 + 4.0))
            .collect();
        let values: Vec<String> = positions
            .iter()
            .enumerate()
            .map(|(i, (x, y))| format!("({i},{x:?}::FLOAT,{y:?}::FLOAT)"))
            .collect();
        let sql = format!(
            "LOAD spatial; WITH t(i, x, y) AS (VALUES {}), \
             b AS (SELECT {{'min_x': min(x), 'min_y': min(y), 'max_x': max(x), 'max_y': max(y)}}::BOX_2D AS bx FROM t) \
             SELECT ST_Hilbert(x, y, bx) FROM t, b ORDER BY i;",
            values.join(",")
        );
        let Ok(out) = std::process::Command::new("duckdb")
            .args(["-noheader", "-csv", "-c", &sql])
            .output()
        else {
            eprintln!("skipped: no `duckdb` on the path");
            return;
        };
        if !out.status.success() {
            eprintln!(
                "skipped: `duckdb` could not run ST_Hilbert: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let theirs: Vec<u32> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().parse().expect("ST_Hilbert returns a UINTEGER"))
            .collect();
        assert_eq!(theirs.len(), positions.len());
        assert_eq!(hilbert_codes(&positions), theirs);
    }

    #[test]
    fn ties_rank_by_the_index_going_in() {
        let (rank, order) = hilbert_ranks(&[5, 1, 5, 0]);
        assert_eq!(order, vec![3, 1, 0, 2]);
        assert_eq!(rank, vec![2, 1, 3, 0]);
    }

    #[test]
    fn hilbert_is_a_quaternary_bijection() {
        for d in (0..(1u32 << 20)).step_by(7) {
            let (x, y) = hilbert_decode(d);
            assert_eq!(hilbert2(x as u16, y as u16), d);
        }
        for block in 0..64u32 {
            let lo = block * 256;
            let cells: Vec<_> = (lo..lo + 256).map(hilbert_decode).collect();
            let (x0, y0) = cells
                .iter()
                .fold((u32::MAX, u32::MAX), |a, c| (a.0.min(c.0), a.1.min(c.1)));
            let (x1, y1) = cells
                .iter()
                .fold((0, 0), |a, c| (a.0.max(c.0), a.1.max(c.1)));
            assert_eq!(
                (x1 - x0, y1 - y0),
                (15, 15),
                "block {block} is a 16x16 square"
            );
            assert_eq!((x0 % 16, y0 % 16), (0, 0), "block {block} is aligned");
        }
    }

    /// What Morton could not do: consecutive codes are always edge-adjacent
    /// cells, so an interval of any length is connected and its box is never a
    /// hull over two far quads.
    #[test]
    fn consecutive_codes_are_neighbours() {
        for d in (0..(1u32 << 22)).step_by(3) {
            let (x0, y0) = hilbert_decode(d);
            let (x1, y1) = hilbert_decode(d + 1);
            assert_eq!(
                x0.abs_diff(x1) + y0.abs_diff(y1),
                1,
                "codes {d} and {}",
                d + 1
            );
        }
    }

    #[test]
    fn hilbert_codes_quantize_and_order_spatially() {
        // Two points near the origin get closer codes than a far one.
        let codes = hilbert_codes(&[(0.0, 0.0), (1.0, 1.0), (100.0, 100.0)]);
        assert_eq!(codes.len(), 3);
        assert!(codes[0] < codes[2] && codes[1] < codes[2]);
    }

    #[test]
    fn hilbert_codes_degenerate_axis_no_panic() {
        // All same y (range 0 on that axis) → no div-by-zero.
        let codes = hilbert_codes(&[(0.0, 5.0), (10.0, 5.0)]);
        assert_eq!(codes.len(), 2);
    }
}
