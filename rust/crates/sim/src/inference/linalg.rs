//! Shared linear algebra utilities for inference.

/// Cholesky decomposition: A = L L^T.
///
/// Returns `Some(L)` (lower triangular, row-major) if `a` is positive definite,
/// `None` otherwise (with a warning log).
pub fn cholesky_lower(a: &[f64], d: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0; d * d];
    for i in 0..d {
        for j in 0..=i {
            let mut sum = 0.0;
            for k in 0..j {
                sum += l[i * d + k] * l[j * d + k];
            }
            if i == j {
                let diag = a[i * d + i] - sum;
                if diag <= 0.0 {
                    log::warn!(
                        "cholesky_lower: matrix not positive definite \
                         (diag[{}] = {:.6e} after subtraction)",
                        i, diag,
                    );
                    return None;
                }
                l[i * d + j] = diag.sqrt();
            } else {
                l[i * d + j] = (a[i * d + j] - sum) / l[j * d + j];
            }
        }
    }
    Some(l)
}
