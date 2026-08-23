//! Per-phase resident-set and elapsed-time reporting, off unless asked for.
//!
//! Writing a ten-million-vertex corpus peaks at **17.0 GiB** for 713 MB of output (measured
//! 2026-08-04), and four hypotheses about where that memory went were false — each eliminated by
//! measurement rather than argument. This is the instrument that eliminated them, and both halves
//! of the write path report through it: the `DataFusion` executor and Parquet sink in `fossil-df`,
//! and the layout post-pass in `fossil-layout`. Neither may depend on the other, so it is a
//! crate of its own rather than a module inside one of them.
//!
//! ```text
//! FOSSIL_MEM_PROBE=1 fossil run …
//! ```
//!
//! # Why it is not in `fossil-base`
//!
//! It lived there, on the argument that the substrate is the thing under both halves. But it is
//! not substrate: it asks the OS for a number and reads a monotonic clock, which is exactly the
//! I/O `fossil-base`'s `System` exists to keep behind a trait — and it did neither through
//! `System`. The consequence was structural rather than aesthetic: this `ps` call was the ONLY
//! edge from `fossil-layout` to `fossil-base`, so a post-pass over Parquet linked the whole
//! compiler substrate in order to time a phase.
//!
//! # The clock is `Instant`, on purpose
//!
//! Not `System::now()`. That returns a `SystemTime` — a wall clock, which NTP may step backwards
//! mid-run, and a negative phase duration in the middle of a memory report is worse than no report.
//! `Instant` is monotonic and is what an elapsed duration is measured with. Taking the clock from
//! `System` would also mean depending on `fossil-base` again, for a number no query ever sees:
//! nothing here is inside a Salsa query, and nothing here is reproducible by construction — a
//! timing report that did not vary between two runs would be broken.
//!
//! Off, it is a bool test per phase. On, it shells out to `ps` per phase — which is fine at this
//! granularity (a dozen calls per run) and is the honest number, because it includes the
//! allocator's fragmentation where a counting allocator would not.

/// A phase-by-phase RSS report over one pass of work. See the module docs.
#[derive(Debug)]
pub struct Probe {
    enabled: bool,
    peak: u64,
    last: u64,
    started: std::time::Instant,
    phase_started: std::time::Instant,
}

impl Probe {
    /// Reads the environment once. A run that does not ask pays a bool.
    #[must_use]
    pub fn new(label: &str) -> Self {
        let enabled = asked_for(std::env::var("FOSSIL_MEM_PROBE").ok().as_deref());
        let now = std::time::Instant::now();
        let rss = if enabled { rss_bytes() } else { 0 };
        if enabled {
            eprintln!("mem probe: {label}");
            eprintln!(
                "  {:<28} {:>9} {:>10} {:>10}",
                "phase", "seconds", "RSS", "delta"
            );
            eprintln!("  {:<28} {:>9} {:>9.2}G {:>10}", "start", "", gib(rss), "");
        }
        Self {
            enabled,
            peak: rss,
            last: rss,
            started: now,
            phase_started: now,
        }
    }

    /// Close a phase and report it. The delta is against the previous mark, so a phase that frees
    /// as much as it takes shows zero and its cost lives in the `peak` field instead.
    pub fn mark(&mut self, phase: &str) {
        if !self.enabled {
            return;
        }
        let rss = rss_bytes();
        self.peak = self.peak.max(rss);
        eprintln!(
            "  {:<28} {:>9.1} {:>9.2}G {:>+9.2}G",
            phase,
            self.phase_started.elapsed().as_secs_f64(),
            gib(rss),
            gib(rss) - gib(self.last)
        );
        self.last = rss;
        self.phase_started = std::time::Instant::now();
    }

    /// Final line. Kept separate from [`Self::mark`] so the total is visible even when the last
    /// phase is cheap and the interesting number was reached three phases ago.
    pub fn finish(&mut self) {
        if !self.enabled {
            return;
        }
        let rss = rss_bytes();
        self.peak = self.peak.max(rss);
        eprintln!(
            "  {:<28} {:>9.1} {:>9.2}G  peak {:.2}G",
            "total",
            self.started.elapsed().as_secs_f64(),
            gib(rss),
            gib(self.peak)
        );
    }
}

