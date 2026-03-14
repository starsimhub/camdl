/// Per-transition firing statistics collected during a simulation run.
#[derive(Debug, Clone)]
pub struct TransitionDiagnostics {
    pub name:               String,
    pub total_firings:      u64,
    pub propensity_sum:     f64,   // sum of propensity at each firing (for mean)
    pub propensity_max:     f64,
    pub first_firing_time:  Option<f64>,
    pub last_firing_time:   Option<f64>,
}

impl TransitionDiagnostics {
    pub fn new(name: String) -> Self {
        TransitionDiagnostics {
            name,
            total_firings:     0,
            propensity_sum:    0.0,
            propensity_max:    f64::NEG_INFINITY,
            first_firing_time: None,
            last_firing_time:  None,
        }
    }

    /// Mean propensity at the time of each firing.
    pub fn mean_propensity(&self) -> f64 {
        if self.total_firings == 0 {
            0.0
        } else {
            self.propensity_sum / self.total_firings as f64
        }
    }

    /// Record a single event firing.
    pub fn record_firing(&mut self, t: f64, propensity: f64) {
        self.total_firings += 1;
        self.propensity_sum += propensity;
        if propensity > self.propensity_max {
            self.propensity_max = propensity;
        }
        if self.first_firing_time.is_none() {
            self.first_firing_time = Some(t);
        }
        self.last_firing_time = Some(t);
    }
}

/// Write a diagnostics.tsv file for the given transition diagnostics.
/// Returns the number of zero-firing transitions.
pub fn write_tsv(path: &str, diags: &[TransitionDiagnostics]) -> std::io::Result<usize> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    writeln!(f, "transition_name\ttotal_firings\tmean_propensity\tmax_propensity\tfirst_firing\tlast_firing")?;
    for d in diags {
        let mean_p = d.mean_propensity();
        let max_p  = if d.propensity_max == f64::NEG_INFINITY { 0.0 } else { d.propensity_max };
        let first  = d.first_firing_time.map_or("never".to_string(), |t| format!("{:.6}", t));
        let last   = d.last_firing_time.map_or("never".to_string(),  |t| format!("{:.6}", t));
        writeln!(f, "{}\t{}\t{:.6}\t{:.6}\t{}\t{}",
            d.name, d.total_firings, mean_p, max_p, first, last)?;
    }
    let zero_count = diags.iter().filter(|d| d.total_firings == 0).count();
    Ok(zero_count)
}

/// Print a zero-firing warning to stderr.
pub fn warn_zero_firings(diags: &[TransitionDiagnostics]) {
    let zeros: Vec<&TransitionDiagnostics> = diags.iter()
        .filter(|d| d.total_firings == 0)
        .collect();
    if zeros.is_empty() { return; }
    if zeros.len() > 20 {
        eprintln!(
            "warning: {} transitions never fired during simulation",
            zeros.len()
        );
        eprintln!("  (run `camdl inspect MODEL --transition \"...*\" --rate` to debug)");
    } else {
        eprintln!(
            "warning: {} transition{} never fired during simulation:",
            zeros.len(),
            if zeros.len() == 1 { "" } else { "s" }
        );
        for d in &zeros {
            eprintln!("  {}    (propensity always 0.0)", d.name);
        }
    }
}
