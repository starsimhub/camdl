#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

// ─── ParamOverride ────────────────────────────────────────────────────────────

/// `--param NAME=VALUE`  e.g. `--param R0=2.5`
#[derive(Clone, Debug)]
pub struct ParamOverride {
    pub name:  String,
    pub value: f64,
}

impl FromStr for ParamOverride {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (name, val) = s.split_once('=')
            .ok_or_else(|| format!("expected NAME=VALUE, got '{}'", s))?;
        let value = val.parse::<f64>()
            .map_err(|_| format!("'{}' is not a valid float in --param {}", val, name))?;
        Ok(Self { name: name.to_string(), value })
    }
}

// ─── TableSpec ────────────────────────────────────────────────────────────────

/// `--table NAME=FILE`  e.g. `--table contact=matrix.tsv`
#[derive(Clone, Debug)]
pub struct TableSpec {
    pub name: String,
    pub path: PathBuf,
}

impl FromStr for TableSpec {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (name, path) = s.split_once('=')
            .ok_or_else(|| format!("expected NAME=FILE, got '{}'", s))?;
        Ok(Self { name: name.to_string(), path: PathBuf::from(path) })
    }
}

// ─── Backend ──────────────────────────────────────────────────────────────────

/// Simulation backend.  Used as a clap `ValueEnum` so `--help` lists variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Backend {
    Gillespie,
    TauLeap,
    ChainBinomial,
    Ode,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gillespie     => "gillespie",
            Self::TauLeap       => "tau_leap",
            Self::ChainBinomial => "chain_binomial",
            Self::Ode           => "ode",
        }
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ─── SeedSpec ─────────────────────────────────────────────────────────────────

/// `--seeds 1:100`  or  `--seeds 1,2,42`
#[derive(Clone, Debug)]
pub enum SeedSpec {
    Range { from: u64, to: u64 },
    List(Vec<u64>),
}

impl SeedSpec {
    pub fn expand(&self) -> Vec<u64> {
        match self {
            Self::Range { from, to } => (*from..=*to).collect(),
            Self::List(v)            => v.clone(),
        }
    }
}

impl FromStr for SeedSpec {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((a, b)) = s.split_once(':') {
            let from = a.parse::<u64>()
                .map_err(|_| format!("invalid seed range start '{}' in '{}'", a, s))?;
            let to   = b.parse::<u64>()
                .map_err(|_| format!("invalid seed range end '{}' in '{}'", b, s))?;
            if from > to {
                return Err(format!("seed range {}:{} is empty (from > to)", from, to));
            }
            Ok(Self::Range { from, to })
        } else {
            let list = s.split(',')
                .map(|t| t.trim().parse::<u64>()
                    .map_err(|_| format!("'{}' is not a valid seed in '{}'", t, s)))
                .collect::<Result<Vec<_>, _>>()?;
            if list.is_empty() {
                return Err(format!("empty seed list '{}'", s));
            }
            Ok(Self::List(list))
        }
    }
}

// ─── SweepSpec ────────────────────────────────────────────────────────────────

/// `--sweep NAME=V1,V2,...`  e.g. `--sweep beta=0.1,0.2,0.3`
#[derive(Clone, Debug)]
pub struct SweepSpec {
    pub name:   String,
    pub values: Vec<f64>,
}

impl FromStr for SweepSpec {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (name, vals) = s.split_once('=')
            .ok_or_else(|| format!("expected NAME=V1,V2,..., got '{}'", s))?;
        if name.is_empty() {
            return Err(format!("empty parameter name in --sweep '{}'", s));
        }
        let values = vals.split(',')
            .map(|v| v.trim().parse::<f64>()
                .map_err(|_| format!("'{}' is not a valid float in --sweep {}", v, name)))
            .collect::<Result<Vec<_>, _>>()?;
        if values.is_empty() {
            return Err(format!("no values for --sweep {}", name));
        }
        Ok(Self { name: name.to_string(), values })
    }
}

// ─── RwSd ─────────────────────────────────────────────────────────────────────

/// `--rw-sd auto`  or  `--rw-sd "beta=0.05,rho=0.01"`  or  `--rw-sd "beta=auto,rho=0.01"`
///
/// `None` values in the Map mean "auto" (use heuristic from parameter bounds).
#[derive(Clone, Debug)]
pub enum RwSd {
    Auto,
    Map(HashMap<String, Option<f64>>),
}

impl FromStr for RwSd {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        let mut map = HashMap::new();
        for part in s.split(',') {
            let (k, v) = part.trim().split_once('=')
                .ok_or_else(|| format!("expected NAME=VALUE in --rw-sd, got '{}'", part))?;
            let val = if v.eq_ignore_ascii_case("auto") {
                None
            } else {
                Some(v.parse::<f64>()
                    .map_err(|_| format!("'{}' is not a valid float in --rw-sd {}", v, k))?)
            };
            map.insert(k.to_string(), val);
        }
        Ok(Self::Map(map))
    }
}

// ─── ParamVecSpec ─────────────────────────────────────────────────────────────

/// `--param-vec PREFIX=FILE`  e.g. `--param-vec beta=params.tsv`
#[derive(Clone, Debug)]
pub struct ParamVecSpec {
    pub prefix: String,
    pub file:   String,
}

impl FromStr for ParamVecSpec {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (prefix, file) = s.split_once('=')
            .ok_or_else(|| format!("expected PREFIX=FILE, got '{}'", s))?;
        Ok(Self { prefix: prefix.to_string(), file: file.to_string() })
    }
}

// ─── ListDuration ─────────────────────────────────────────────────────────────

/// `--since 1h` / `30m` / `2d`  (for `camdl list`)
#[derive(Clone, Debug)]
pub struct ListDuration(pub std::time::Duration);

impl FromStr for ListDuration {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (num_str, unit) = s
            .find(|c: char| c.is_alphabetic())
            .map(|i| s.split_at(i))
            .ok_or_else(|| format!("expected duration like 1h/30m/2d, got '{}'", s))?;
        let n = num_str.parse::<u64>()
            .map_err(|_| format!("'{}' is not a valid number in duration '{}'", num_str, s))?;
        let secs = match unit {
            "s"           => n,
            "m"           => n * 60,
            "h"           => n * 3600,
            "d"           => n * 86_400,
            "w"           => n * 604_800,
            other         => return Err(format!("unknown duration unit '{}' in '{}' (use s/m/h/d/w)", other, s)),
        };
        Ok(Self(std::time::Duration::from_secs(secs)))
    }
}
