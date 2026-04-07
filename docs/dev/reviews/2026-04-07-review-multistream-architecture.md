---
status: closed
date: 2026-04-07
note: Refactored to ObsStreamSpec + joint_obs_weight shared across PF, PGAS, CSMC, and gradient evaluation.
---

# Multi-Stream Observation Architecture Review

The current multi-stream architecture has a fundamental interface split.
Three separate projection+weight code paths (PF, complete_data_loglik,
csmc_as) would need independent modification — fragile and error-prone.

The fix: `ObsStreamSpec` + `joint_obs_weight` — one auditable function
that all pipelines share. Single stream is `streams.len() == 1`, no
special case needed.

See `2026-04-07-review-17-resume-multistream-alpha.md` for full details.
