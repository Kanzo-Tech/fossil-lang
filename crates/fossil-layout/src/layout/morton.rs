//! Morton (Z-order) codes, and the quantisation that gets coordinates into one.
//!
//! Split out of `layout.rs` unchanged. The arithmetic is the addressing's:
//! [`morton2`] interleaves, [`morton_decode`] is its inverse over a cluster grid,
//! and [`morton_ranks`] turns codes into the renumbering the write path applies.

/// Rank each vertex by its Morton code — its position in the renumbering — and
/// the ranking's inverse.
///
/// Both, because both are used and the sort produces both. `rank[old] = new` is
/// what an adjacency's endpoints are remapped through; `order[new] = old` is the
/// gather that puts the vertex rows in the new order, and it is the sorted array
/// itself. Returning only the first and recovering the second is a second pass
/// over `n` to undo what the first line did.
///
/// Ties are broken by the old `dense_id`, so the ranking is total and the same
/// input yields the same numbering on every run. Two vertices sharing a code is
/// the common case rather than an edge case: the codes quantise to 16 bits per
/// axis, and a community packs many vertices into far less than one bucket.
pub(super) fn morton_ranks(morton: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let mut order: Vec<u32> = (0..morton.len() as u32).collect();
    order.sort_unstable_by_key(|&i| (morton[i as usize], i));
    let mut rank = vec![0u32; morton.len()];
    for (new_id, &old_id) in order.iter().enumerate() {
        rank[old_id as usize] = new_id as u32;
    }
    (rank, order)
}

/// Interleave the low 16 bits of `x` and `y` into a 32-bit Morton (Z-order)
/// code (`x` in even bits, `y` in odd). Spatially-near points get
/// near-sequential codes, so sorting vertices by it groups nearby ones into the
/// same Parquet row group — a bbox viewport query then prunes via row-group
/// min/max stats (the larger-than-RAM predicate-pushdown contract).
fn morton2(x: u16, y: u16) -> u32 {
    fn spread(n: u16) -> u32 {
        let mut n = u32::from(n);
        n = (n | (n << 8)) & 0x00ff_00ff;
        n = (n | (n << 4)) & 0x0f0f_0f0f;
        n = (n | (n << 2)) & 0x3333_3333;
        n = (n | (n << 1)) & 0x5555_5555;
        n
    }
    spread(x) | (spread(y) << 1)
}

/// One coordinate onto the `u16` grid, over `[lo, hi]`; a degenerate axis maps
/// to 0 rather than dividing by zero.
///
/// **The width is part of the contract, not an implementation detail.** The
/// subtraction, the division, the multiply by 65535 and the rounding are all
/// binary32, and a port that does the same arithmetic in binary64 disagrees:
/// `quantize(147, 0, 167)` is 57687 here and 57686 there. One unit is a
/// different Morton code, a different rank, a different `dense_id` and a
/// different tile — so the two engines that read a corpus stop agreeing about
/// which vertices are in it.
///
/// It was a closure inside [`morton_codes`] and therefore untestable, which is
/// how the writer came to be the one side of this contract nothing executed:
/// `apps/corpus/guards/vectors.json` publishes the table, `guards/arithmetic.mjs`
/// is checked against it, and until `quantize_agrees_with_the_published_table`
/// existed the Rust could have been switched to `f64` with every test in the
/// workspace still green.
fn quantize(v: f32, lo: f32, hi: f32) -> u16 {
    if hi <= lo {
        return 0;
    }
    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    (t * f32::from(u16::MAX)).round() as u16
}

/// The bounding box a vertex type's positions were quantised against.
///
/// A Morton code is `quantize(x, xlo, xhi)` interleaved with
/// `quantize(y, ylo, yhi)`, so this is the box the whole grid is relative to.
///
/// **`f32`, and that is load-bearing.** The quantisation is binary32 at every
/// step (see [`quantize`]); a reader that widens these to binary64 before
/// dividing gets a different grid cell for values near a boundary, and that is a
/// different code, a different rank and a different tile.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Extent {
    xlo: f32,
    ylo: f32,
    xhi: f32,
    yhi: f32,
}

/// The bounding box of a position list. `None` for an empty one, which has no
/// box rather than a degenerate one at the origin.
#[must_use]
fn extent_of(positions: &[(f32, f32)]) -> Option<Extent> {
    if positions.is_empty() {
        return None;
    }
    let (mut xlo, mut ylo, mut xhi, mut yhi) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in positions {
        xlo = xlo.min(x);
        ylo = ylo.min(y);
        xhi = xhi.max(x);
        yhi = yhi.max(y);
    }
    Some(Extent { xlo, ylo, xhi, yhi })
}

/// Morton codes for a position list, quantised against a **given** box.
#[must_use]
fn morton_codes_within(positions: &[(f32, f32)], extent: Extent) -> Vec<u32> {
    positions
        .iter()
        .map(|&(x, y)| {
            morton2(
                quantize(x, extent.xlo, extent.xhi),
                quantize(y, extent.ylo, extent.yhi),
            )
        })
        .collect()
}

/// Morton codes for a position list — quantises each coordinate to `u16` over
/// the list's bounding box (a degenerate axis maps to 0). Index-aligned with
/// `positions`.
#[must_use]
pub(super) fn morton_codes(positions: &[(f32, f32)]) -> Vec<u32> {
    match extent_of(positions) {
        None => Vec::new(),
        Some(extent) => morton_codes_within(positions, extent),
    }
}

