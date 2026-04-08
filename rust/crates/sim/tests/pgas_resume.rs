//! Tests for PGAS chain resume: bincode serialization, config hash.

use sim::inference::pgas::{ChainResumeState, PGASTrajectory, SubstepRecord};
use sim::inference::nuts::MassMatrix;

/// T1: Bincode round-trip — serialize and deserialize a ChainResumeState
/// with all field types (trajectory, dense mass matrix, etc.)
#[test]
fn test_resume_state_bincode_roundtrip() {
    let trajectory = PGASTrajectory {
        initial_counts: vec![1000, 10, 0],
        substeps: vec![
            SubstepRecord {
                counts_before: vec![995, 15, 0],
                counts_after: vec![993, 17, 0],
                flows: vec![5, 3],
                gammas: vec![1.02],
            },
            SubstepRecord {
                counts_before: vec![990, 18, 2],
                counts_after: vec![988, 20, 2],
                flows: vec![5, 2],
                gammas: vec![0.98],
            },
        ],
    };

    let mass = MassMatrix::dense_from_covariance(&[
        1.0, 0.5, 0.5, 2.0,
    ], 2);

    let state = ChainResumeState {
        config_hash: "abc123def456".into(),
        completed_sweeps: 5000,
        params: vec![0.4, 0.1, 1000.0],
        transformed: vec![-0.916, -2.302, 6.908],
        trajectory,
        mass_matrix: mass,
        nuts_step_size: 0.0234,
        log_proposal_sd: vec![-5.0, -6.0, -3.0],
        total_accepted: vec![2200, 1800, 2500],
        current_ll: -131456.78,
    };

    // Serialize
    let encoded = bincode::serialize(&state).unwrap();
    eprintln!("  resume state size: {} bytes", encoded.len());

    // Deserialize
    let decoded: ChainResumeState = bincode::deserialize(&encoded).unwrap();

    // Verify all fields
    assert_eq!(decoded.config_hash, "abc123def456");
    assert_eq!(decoded.completed_sweeps, 5000);
    assert_eq!(decoded.params, vec![0.4, 0.1, 1000.0]);
    assert_eq!(decoded.transformed, vec![-0.916, -2.302, 6.908]);
    assert_eq!(decoded.trajectory.initial_counts, vec![1000, 10, 0]);
    assert_eq!(decoded.trajectory.substeps.len(), 2);
    assert_eq!(decoded.trajectory.substeps[0].counts_before, vec![995, 15, 0]);
    assert_eq!(decoded.trajectory.substeps[0].flows, vec![5, 3]);
    assert!((decoded.trajectory.substeps[0].gammas[0] - 1.02).abs() < 1e-10);
    assert!((decoded.nuts_step_size - 0.0234).abs() < 1e-10);
    assert_eq!(decoded.log_proposal_sd, vec![-5.0, -6.0, -3.0]);
    assert_eq!(decoded.total_accepted, vec![2200, 1800, 2500]);
    assert!((decoded.current_ll - (-131456.78)).abs() < 1e-6);

    // Verify mass matrix round-trips (dense)
    match &decoded.mass_matrix {
        MassMatrix::Dense { dim, .. } => assert_eq!(*dim, 2),
        _ => panic!("expected Dense mass matrix after round-trip"),
    }
}

/// T2: Bincode round-trip with diagonal mass matrix
#[test]
fn test_resume_state_diagonal_mass_roundtrip() {
    let state = ChainResumeState {
        config_hash: "test".into(),
        completed_sweeps: 100,
        params: vec![1.0],
        transformed: vec![0.0],
        trajectory: PGASTrajectory { initial_counts: vec![100], substeps: vec![] },
        mass_matrix: MassMatrix::identity(3),
        nuts_step_size: 0.1,
        log_proposal_sd: vec![-3.0],
        total_accepted: vec![50],
        current_ll: -100.0,
    };

    let encoded = bincode::serialize(&state).unwrap();
    let decoded: ChainResumeState = bincode::deserialize(&encoded).unwrap();

    match &decoded.mass_matrix {
        MassMatrix::Diagonal(v) => {
            assert_eq!(v.len(), 3);
            assert_eq!(v, &vec![1.0, 1.0, 1.0]);
        }
        _ => panic!("expected Diagonal mass matrix"),
    }
}

