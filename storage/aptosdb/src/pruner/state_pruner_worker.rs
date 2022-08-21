// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0
use crate::pruner::db_pruner::DBPruner;
use crate::pruner::state_store::StateMerklePruner;
use aptos_config::config::StateMerklePrunerConfig;
use aptos_logger::{
    error,
    prelude::{sample, SampleRate},
    sample::Sampling,
};
use aptos_types::transaction::Version;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

/// Maintains the state store pruner and periodically calls the db_pruner's prune method to prune
/// the DB. This also exposes API to report the progress to the parent thread.
#[derive(Debug)]
pub struct StatePrunerWorker {
    /// The worker will sleep for this period of time after pruning each batch.
    pruning_time_interval_in_ms: u64,
    /// State store pruner.
    pruner: Arc<StateMerklePruner>,
    /// Max items to prune per batch (i.e. the max stale nodes to prune.)
    max_node_to_prune_per_batch: u64,
    /// Indicates whether the pruning loop should be running. Will only be set to true on pruner
    /// destruction.
    quit_worker: AtomicBool,
}

impl StatePrunerWorker {
    pub(crate) fn new(
        state_pruner: Arc<StateMerklePruner>,
        state_merkle_pruner_config: StateMerklePrunerConfig,
    ) -> Self {
        Self {
            pruning_time_interval_in_ms: if cfg!(test) { 100 } else { 1 },
            pruner: state_pruner,
            max_node_to_prune_per_batch: state_merkle_pruner_config.batch_size as u64,
            quit_worker: AtomicBool::new(false),
        }
    }

    // Loop that does the real pruning job.
    pub(crate) fn work(&self) {
        while !self.quit_worker.load(Ordering::Relaxed) {
            let pruner_result = self.pruner.prune(self.max_node_to_prune_per_batch as usize);
            if pruner_result.is_err() {
                sample!(
                    SampleRate::Duration(Duration::from_secs(1)),
                    error!(error = ?pruner_result.err().unwrap(),
                        "State pruner has error.")
                );
                sleep(Duration::from_millis(self.pruning_time_interval_in_ms));
                return;
            }
            if !self.pruner.is_pruning_pending() {
                sleep(Duration::from_millis(self.pruning_time_interval_in_ms));
            }
        }
    }

    pub fn set_target_db_version(&self, target_db_version: Version) {
        assert!(target_db_version >= self.pruner.target_version());
        self.pruner.set_target_version(target_db_version);
    }

    pub fn stop_pruning(&self) {
        self.quit_worker.store(true, Ordering::Relaxed);
    }
}