/// Split a Morton code back into the two coordinates [`morton2`] interleaved.
///
/// Used to walk the cluster grid in Z-order rather than row by row. Row-major
/// numbering makes a run of consecutive clusters into a horizontal strip that
/// wraps at the edge, so a parent's children end up spread across a row and
/// broken over two; Z-order keeps a consecutive run inside a compact block, and
/// consecutive is exactly what [`order_by_hierarchy`] arranges for siblings.
pub(super) const fn morton_decode(code: u32) -> (u32, u32) {
    const fn compact(n: u32) -> u32 {
        let mut n = n & 0x5555_5555;
        n = (n | (n >> 1)) & 0x3333_3333;
        n = (n | (n >> 2)) & 0x0f0f_0f0f;
        n = (n | (n >> 4)) & 0x00ff_00ff;
        (n | (n >> 8)) & 0x0000_ffff
    }
    (compact(code), compact(code >> 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The writer side of the quantisation contract, read off the published
    /// table instead of restated beside it.
    ///
    /// `apps/corpus/guards/vectors.json` is the deliverable — a second
    /// implementation is checked against that table and not against a sentence
    /// — and `apps/corpus/guards/arithmetic.mjs` executes it. **The engine that
    /// writes the corpus did not.** Nothing in Rust read that file; the two
    /// `morton_codes` tests below assert relative ordering and no-panic, which
    /// hold for either float width. So the reference implementation was pinned
    /// to the border cases and the producer was free to drift past them.
    ///
    /// Proved red twice, and the second one is the point: with `quantize`'s
    /// arithmetic widened to `f64` (`(f64::from(v) - f64::from(lo)) / …`, the
    /// change that leaves the whole workspace green) this fails on the
    /// 147/0/167 row with `57686, want 57687`.
    ///
    /// **What it cannot prove.** That the `DuckDB` half agrees:
    /// `apps/corpus/guards/guards.mjs` spells the width `::FLOAT` in SQL, and
    /// that `FLOAT / FLOAT` stays binary32 rather than promoting is evidenced
    /// by a passing guard, not by a width proof. And it says nothing about the
    /// bounding box the coordinates are quantised against — `morton_codes`
    /// derives that from the positions it is handed, and no vector covers it.
    #[test]
    fn quantize_agrees_with_the_published_table() {
        // Widening to binary64 is the drift this guard exists to catch, so it
        // has to be computable here — otherwise the table could quietly lose
        // the one row that separates the two widths and still pass.
        fn in_binary64(v: f64, lo: f64, hi: f64) -> u16 {
            if hi <= lo {
                return 0;
            }
            let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
            (t * f64::from(u16::MAX)).round() as u16
        }

        let table = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("CARGO_MANIFEST_DIR has two parents")
            .join("apps/corpus/guards/vectors.json");
        let published: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&table)
                .unwrap_or_else(|e| panic!("read {}: {e}", table.display())),
        )
        .expect("vectors.json is JSON");

        let rows = published["quantize"]["vectors"]
            .as_array()
            .expect("vectors.json declares quantize.vectors");
        assert!(!rows.is_empty(), "the published table is empty");

        let f = |row: &serde_json::Value, key: &str| -> f64 {
            row[key]
                .as_f64()
                .unwrap_or_else(|| panic!("row {row} has no numeric {key}"))
        };

        let mut separates_the_widths = false;
        for row in rows {
            let (v, lo, hi) = (f(row, "v"), f(row, "lo"), f(row, "hi"));
            let want =
                u16::try_from(row["q"].as_u64().expect("q is an integer")).expect("q fits in u16");
            let got = quantize(v as f32, lo as f32, hi as f32);
            assert_eq!(
                got,
                want,
                "quantize({v}, {lo}, {hi}) = {got}, want {want} — {}",
                row["why"].as_str().unwrap_or("(no why)")
            );
            separates_the_widths |= in_binary64(v, lo, hi) != want;
        }

        assert!(
            separates_the_widths,
            "every published vector is exact in both float widths, so this \
             guard passes against a binary64 implementation and proves nothing \
             about the width the table calls part of the contract. Restore a \
             row that separates them — 147 over [0, 167] is 57687 in binary32 \
             and 57686 in binary64."
        );
    }

    #[test]
    fn morton2_interleaves_bits() {
        // x bits in even positions, y bits in odd. (1,0)→0b01=1; (0,1)→0b10=2;
        // (1,1)→0b11=3; (3,0)→0b0101=5.
        assert_eq!(morton2(0, 0), 0);
        assert_eq!(morton2(1, 0), 1);
        assert_eq!(morton2(0, 1), 2);
        assert_eq!(morton2(1, 1), 3);
        assert_eq!(morton2(3, 0), 5);
    }

    #[test]
    fn morton_codes_quantize_and_order_spatially() {
        // Two points near the origin get closer codes than a far one.
        let codes = morton_codes(&[(0.0, 0.0), (1.0, 1.0), (100.0, 100.0)]);
        assert_eq!(codes.len(), 3);
        assert!(codes[0] < codes[2] && codes[1] < codes[2]);
    }

    #[test]
    fn morton_codes_degenerate_axis_no_panic() {
        // All same y (range 0 on that axis) → no div-by-zero.
        let codes = morton_codes(&[(0.0, 5.0), (10.0, 5.0)]);
        assert_eq!(codes.len(), 2);
    }

    /// Z-order is what turns "consecutive" into "nearby" on the grid: the first
    /// four cells are a 2x2 block, where row-major would be a 4x1 strip.
    #[test]
    fn morton_decode_walks_the_grid_in_blocks() {
        assert_eq!(morton_decode(0), (0, 0));
        assert_eq!(morton_decode(1), (1, 0));
        assert_eq!(morton_decode(2), (0, 1));
        assert_eq!(morton_decode(3), (1, 1));
        // And it is the exact inverse of the interleave the row order uses.
        for code in 0..64u32 {
            let (x, y) = morton_decode(code);
            assert_eq!(morton2(x as u16, y as u16), code);
        }
    }
}
