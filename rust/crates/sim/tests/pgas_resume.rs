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
                counts: vec![995, 15, 0],
                flows: vec![5, 3],
                gammas: vec![1.02],
            },
            SubstepRecord {
                counts: vec![990, 18, 2],
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
    assert_eq!(decoded.trajectory.substeps[0].counts, vec![995, 15, 0]);
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

/// T3: Large trajectory serialization (realistic size)
#[test]
fn test_resume_state_large_trajectory() {
    // Simulate a realistic trajectory size (1000 substeps, 4 compartments)
    let substeps: Vec<SubstepRecord> = (0..1000).map(|i| SubstepRecord {
        counts: vec![10000 - i as i64, i as i64, 0, 0],
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