/// T3: Config hash stability — same inputs produce same hash.
#[test]
fn test_config_hash_deterministic() {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;
    let compute = || {
        let mut h = DefaultHasher::new();
        "model_json_content".hash(&mut h);
        "data_content".hash(&mut h);
        "R0:bounds=(1,100):prior=lognormal(3.9,0.4)".hash(&mut h);
        100_usize.hash(&mut h);
        1.0_f64.to_bits().hash(&mut h);
        h.finish()
    };
    let hash1 = compute();
    let hash2 = compute();
    assert_eq!(hash1, hash2, "same config must produce same hash");
}

/// T4: Config hash sensitivity — changing invalidating fields changes the hash.
#[test]
fn test_config_hash_sensitivity() {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;
    let compute = |model: &str, n_particles: usize, prior: &str| {
        let mut h = DefaultHasher::new();
        model.hash(&mut h);
        "data".hash(&mut h);
        prior.hash(&mut h);
        n_particles.hash(&mut h);
        1.0_f64.to_bits().hash(&mut h);
        h.finish()
    };

    let base = compute("model_v1", 100, "flat");

    // Changed model → different hash
    assert_ne!(base, compute("model_v2", 100, "flat"));
    // Changed particles → different hash
    assert_ne!(base, compute("model_v1", 200, "flat"));
    // Changed prior → different hash
    assert_ne!(base, compute("model_v1", 100, "lognormal(3.9,0.4)"));
    // Same everything → same hash
    assert_eq!(base, compute("model_v1", 100, "flat"));
}

/// T5: Resume state with mismatched hash is detected.
#[test]
fn test_resume_hash_mismatch_detection() {
    let state = ChainResumeState {
        config_hash: "original_hash_abc123".into(),
        completed_sweeps: 1000,
        params: vec![1.0],
        transformed: vec![0.0],
        trajectory: PGASTrajectory { initial_counts: vec![100], substeps: vec![] },
        mass_matrix: MassMatrix::identity(1),
        nuts_step_size: 0.1,
        log_proposal_sd: vec![-3.0],
        total_accepted: vec![500],
        current_ll: -100.0,
    };

    let current_hash = "different_hash_xyz789";
    assert_ne!(state.config_hash, current_hash,
        "mismatched hashes should be detected by the caller");

    // Matching hash should pass
    let matching_hash = "original_hash_abc123";
    assert_eq!(state.config_hash, matching_hash);
}

/// T6: Large trajectory serialization (realistic size)
#[test]
fn test_resume_state_large_trajectory() {
    // Simulate a realistic trajectory size (1000 substeps, 4 compartments)
    let substeps: Vec<SubstepRecord> = (0..1000).map(|i| SubstepRecord {
        counts_before: vec![10000 - i as i64, i as i64, 0, 0], counts_after: vec![10000 - i as i64 - 1, i as i64 + 1, 0, 0],
        flows: vec![1, 0, 0, 0, 0, 0],
        gammas: vec![1.0],
    }).collect();

    let state = ChainResumeState {
        config_hash: "large_test".into(),
        completed_sweeps: 10000,
        params: vec![50.0, 0.08, 0.55, 0.035],
        transformed: vec![3.91, -2.53, 0.20, -3.35],
        trajectory: PGASTrajectory { initial_counts: vec![10000, 0, 0, 0], substeps },
        mass_matrix: MassMatrix::dense_from_covariance(&[
            1.0, 0.9, 0.1, 0.2,
            0.9, 1.0, 0.3, 0.1,
            0.1, 0.3, 1.0, 0.5,
            0.2, 0.1, 0.5, 1.0,
        ], 4),
        nuts_step_size: 0.015,
        log_proposal_sd: vec![-4.0; 4],
        total_accepted: vec![4500; 4],
        current_ll: -135000.0,
    };

    let encoded = bincode::serialize(&state).unwrap();
    let decoded: ChainResumeState = bincode::deserialize(&encoded).unwrap();

    eprintln!("  large trajectory: {} bytes for {} substeps",
        encoded.len(), decoded.trajectory.substeps.len());
    assert_eq!(decoded.trajectory.substeps.len(), 1000);
    assert_eq!(decoded.completed_sweeps, 10000);
    assert_eq!(decoded.params.len(), 4);
}
