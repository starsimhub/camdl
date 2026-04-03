/// Full version string: "camdl 0.1.0-dev+ce78a5e (2026-04-03)"
pub const VERSION: &str = concat!(
    "camdl ",
    env!("CARGO_PKG_VERSION"),
    "+", env!("CAMDL_GIT_HASH"),
    " (", env!("CAMDL_BUILD_DATE"), ")"
);

/// Short version for embedding in output files: "0.1.0+ce78a5e"
pub const VERSION_SHORT: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "+", env!("CAMDL_GIT_HASH"),
);

/// Just the git hash for comparison.
pub const GIT_HASH: &str = env!("CAMDL_GIT_HASH");
