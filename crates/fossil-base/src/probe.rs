//! Per-phase resident-set reporting, off unless asked for.
//!
//! Writing a ten-million-vertex corpus peaks at **17.0 GiB** for 713 MB of output
//! (`kanzo-ui/BENCHMARKS.md`, 2026-08-04), and four hypotheses about where that memory went were
//! false — each eliminated by measurement rather than argument (ADR-0043). This is the instrument
//! that eliminated them, and it lives here because both halves of the write path need it: the
//! `DataFusion` executor and Parquet sink in `fossil-df`, and the `DuckDB` layout pass in
//! `fossil-runtime`. Neither may depend on the other, and this crate is the substrate under both.
//!
//! ```text
//! FOSSIL_MEM_PROBE=1 fossil run …
//! ```
//!
//! Off, it is one relaxed load per phase. On, it shells out to `ps` per phase — which is fine at
//! this granularity (a dozen calls per run) and is the honest number, because it includes the
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
        let enabled = std::env::var("FOSSIL_MEM_PROBE").is_ok_and(|v| !v.is_empty() && v != "0");
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
    /// as much as it takes shows zero and its cost lives in [`Self::peak`] instead.
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
