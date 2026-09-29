//! Hilbert codes, and the quantisation that gets coordinates into one.
//!
//! The arithmetic is the addressing's: [`hilbert2`] is a quantised position's
//! index on the order-16 curve, [`hilbert_decode`] is its inverse over the
//! placement's block grid, and [`hilbert_ranks`] turns codes into the
//! renumbering the write path applies. `/docs/design/position` has why the curve
//! is Hilbert's and not Morton's.

/// Rank each vertex by its Hilbert code — its position in the renumbering — and
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
///
/// **Public because the rule it implements is published.**
/// `/docs/format/conventions/addressing` states that `dense_id` is the Hilbert
/// rank of a position, and a crate that publishes the rule and keeps the
/// function private makes every second reader write it again — which has already
/// happened once, in `crates/fossil-df/examples/tile_layout.rs`.
pub fn hilbert_ranks(codes: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let mut order: Vec<u32> = (0..codes.len() as u32).collect();
    order.sort_unstable_by_key(|&i| (codes[i as usize], i));
    let mut rank = vec![0u32; codes.len()];
    for (new_id, &old_id) in order.iter().enumerate() {
        rank[old_id as usize] = new_id as u32;
    }
    (rank, order)
}

/// One coordinate onto the `u16` grid, over `[lo, hi]`; a degenerate axis maps
/// to 0 rather than dividing by zero.
///
/// **The width is part of the contract, not an implementation detail.** The
/// subtraction, the division, the multiply by 65535 and the rounding are all
/// binary32, and a port that does the same arithmetic in binary64 disagrees:
/// `quantize(147, 0, 167)` is 57687 here and 57686 there. One unit is a
/// different Hilbert code, a different rank, a different `dense_id` and a
/// different tile — so the two engines that read a corpus stop agreeing about
/// which vertices are in it.
///
/// It was a closure inside [`hilbert_codes`] and therefore untestable, which is
/// how the writer came to be the one side of this contract nothing executed:
/// `packages/corpus/guards/vectors.json` publishes the table, `guards/arithmetic.mjs`
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
/// A Hilbert code is [`hilbert2`] of `quantize(x, xlo, xhi)` and
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

/// Hilbert codes for a position list, quantised against a **given** box.
#[must_use]
fn hilbert_codes_within(positions: &[(f32, f32)], extent: Extent) -> Vec<u32> {
    positions
        .iter()
        .map(|&(x, y)| {
            hilbert2(
                quantize(x, extent.xlo, extent.xhi),
                quantize(y, extent.ylo, extent.yhi),
            )
        })
        .collect()
}

/// Hilbert codes for a position list — quantises each coordinate to `u16` over
/// the list's bounding box (a degenerate axis maps to 0). Index-aligned with
/// `positions`.
///
/// Public for [`hilbert_ranks`]'s reason: the pair of them IS the published
/// addressing rule, and the box is the one `/docs/format` names.
#[must_use]
pub fn hilbert_codes(positions: &[(f32, f32)]) -> Vec<u32> {
    match extent_of(positions) {
        None => Vec::new(),
        Some(extent) => hilbert_codes_within(positions, extent),
    }
}

/// The index of a cell on the order-16 Hilbert curve over the `u16` grid — the
/// order `dense_id` is a rank in.
///
/// Wikipedia's `xy2d` at `n = 2^16`, which is the spelling a port is most likely
/// to start from, and `packages/corpus/guards/vectors.json` publishes its
/// borders so a port is checked against a table and not against a link.
///
/// **A quaternary, like the Z-curve it replaced**: every aligned `2^k` square is
/// one interval of `4^k` codes, so a group's buddy block is one run of ids and a
/// cell is `dense_id >> shift`. What differs is an interval that is NOT an
/// aligned square. A Morton interval that crosses a quad boundary jumps
/// diagonally across the parent, so a 4,096-row tile's box could be most of the
/// extent; a Hilbert interval is always edge-connected, so its box is never far
/// from its area. `/docs/design/position` has the measurement.
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

    /// The published table, read off the file a port is checked against.
    fn published(key: &str) -> Vec<serde_json::Value> {
        let table = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("CARGO_MANIFEST_DIR has two parents")
            .join("packages/corpus/guards/vectors.json");
        let published: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&table)
                .unwrap_or_else(|e| panic!("read {}: {e}", table.display())),
        )
        .expect("vectors.json is JSON");
        let rows = published[key]["vectors"]
            .as_array()
            .unwrap_or_else(|| panic!("vectors.json declares {key}.vectors"))
            .clone();
        assert!(!rows.is_empty(), "the published {key} table is empty");
        rows
    }

    /// The writer side of the quantisation contract, read off the published
    /// table instead of restated beside it.
    ///
    /// `packages/corpus/guards/vectors.json` is the deliverable — a second
    /// implementation is checked against that table and not against a sentence
    /// — and `packages/corpus/guards/arithmetic.mjs` executes it. **The engine that
    /// writes the corpus did not.** Nothing in Rust read that file; the two
    /// `hilbert_codes` tests below assert relative ordering and no-panic, which
    /// hold for either float width. So the reference implementation was pinned
    /// to the border cases and the producer was free to drift past them.
    ///
    /// Proved red twice, and the second one is the point: with `quantize`'s
    /// arithmetic widened to `f64` (`(f64::from(v) - f64::from(lo)) / …`, the
    /// change that leaves the whole workspace green) this fails on the
    /// 147/0/167 row with `57686, want 57687`.
    ///
    /// **What it cannot prove.** That the `DuckDB` half agrees:
    /// `packages/corpus/guards/guards.mjs` spells the width `::FLOAT` in SQL, and
    /// that `FLOAT / FLOAT` stays binary32 rather than promoting is evidenced
    /// by a passing guard, not by a width proof. And it says nothing about the
    /// bounding box the coordinates are quantised against — `hilbert_codes`
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

        let rows = published("quantize");

        let f = |row: &serde_json::Value, key: &str| -> f64 {
            row[key]
                .as_f64()
                .unwrap_or_else(|| panic!("row {row} has no numeric {key}"))
        };

        let mut separates_the_widths = false;
        for row in &rows {
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

    /// The writer's curve against the published table — the other half of the
    /// contract `quantize_agrees_with_the_published_table` holds, and the table
    /// `examples/hilbert_vectors.rs` writes. Read back rather than restated, so
    /// a table edited by hand goes red here and a curve changed here goes red
    /// against every port that copied the table.
    #[test]
    fn hilbert2_agrees_with_the_published_table() {
        for row in published("hilbert2") {
            let axis = |key: &str| -> u16 {
                u16::try_from(row[key].as_u64().expect("an integer coordinate"))
                    .expect("a coordinate fits in u16")
            };
            let (x, y) = (axis("x"), axis("y"));
            let want = u32::try_from(row["hilbert"].as_u64().expect("hilbert is an integer"))
                .expect("hilbert fits in u32");
            assert_eq!(hilbert2(x, y), want, "hilbert2({x}, {y})");
        }
    }

    /// The two halves are inverses, and an aligned square is one interval —
    /// the property the buddy placement and the cells rest on.
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
            assert_eq!(x0.abs_diff(x1) + y0.abs_diff(y1), 1, "codes {d} and {}", d + 1);
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
