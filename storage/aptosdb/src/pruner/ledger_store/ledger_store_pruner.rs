// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use crate::pruner::pruner_metadata::{PrunerMetadata, PrunerTag};
use crate::pruner::state_store::state_value_pruner::StateValuePruner;
use crate::pruner_metadata::PrunerMetadataSchema;
use crate::{
    metrics::PRUNER_LEAST_READABLE_VERSION,
    pruner::{
        db_pruner::DBPruner,
        db_sub_pruner::DBSubPruner,
        event_store::event_store_pruner::EventStorePruner,
        transaction_store::{
            transaction_store_pruner::TransactionStorePruner, write_set_pruner::WriteSetPruner,
        },
    },
    utils, EventStore, StateStore, TransactionStore,
};
use aptos_types::transaction::{AtomicVersion, Version};
use schemadb::{SchemaBatch, DB};
use std::sync::{atomic::Ordering, Arc};

pub const LEDGER_PRUNER_NAME: &str = "ledger_pruner";

#[derive(Debug)]
/// Responsible for pruning everything except for the state tree.
pub(crate) struct LedgerPruner {
    db: Arc<DB>,
    /// Keeps track of the target version that the pruner needs to achieve.
    target_version: AtomicVersion,
    min_readable_version: AtomicVersion,
    transaction_store_pruner: Arc<dyn DBSubPruner + Send + Sync>,
    state_value_pruner: Arc<dyn DBSubPruner + Send + Sync>,
    event_store_pruner: Arc<dyn DBSubPruner + Send + Sync>,
    write_set_pruner: Arc<dyn DBSubPruner + Send + Sync>,
}

impl DBPruner for LedgerPruner {
    fn name(&self) -> &'static str {
        LEDGER_PRUNER_NAME
    }

    fn prune(&self, max_versions: usize) -> anyhow::Result<Version> {
        if !self.is_pruning_pending() {
            return Ok(self.min_readable_version());
        }

        // Collect the schema batch writes
        let mut db_batch = SchemaBatch::new();
        let current_target_version = self.prune_inner(max_versions, &mut db_batch)?;
        db_batch.put::<PrunerMetadataSchema>(
            &PrunerTag::LedgerPruner,
            &PrunerMetadata::LatestVersion(current_target_version),
        )?;
        // Commit all the changes to DB atomically
        self.db.write_schemas(db_batch)?;

        // TODO(zcc): recording progress after writing schemas might provide wrong answers to
        // API calls when they query min_readable_version while the write_schemas are still in
        // progress.
        self.record_progress(current_target_version);
        Ok(current_target_version)
    }

    fn initialize_min_readable_version(&self) -> anyhow::Result<Version> {
        Ok(self
            .db
            .get::<PrunerMetadataSchema>(&PrunerTag::LedgerPruner)?
            .map_or(0, |pruned_until_version| match pruned_until_version {
                PrunerMetadata::LatestVersion(version) => version,
            }))
    }

    fn min_readable_version(&self) -> Version {
        self.min_readable_version.load(Ordering::Relaxed)
    }

    fn set_target_version(&self, target_version: Version) {
        self.target_version.store(target_version, Ordering::Relaxed)
    }

    fn target_version(&self) -> Version {
        self.target_version.load(Ordering::Relaxed)
    }

    fn record_progress(&self, min_readable_version: Version) {
        self.min_readable_version
            .store(min_readable_version, Ordering::Relaxed);
        PRUNER_LEAST_READABLE_VERSION
            .with_label_values(&["ledger_pruner"])
            .set(min_readable_version as i64);
    }

    /// (For tests only.) Updates the minimal readable version kept by pruner.
    fn testonly_update_min_version(&self, version: Version) {
        self.min_readable_version.store(version, Ordering::Relaxed)
    }
}

impl LedgerPruner {
    pub fn new(
        db: Arc<DB>,
        transaction_store: Arc<TransactionStore>,
        event_store: Arc<EventStore>,
        state_store: Arc<StateStore>,
    ) -> Self {
        let pruner = LedgerPruner {
            db,
            target_version: AtomicVersion::new(0),
            min_readable_version: AtomicVersion::new(0),
            transaction_store_pruner: Arc::new(TransactionStorePruner::new(
                transaction_store.clone(),
            )),
            state_value_pruner: Arc::new(StateValuePruner::new(state_store)),
            event_store_pruner: Arc::new(EventStorePruner::new(event_store)),
            write_set_pruner: Arc::new(WriteSetPruner::new(transaction_store)),
        };
        pruner.initialize();
        pruner
    }

    /// Prunes the genesis transaction and saves the db alterations to the given change set
    pub fn prune_genesis(
        ledger_db: Arc<DB>,
        state_store: Arc<StateStore>,
        db_batch: &mut SchemaBatch,
    ) -> anyhow::Result<()> {
        let target_version = 1; // The genesis version is 0. Delete [0,1) (exclusive)
        let max_version = 1; // We should only be pruning a single version

        let ledger_pruner = utils::create_ledger_pruner(ledger_db, state_store);
        ledger_pruner.set_target_version(target_version);
        ledger_pruner.prune_inner(max_version, db_batch)?;

        Ok(())
    }

    fn prune_inner(
        &self,
        max_versions: usize,
        db_batch: &mut SchemaBatch,
    ) -> anyhow::Result<Version> {
        let min_readable_version = self.min_readable_version();

        // Current target version might be less than the target version to ensure we don't prune
        // more than max_version in one go.
        let current_target_version = self.get_currrent_batch_target(max_versions as Version);

        self.transaction_store_pruner.prune(
            db_batch,
            min_readable_version,
            current_target_version,
        )?;
        self.write_set_pruner
            .prune(db_batch, min_readable_version, current_target_version)?;
        self.state_value_pruner
            .prune(db_batch, min_readable_version, current_target_version)?;
        self.event_store_pruner
            .prune(db_batch, min_readable_version, current_target_version)?;

        Ok(current_target_version)
    }
}