/// Did the run ask for a report? Unset, empty and `0` are the three ways to say no; every other
/// value is a yes, including `false` — this is a presence switch, not a boolean parse, and
/// pretending otherwise would make `FOSSIL_MEM_PROBE=off` silently print.
const fn asked_for(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(v) => !v.is_empty() && !matches!(v.as_bytes(), b"0"),
    }
}

// A resident set only loses precision here past four petabytes, which is not a machine this runs on.
#[allow(clippy::cast_precision_loss)]
fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// Resident set from the OS, because that is what the machine had to find.
#[cfg(not(target_arch = "wasm32"))]
fn rss_bytes() -> u64 {
    let pid = std::process::id();
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u64>()
                .ok()
        })
        .unwrap_or(0)
        * 1024
}

/// The browser has no resident set to ask for, and no `ps` to ask with. The probe compiles there
/// so the crates that carry it stay wasm-clean; it reports nothing.
#[cfg(target_arch = "wasm32")]
fn rss_bytes() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn off() -> Probe {
        let now = Instant::now();
        Probe {
            enabled: false,
            peak: 0,
            last: 0,
            started: now,
            phase_started: now,
        }
    }

    /// The switch. `FOSSIL_MEM_PROBE=false` is ON, and that is the surprising half worth pinning:
    /// three spellings mean off and everything else means on.
    #[test]
    fn only_unset_empty_and_zero_mean_off() {
        assert!(!asked_for(None));
        assert!(!asked_for(Some("")));
        assert!(!asked_for(Some("0")));
        assert!(asked_for(Some("1")));
        assert!(asked_for(Some("00")));
        assert!(asked_for(Some("false")));
    }

    /// «A run that does not ask pays a bool» — the claim is that an off probe does not sample,
    /// not merely that it does not print. Sampling would show up as a non-zero peak.
    #[test]
    fn an_off_probe_samples_nothing() {
        let mut p = off();
        p.mark("a phase that allocated something");
        p.finish();
        assert_eq!(p.peak, 0, "an off probe never asked the OS anything");
        assert_eq!(p.last, 0);
    }

    /// The number comes from the OS, and it comes back parseable. `ps -o rss= -p <pid>` is a
    /// string contract with a program outside this repo: if the flags or the output shape ever
    /// stop matching, `rss_bytes` silently returns 0 and every report reads `0.00G`.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_resident_set_comes_back_from_the_os() {
        let rss = rss_bytes();
        assert!(rss > 0, "`ps` gave nothing back for this process");
        // A test binary holds more than a page and less than a terabyte. Both bounds exist to
        // catch a unit error — `ps` reports kibibytes, and dropping the `* 1024` would land the
        // answer under the lower one.
        assert!(rss > 1 << 20, "{rss} bytes is below one MiB");
        assert!(rss < 1 << 40, "{rss} bytes is above one TiB");
    }

    /// A mark raises the peak and never lowers it, which is what makes the peak the number the
    /// report is for: a phase that frees as much as it takes shows a zero delta and its cost
    /// survives here.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_peak_is_a_high_water_mark() {
        let now = Instant::now();
        let mut p = Probe {
            enabled: true,
            peak: u64::MAX,
            last: 0,
            started: now,
            phase_started: now,
        };
        p.mark("a phase smaller than the peak already seen");
        assert_eq!(p.peak, u64::MAX, "a smaller sample does not lower the peak");
        assert!(p.last > 0, "and the delta baseline did move");
    }

    #[test]
    fn gibibytes_are_binary_not_decimal() {
        assert!((gib(1024 * 1024 * 1024) - 1.0).abs() < f64::EPSILON);
        assert!((gib(0) - 0.0).abs() < f64::EPSILON);
    }
}
