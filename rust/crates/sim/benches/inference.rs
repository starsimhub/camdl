//! Criterion benchmarks for simulation and inference hot paths.
//!
//! Models:
//!   - seir_age: 8 compartments, 6 transitions, contact matrix (table lookups)
//!     Realistic enough to stress propensity eval; small enough to iterate fast.
//!
//! Benchmarks:
//!   - step_one:         single chain-binomial step (the innermost hot function)
//!   - pfilter_100obs:   bootstrap particle filter, 100 observations, 1000 particles
//!   - eval_propensities: propensity evaluation alone (isolates expression eval)

use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use sim::chain_binomial::{step_one, StepScratch};
use sim::compiled_model::CompiledModel;
use sim::rng::StatefulRng;
use sim::inference::obs_loglik::negbin_logpmf;
use sim::inference::particle_filter::{bootstrap_filter, Observation};
use sim::propensity::eval_propensities;

/// Load the seir_age golden model and return (compiled_model, params).
/// Applies the "baseline" scenario params so the model compiles.
fn load_seir_age() -> (CompiledModel, Vec<f64>) {
    let json = include_str!("../../../../ir/golden/seir_age.ir.json");
    let mut model: ir::Model = ir::from_str(json).expect("parse seir_age");  // gh#audit-C8

    // Apply baseline scenario values to parameter defaults before compilation
    let scenario_params: &[(&str, f64)] = &[
        ("beta", 0.05), ("sigma", 0.2), ("gamma", 0.1),
    ];
    for (name, val) in scenario_params {
        if let Some(p) = model.parameters.iter_mut().find(|p| p.name == *name) {
            p.value = Some(*val);
        }
    }

    let compiled = CompiledModel::new(model).expect("compile seir_age");
    let params = compiled.default_params.clone();
    (compiled, params)
}

/// Find indices of transmission transitions.
fn transmission_indices(model: &CompiledModel) -> Vec<usize> {
    model.model.transitions.iter()
        .enumerate()
        .filter(|(_, tr)| tr.metadata.as_ref()
            .and_then(|m| m.origin_kind.as_deref())
            .map_or(false, |k| k == "transmission"))
        .map(|(i, _)| i)
        .collect()
}

/// Generate synthetic weekly observations by running a forward simulation.
/// Returns observations at t=7, 14, ..., 7*n_obs.
fn generate_observations(
    model: &CompiledModel,
    params: &[f64],
    n_obs: usize,
    seed: u64,
) -> Vec<Observation> {
    let n_tr = model.model.transitions.len();
    let (init_int, _) = model.initial_state(params).unwrap();

    let mut counts: Vec<i64> = init_int.counts.clone();
    let mut flows = vec![0u64; n_tr];
    let mut rng = StatefulRng::new(seed);
    let dt = 1.0_f64;
    let mut t = 0.0_f64;
    let mut obs = Vec::with_capacity(n_obs);

    let infection_indices = transmission_indices(model);
    let mut scratch = StepScratch::new(model);

    for week in 1..=n_obs {
        let target = week as f64 * 7.0;
        while t < target - 1e-10 {
            let step_dt = dt.min(target - t);
            step_one(model, &mut counts, &mut flows, params, t, step_dt, &mut rng, &mut scratch).unwrap();
            t += step_dt;
        }
        let incidence: f64 = infection_indices.iter()
            .map(|&i| flows[i] as f64)
            .sum();
        let reported = (incidence * 0.5_f64).max(0.0_f64);
        obs.push(Observation { time: target, value: reported });
        for f in flows.iter_mut() { *f = 0; }
    }
    obs
}

fn bench_step_one(c: &mut Criterion) {
    let (model, params) = load_seir_age();
    let n_tr = model.model.transitions.len();
    let (init_int, _) = model.initial_state(&params).unwrap();

    let mut scratch = StepScratch::new(&model);

    c.bench_function("step_one/seir_age", |b| {
        b.iter_batched(
            || {
                let counts = init_int.counts.clone();
                let flows = vec![0u64; n_tr];
                let rng = StatefulRng::new(42);
                (counts, flows, rng)
            },
            |(mut counts, mut flows, mut rng)| {
                step_one(&model, &mut counts, &mut flows, &params, 0.0, 1.0, &mut rng, &mut scratch).unwrap();
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

fn bench_eval_propensities(c: &mut Criterion) {
    let (model, params) = load_seir_age();
    let (init_int, init_real) = model.initial_state(&params).unwrap();

    c.bench_function("eval_propensities/seir_age", |b| {
        let mut out = Vec::with_capacity(model.model.transitions.len());
        b.iter(|| {
            eval_propensities(&model, &init_int, &init_real, &params, 10.0, &mut out).unwrap();
        });
    });
}

fn bench_pfilter(c: &mut Criterion) {
    let (model, params) = load_seir_age();
    let obs = generate_observations(&model, &params, 100, 99);
    let infection_indices = transmission_indices(&model);

    let rho = 0.5_f64;
    let k = 10.0_f64;

    let step_fn = |state: &mut sim::inference::ParticleState, t: f64, dt: f64, rng: &mut StatefulRng, scratch: &mut StepScratch| {
        step_one(&model, &mut state.counts, &mut state.flow_accumulators, &params, t, dt, rng, scratch)
    };
    let project_fn = |state: &sim::inference::ParticleState| -> f64 {
        infection_indices.iter().map(|&i| state.flow_accumulators[i] as f64).sum()
    };
    let obs_loglik_fn = |projected: f64, observed: f64| -> f64 {
        negbin_logpmf(observed, rho * projected, k)
    };

    let mut group = c.benchmark_group("pfilter/seir_age");
    group.sample_size(10); // pfilter is slow, fewer samples

    for &n_particles in &[100, 500, 1000] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}p", n_particles)),
            &n_particles,
            |b, &np| {
                b.iter(|| {
                    bootstrap_filter(
                        &model, &params, &obs, np, 1.0,
                        &step_fn, &project_fn, &obs_loglik_fn, None, None, 42, None,
                    ).unwrap()
                });
            },
        );
    }
    group.finish();
}

fn bench_negbin_logpmf(c: &mut Criterion) {
    c.bench_function("negbin_logpmf", |b| {
        b.iter(|| {
            // Typical inference workload: 1000 particles × 1 observation
            let mut total = 0.0_f64;
            for i in 0..1000 {
                let projected = 50.0 + i as f64 * 0.1;
                total += negbin_logpmf(100.0, 0.5 * projected, 10.0);
            }
            total
        });
    });
}

criterion_group!(
    benches,
    bench_step_one,
    bench_eval_propensities,
    bench_pfilter,
    bench_negbin_logpmf,
);
criterion_main!(benches);
