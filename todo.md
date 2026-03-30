# TODO

Updated 2026-03-29.

## Now (blocking vignette work)

- [ ] Fix rayon parallelism in `camdl if2 --chains` (currently sequential;
      rayon + closure captures need debugging)
- [ ] Install script / Makefile target: `make install` should copy camdl-sim +
      camdlc + bin/camdl to ~/.local/bin reliably

## Inference (inference-v0 branch, pre-merge)

- [ ] Validate IF2 against pomp mif2 on He et al. (vignette agent running)
- [ ] Validate pfilter loglik against pomp pfilter (--flow recovery fix landed)
- [ ] IVP parameters: perturb initial conditions only at t=0, not every obs
- [ ] IF2 JSON summary output (scout_summary.json etc.) for agent consumption
- [ ] Merge inference-v0 to main once pfilter + IF2 match pomp

## Inference (post-merge)

- [ ] PMMH (Bayesian posterior via MCMC with PF likelihood)
- [ ] Content-addressable inference output integrated with experiment system
- [ ] `camdl experiment` integration: run experiments at IF2-fitted params
- [ ] Model comparison framework (AIC/BIC from pfilter logliks)
- [ ] Multi-parameter profile grids (profile.toml with experiment-spec notation)
- [ ] Pairs plot visualization for 2D/3D profiles

## DSL

- [ ] `discretized_normal` as first-class likelihood in DSL observations block
      (currently CLI-only via --obs-model flag)
- [ ] OCaml tests for new features: overdispersed(), deterministic(), t, mod(),
      math functions, reserved names, forcing arg validation (~20 tests)
- [ ] Remove EkRng dead code (never used, CTMC incompatible)
- [ ] `and`/`or` in expression position (currently only in where guards)

## Performance

- [ ] Sparse propensity updates for Gillespie (comp_to_transitions already
      exists; need to wire into the event loop for 774-patch models)
- [ ] Profile camdl on 774-patch Nigeria model: is propensity eval the
      bottleneck?

## Documentation

- [ ] Inference workflow guide (pfilter → IF2 → profile → diagnostics)
- [ ] Update experiment spec v0.2-inference with actual inference design
- [ ] Update language spec for discretized_normal, DrawMethod enum, mod()
