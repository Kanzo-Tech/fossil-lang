//! Writes the `hilbert2` table of `packages/corpus/guards/vectors.json` from the
//! writer's own curve.
//!
//! ```text
//! cargo run -p fossil-layout --example hilbert_vectors
//! ```
//!
//! The inputs and why each is a border are chosen here; the answers are
//! [`fossil_layout::layout::hilbert::hilbert2`]'s and nothing else's. The block
//! is replaced in place and the rest of the file is left byte for byte, so the
//! diff after a run is the table. `hilbert2_agrees_with_the_published_table`
//! holds the result against the curve, so a table edited by hand goes red there.

use std::fmt::Write as _;
use std::path::Path;

use fossil_layout::layout::hilbert::hilbert2;

/// `(x, y, why)` — the borders a port of the curve is checked at.
const BORDERS: &[(u16, u16, &str)] = &[
    (0, 0, "the origin, where the curve starts"),
    (
        1,
        0,
        "the first step is along x. At an odd order the curve starts along y, so a port that runs it at any order but 16 answers 3 here",
    ),
    (
        1,
        1,
        "the first 2×2 block is a U and not a Z: this cell comes before (0, 1), where Morton puts it last",
    ),
    (0, 1, "the last cell of the first U"),
    (
        34,
        17,
        "an interior value with no symmetry to hide behind. A port that swaps without reflecting answers 3855",
    ),
    (4096, 4096, "a power of two on both axes"),
    (
        32768,
        32768,
        "2^31 — bit 31, negative in a signed 32-bit reading, where a port that keeps the index in a signed integer sorts the second half of the curve first",
    ),
    (
        65535,
        32767,
        "3·2^30, the first cell of the last quadrant: the one step that adds three quarters of the codomain at once, and overflows a signed accumulator",
    ),
    (
        32768,
        0,
        "the last quadrant's reflection. A port that swaps without reflecting answers 3221225472 here and 3937053354 at the row above — the two cells trade places",
    ),
    (0, 65535, "the top-left corner"),
    (65535, 65535, "the top-right corner"),
    (12345, 54321, "a second interior value, on the other side of the diagonal"),
    (65535, 0, "the last code: the curve ends at the bottom-right corner"),
];

fn main() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/corpus/guards/vectors.json")
        .canonicalize()
        .expect("packages/corpus/guards/vectors.json exists");
    let text = std::fs::read_to_string(&path).expect("read vectors.json");

    let mut block = String::from(
        "\"hilbert2\": {\n    \
         \"formula\": \"Wikipedia's xy2d at n = 2^16: for s from 2^15 down to 1, rx = (x & s) > 0, ry = (y & s) > 0, d += s·s·((3·rx) ^ ry), and when ry = 0 reflect both coordinates across the grid if rx = 1 and then swap them\",\n    \
         \"input\": \"two unsigned 16-bit quantised coordinates\",\n    \
         \"output\": \"unsigned 32-bit index on the order-16 curve; the accumulator must stay unsigned, and a single step adds up to 3·2^30\",\n    \
         \"vectors\": [\n",
    );
    for (i, &(x, y, why)) in BORDERS.iter().enumerate() {
        let comma = if i + 1 == BORDERS.len() { "" } else { "," };
        writeln!(
            block,
            "      {{ \"x\": {x}, \"y\": {y}, \"hilbert\": {}, \"why\": \"{why}\" }}{comma}",
            hilbert2(x, y)
        )
        .expect("write to a String");
    }
    block.push_str("    ]\n  }");

    let start = text
        .find("\"hilbert2\": {")
        .expect("vectors.json has a hilbert2 table to replace");
    let end = start + closing_brace(&text[start..]) + 1;
    let out = format!("{}{block}{}", &text[..start], &text[end..]);
    std::fs::write(&path, out).expect("write vectors.json");
    println!("wrote {} hilbert2 vectors to {}", BORDERS.len(), path.display());
}

/// The offset of the brace that closes the object the text opens with. No
/// value in the table holds a brace, which the borders above keep true.
fn closing_brace(text: &str) -> usize {
    let mut depth = 0usize;
    for (i, c) in text.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    panic!("the hilbert2 table is not closed");
}
