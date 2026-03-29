use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, Poisson, Exp, Gamma, Binomial, StandardNormal};
use ahash::AHasher;
use std::hash::{Hash, Hasher};

/// Event-Keyed RNG. Each draw is fully determined by (seed, event_key, counter).
/// Draws are order-independent — the EKRNG placebo invariant holds.
///
/// Design: hash (seed, event_key, counter) with ahash to get a u64, then seed
/// a ChaCha8Rng from that u64. This is stateless per draw.
pub struct EkRng {
    seed: u64,
}

impl EkRng {
    pub fn new(seed: u64) -> Self {
        EkRng { seed }
    }

    fn make_rng(&self, event_key: &str, counter: u64) -> ChaCha8Rng {
        let mut hasher = AHasher::default();
        self.seed.hash(&mut hasher);
        event_key.hash(&mut hasher);
        counter.hash(&mut hasher);
        let derived = hasher.finish();
        // Expand 64-bit hash to 256-bit seed for ChaCha8
        let seed_bytes = expand_u64_to_seed(derived);
        ChaCha8Rng::from_seed(seed_bytes)
    }

    /// Draw Poisson(lambda) for a keyed event. Returns 0 for lambda ≤ 0.
    pub fn poisson_keyed(&self, event_key: &str, counter: u64, lambda: f64) -> u64 {
        if lambda <= 0.0 { return 0; }
        let mut rng = self.make_rng(event_key, counter);
        // rand_distr::Poisson panics for lambda > ~1e308; clamp to something sane
        let lambda = lambda.min(1e15);
        Poisson::new(lambda).unwrap().sample(&mut rng) as u64
    }

    /// Draw Exp(rate) for a keyed event. Returns f64::INFINITY for rate ≤ 0.
    pub fn exp_keyed(&self, event_key: &str, counter: u64, rate: f64) -> f64 {
        if rate <= 0.0 { return f64::INFINITY; }
        let mut rng = self.make_rng(event_key, counter);
        Exp::new(rate).unwrap().sample(&mut rng)
    }
}

/// Stateful RNG for transitions with no event_key (backward compatibility).
/// Separate type so callers cannot accidentally mix keyed and stateful paths.
pub struct StatefulRng(ChaCha8Rng);

impl StatefulRng {
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
        let scale = sigma_sq / dt;
        let g = Gamma::new(shape, scale).unwrap().sample(&mut self.0);
        self.poisson(mean * g)
    }

    /// Unit-mean Gamma multiplier for overdispersed rates (He et al. 2010).
    /// G ~ Gamma(dt/σ², σ²/dt), E[G] = 1, Var[G] = σ²/dt.
    /// Used by chain-binomial to noise the rate before probability conversion.
    pub fn gamma_multiplier(&mut self, sigma_sq: f64, dt: f64) -> f64 {
        if sigma_sq <= 0.0 { return 1.0; }
        let shape = dt / sigma_sq;
        let scale = sigma_sq / dt;
        Gamma::new(shape, scale).unwrap().sample(&mut self.0)
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
