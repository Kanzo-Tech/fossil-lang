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
//! Three decisions are argued in `/docs/design/cost`: why it is not in `fossil-base` (that `ps`
//! call was `fossil-layout`'s only edge to the compiler substrate), why the clock is `Instant`
//! (NTP steps a wall clock backwards), and why it is off on wasm32 (`Instant::now()` is
//! `unimplemented!()` there, and taking it before consulting the switch aborted every browser run).
//!
//! Off, it is a bool test per phase. On, it shells out to `ps` per phase — a dozen calls per run,
//! and the honest number, because it includes the allocator's fragmentation where a counting
//! allocator would not.

/// A phase-by-phase RSS report over one pass of work. See the module docs.
#[derive(Debug)]
pub struct Probe {
    enabled: bool,
    peak: u64,
    last: u64,
    /// The baseline [`Probe::sample`] reports against — the resident set at the previous sample,
    /// or at the start of the phase when there has been none. Distinct from `last` on purpose:
    /// a sample must not move the phase's own baseline, or the mark that closes the phase would
    /// report only its tail. See [`Probe::sample`].
    within: u64,
    /// `None` exactly when the probe is off. An off probe must not read a clock — see the module
    /// docs for the platform where reading one is a panic.
    started: Option<std::time::Instant>,
    phase_started: Option<std::time::Instant>,
    sample_started: Option<std::time::Instant>,
}

impl Probe {
    /// Reads the environment once. A run that does not ask pays a bool.
    ///
    /// Never on wasm32: there is no resident set to sample and no clock to sample it against, and
    /// taking the clock anyway is what aborted the browser executor. The switch is consulted
    /// first and the clock second, in that order, on every platform.
    #[must_use]
    pub fn new(label: &str) -> Self {
        let enabled = !cfg!(target_arch = "wasm32")
            && asked_for(std::env::var("FOSSIL_MEM_PROBE").ok().as_deref());
        let now = enabled.then(std::time::Instant::now);
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
            within: rss,
            started: now,
            phase_started: now,
            sample_started: now,
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
            elapsed(self.phase_started),
            gib(rss),
            gib(rss) - gib(self.last)
        );
        self.last = rss;
        let now = Some(std::time::Instant::now());
        self.phase_started = now;
        self.sample_started = now;
        self.within = rss;
    }

    /// Sample **inside** a phase, without closing it.
    ///
    /// A delta taken across a phase boundary cannot tell a long-lived allocation from one that is
    /// made and recycled inside the phase, and that is not a subtlety: `local_moving` looked like
    /// the layout pass's memory for two sessions on the strength of one boundary delta, and its
    /// resident set is flat to three decimal places across all thirty-two of its sweeps. The
    /// allocation that was actually there — one hash table per community in `Weighted::contract`,
    /// +3.41 GiB of the +3.82 the phase billed — was only ever visible from *within*.
    ///
    /// So a sample reports against the previous sample rather than against the phase, and it does
    /// not move the phase's baseline: the [`Self::mark`] that eventually closes the phase still
    /// reports the whole of it. The peak does rise here, which is the other half of the point — a
    /// step that takes two gigabytes and gives them back before the mark shows `+0.00G` on its
    /// phase line and its true size on this one.
    ///
    /// Indented under the phase in the report, because it is not a phase and a reader adding the
    /// column up should not find it counted twice.
    pub fn sample(&mut self, step: &str) {
        if !self.enabled {
            return;
        }
        let rss = rss_bytes();
        self.peak = self.peak.max(rss);
        eprintln!(
            "    {:<26} {:>9.1} {:>9.2}G {:>+9.2}G",
            step,
            elapsed(self.sample_started),
            gib(rss),
            gib(rss) - gib(self.within)
        );
        self.within = rss;
        self.sample_started = Some(std::time::Instant::now());
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
            elapsed(self.started),
            gib(rss),
            gib(self.peak)
        );
    }
}

