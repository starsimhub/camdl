use ir::model::{OutputSchedule, RegularOutputSchedule};

/// Convert an `OutputSchedule` to a sorted list of output times.
pub fn output_times(sched: &OutputSchedule) -> Vec<f64> {
    match sched {
        OutputSchedule::Regular(RegularOutputSchedule { start, step, end }) => {
            let mut times = Vec::new();
            let mut t = *start;
            while t <= end + step * 1e-9 {
                times.push(t);
                t += step;
            }
            times
        }
        OutputSchedule::AtTimes(ts) => ts.clone(),
        OutputSchedule::MatchObservations => vec![],
    }
}
