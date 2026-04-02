use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, Poisson, Exp, Gamma, Binomial, StandardNormal};

/// Stateful RNG wrapping ChaCha8. Deterministic given seed.
pub struct StatefulRng(ChaCha8Rng);

impl StatefulRng {
    /// Access the underlying RNG for use with rand_distr distributions.
    pub fn inner_mut(&mut self) -> &mut ChaCha8Rng { &mut self.0 }

    pub fn new(seed: u64) -> Self {
        // Use a different derivation than EkRng so seeds don't collide
        let seed_bytes = expand_u64_to_seed(seed.wrapping_add(0xdeadbeef_cafebabe));
        StatefulRng(ChaCha8Rng::from_seed(seed_bytes))
    }

    pub fn poisson(&mut self, lambda: f64) -> u64 {
        if lambda <= 0.0 { return 0; }
        let lambda = lambda.min(1e15);
        Poisson::new(lambda).unwrap().sample(&mut self.0) as u64
    }

    pub fn exp(&mut self, rate: f64) -> f64 {
        if rate <= 0.0 { return f64::INFINITY; }
        Exp::new(rate).unwrap().sample(&mut self.0)
    }

    /// Multiplicative Gamma-Poisson compound (He et al. 2010).
    ///
    /// Draw a unit-mean Gamma multiplier G ~ Gamma(dt/σ², σ²/dt), then
    /// Poisson(mean × G).  E[count] = mean, Var[count] = mean + mean²·σ²/dt.
    /// The dt scaling ensures aggregate noise is invariant to step size:
    /// halving dt halves per-step noise but doubles the number of steps.
    pub fn neg_binomial(&mut self, mean: f64, sigma_sq: f64, dt: f64) -> u64 {
        if mean <= 0.0 || sigma_sq <= 0.0 { return self.poisson(mean); }
        let shape = dt / sigma_sq;
        // shape < 1e-6 means sigma_sq >> dt: the Gamma is degenerate
        // (nearly all mass at zero, occasional extreme spikes).
        // Fall back to Poisson (no multiplicative noise) rather than
        // producing nonsense draws. IF2 will push sigma_se away from
        // these extreme values via low likelihood.
        if shape < 1e-6 { return self.poisson(mean); }
        let scale = sigma_sq / dt;
        let g = match Gamma::new(shape, scale) {
            Ok(g) => g.sample(&mut self.0),
            Err(_) => 1.0, // fallback: no overdispersion
        };
        self.poisson(mean * g)
    }

    /// Unit-mean Gamma multiplier for overdispersed rates (He et al. 2010).
    /// G ~ Gamma(dt/σ², σ²/dt), E[G] = 1, Var[G] = σ²/dt.
    /// Used by chain-binomial to noise the rate before probability conversion.
    pub fn gamma_multiplier(&mut self, sigma_sq: f64, dt: f64) -> f64 {
        if sigma_sq <= 0.0 { return 1.0; }
        let shape = dt / sigma_sq;
        // Same degenerate guard as neg_binomial above.
        if shape < 1e-6 { return 1.0; }
        let scale = sigma_sq / dt;
        match Gamma::new(shape, scale) {
            Ok(g) => g.sample(&mut self.0),
            Err(_) => 1.0, // fallback to no noise
        }
    }

    /// Binomial(n, p) draw. Used by chain-binomial for exact multinomial
    /// competing-risk decomposition (not the Poisson approximation).
    pub fn binomial(&mut self, n: u64, p: f64) -> u64 {
        if n == 0 || p <= 0.0 { return 0; }
        if p >= 1.0 { return n; }
        Binomial::new(n, p).unwrap().sample(&mut self.0)
    }

    /// Standard normal draw N(0, 1). Used for IF2 parameter perturbations.
    pub fn normal(&mut self) -> f64 {
        StandardNormal.sample(&mut self.0)
    }

    /// Uniform [0, 1) — used for Gillespie event selection.
    pub fn uniform(&mut self) -> f64 {
        use rand::Rng;
        self.0.gen()
    }
}

fn expand_u64_to_seed(v: u64) -> [u8; 32] {
    // Fill 32 bytes from the 8-byte u64 by repeating + mixing
    let b = v.to_le_bytes();
    let b2 = v.wrapping_mul(0x9e3779b97f4a7c15).to_le_bytes();
    let b3 = v.wrapping_mul(0x6c62272e07bb0142).to_le_bytes();
    let b4 = v.wrapping_mul(0xd800000000000000).to_le_bytes();
    let mut seed = [0u8; 32];
    seed[0..8].copy_from_slice(&b);
    seed[8..16].copy_from_slice(&b2);
    seed[16..24].copy_from_slice(&b3);
    seed[24..32].copy_from_slice(&b4);
    seed
}