/// Seconds since a mark was taken, and `0.0` for a probe that took none. Unreachable with the
/// second value — every caller is behind the `enabled` early-return — and written as a total
/// function anyway, because the alternative is an `unwrap` inside a diagnostic.
fn elapsed(since: Option<std::time::Instant>) -> f64 {
    since.map_or(0.0, |t| t.elapsed().as_secs_f64())
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

    fn off() -> Probe {
        Probe {
            enabled: false,
            peak: 0,
            last: 0,
            within: 0,
            started: None,
            phase_started: None,
            sample_started: None,
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

    /// **An off probe holds no clock**, and that is the property, not an optimisation: the one
    /// platform where the probe is always off is the one where `Instant::now()` panics, so «off»
    /// and «took no clock» have to be the same state. Asserted as the invariant rather than
    /// against a fixed answer, because `FOSSIL_MEM_PROBE` may be set in the ambient environment.
    ///
    /// What it cannot prove is the wasm half. This is a native test and there is no wasm one:
    /// `cargo xtask wasm-check` compiles without running, so the thing that would actually have
    /// caught the abort is `packages/executor`'s JS suite — and only after its gitignored `pkg/`
    /// is rebuilt.
    #[test]
    fn a_probe_holds_a_clock_exactly_when_it_is_on() {
        let p = Probe::new("the invariant");
        assert_eq!(p.started.is_some(), p.enabled);
        assert_eq!(p.phase_started.is_some(), p.enabled);
        assert!(
            !cfg!(target_arch = "wasm32") || !p.enabled,
            "there is no clock and no `ps` on wasm32; the probe cannot be on"
        );
    }

    /// A sample must be as silent as a mark when the probe is off — same claim as
    /// `an_off_probe_samples_nothing`, and worth its own test because `sample` is a second entry
    /// point that could have forgotten the early return.
    #[test]
    fn an_off_probe_samples_nothing_from_within_a_phase_either() {
        let mut p = off();
        p.sample("a step inside a phase that allocated something");
        p.finish();
        assert_eq!(p.peak, 0, "an off probe never asked the OS anything");
        assert_eq!(p.within, 0);
    }

    /// **A sample does not move the phase's baseline**, which is the whole reason it is not a
    /// mark. Sampling three times inside a phase and then closing it must report the phase's own
    /// delta, not the tail after the last sample — otherwise instrumenting a phase would silently
    /// rewrite the number the phase had been reporting all along.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_sample_leaves_the_phase_baseline_where_it_was() {
        let now = Some(std::time::Instant::now());
        let mut p = Probe {
            enabled: true,
            peak: 0,
            last: 7,
            within: 7,
            started: now,
            phase_started: now,
            sample_started: now,
        };
        p.sample("one step");
        p.sample("another");
        assert_eq!(
            p.last, 7,
            "the phase baseline is the mark's, and a sample is not one"
        );
        assert!(p.within > 0, "and the sample baseline did move");
        p.mark("the phase those two steps were inside");
        assert_eq!(p.last, p.within, "closing a phase re-bases both");
    }

    /// The peak is what a sample is *for*: a step that takes memory and gives it back before the
    /// phase closes shows nothing on the phase line and its size here. Asserted on the mechanism
    /// — a sample raises the high-water mark exactly as a mark does.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_sample_raises_the_peak() {
        let now = Some(std::time::Instant::now());
        let mut p = Probe {
            enabled: true,
            peak: 0,
            last: 0,
            within: 0,
            started: now,
            phase_started: now,
            sample_started: now,
        };
        p.sample("a step in the middle of a phase");
        assert!(
            p.peak > 0,
            "a sample is a reading of the resident set, so the peak sees it"
        );
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
        let now = Some(std::time::Instant::now());
        let mut p = Probe {
            enabled: true,
            peak: u64::MAX,
            last: 0,
            within: 0,
            started: now,
            phase_started: now,
            sample_started: now,
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
