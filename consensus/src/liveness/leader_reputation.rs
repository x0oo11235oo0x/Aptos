// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use crate::{
    counters::{
        COMMITTED_PROPOSALS_IN_WINDOW, COMMITTED_VOTES_IN_WINDOW, FAILED_PROPOSALS_IN_WINDOW,
        LEADER_REPUTATION_ROUND_HISTORY_SIZE,
    },
    liveness::proposer_election::{choose_index, ProposerElection},
};
use anyhow::{ensure, Result};
use aptos_bitvec::BitVec;
use aptos_infallible::{Mutex, MutexGuard};
use aptos_logger::prelude::*;
use aptos_types::{
    account_config::{new_block_event_key, NewBlockEvent},
    epoch_change::EpochChangeProof,
    epoch_state::EpochState,
};
use consensus_types::common::{Author, Round};
use short_hex_str::AsShortHexStr;
use std::{collections::HashMap, convert::TryFrom, sync::Arc};
use storage_interface::{DbReader, Order};

/// Interface to query committed NewBlockEvent.
pub trait MetadataBackend: Send + Sync {
    /// Return a contiguous NewBlockEvent window in which last one is at target_round or
    /// latest committed, return all previous one if not enough.
    fn get_block_metadata(&self, target_epoch: u64, target_round: Round) -> Vec<NewBlockEvent>;
}

pub struct AptosDBBackend {
    window_size: usize,
    seek_len: usize,
    aptos_db: Arc<dyn DbReader>,
    db_result: Mutex<(Vec<NewBlockEvent>, u64, bool)>,
}

impl AptosDBBackend {
    pub fn new(window_size: usize, seek_len: usize, aptos_db: Arc<dyn DbReader>) -> Self {
        Self {
            window_size,
            seek_len,
            aptos_db,
            db_result: Mutex::new((vec![], 0u64, true)),
        }
    }

    fn refresh_db_result(
        &self,
        mut locked: MutexGuard<'_, (Vec<NewBlockEvent>, u64, bool)>,
        lastest_db_version: u64,
    ) -> Result<(Vec<NewBlockEvent>, u64, bool)> {
        // assumes target round is not too far from latest commit
        let limit = self.window_size + self.seek_len;

        // there is a race condition between the next two lines, and new events being added.
        // I.e. when latest_db_version is fetched, and get_events are called.
        // if in between a new entry gets added max_returned_version will be larger than
        // latest_db_version, and so we should take the max of the two.

        // we cannot reorder those two functions, as if get_events is first,
        // and then new entry gets added before get_latest_version is called,
        // we would incorrectly think that we have a newer version.
        let events = self.aptos_db.get_events(
            &new_block_event_key(),
            u64::max_value(),
            Order::Descending,
            limit as u64,
            lastest_db_version,
        )?;

        let max_returned_version = events.first().map_or(0, |first| first.transaction_version);

        let new_block_events = events
            .into_iter()
            .map(|event| bcs::from_bytes::<NewBlockEvent>(event.event.event_data()))
            .collect::<Result<Vec<NewBlockEvent>, bcs::Error>>()?;

        let hit_end = new_block_events.len() < limit;

        let result = (
            new_block_events,
            std::cmp::max(lastest_db_version, max_returned_version),
            hit_end,
        );
        *locked = result.clone();
        Ok(result)
    }

    fn get_from_db_result(
        &self,
        target_epoch: u64,
        target_round: Round,
        events: &Vec<NewBlockEvent>,
        hit_end: bool,
    ) -> Vec<NewBlockEvent> {
        let has_larger = events.first().map_or(false, |e| {
            (e.epoch(), e.round()) >= (target_epoch, target_round)
        });
        if !has_larger {
            // error, and not a fatal, in an unlikely scenario that we have many failed consecutive rounds,
            // and nobody has any newer successful blocks.
            error!(
                "Local history is too old, asking for {} epoch and {} round, and latest from db is {} epoch and {} round! Elected proposers are unlikely to match!!",
                target_epoch, target_round, events.first().map_or(0, |e| e.epoch()), events.first().map_or(0, |e| e.round()))
        }

        let mut result = vec![];
        for event in events {
            if (event.epoch(), event.round()) <= (target_epoch, target_round)
                && result.len() < self.window_size
            {
                result.push(event.clone());
            }
        }

        if result.len() < self.window_size && !hit_end {
            error!(
                "We are not fetching far enough in history, we filtered from {} to {}, but asked for {}",
                events.len(),
                result.len(),
                self.window_size
            );
        }
        result
    }
}

impl MetadataBackend for AptosDBBackend {
    // assume the target_round only increases
    fn get_block_metadata(&self, target_epoch: u64, target_round: Round) -> Vec<NewBlockEvent> {
        let locked = self.db_result.lock();
        let events = &locked.0;
        let version = locked.1;
        let hit_end = locked.2;

        let has_larger = events.first().map_or(false, |e| {
            (e.epoch(), e.round()) >= (target_epoch, target_round)
        });
        let lastest_db_version = self.aptos_db.get_latest_version().unwrap_or(0);
        // check if fresher data has potential to give us different result
        if !has_larger && version < lastest_db_version {
            let fresh_db_result = self.refresh_db_result(locked, lastest_db_version);
            match fresh_db_result {
                Ok((events, _version, hit_end)) => {
                    self.get_from_db_result(target_epoch, target_round, &events, hit_end)
                }
                Err(e) => {
                    error!(
                        error = ?e, "[leader reputation] Fail to refresh window",
                    );
                    vec![]
                }
            }
        } else {
            self.get_from_db_result(target_epoch, target_round, events, hit_end)
        }
    }
}

/// Interface to calculate weights for proposers based on history.
pub trait ReputationHeuristic: Send + Sync {
    /// Return the weights of all candidates based on the history.
    fn get_weights(
        &self,
        epoch: u64,
        epoch_to_candidates: &HashMap<u64, Vec<Author>>,
        history: &[NewBlockEvent],
    ) -> Vec<u64>;
}

pub struct NewBlockEventAggregation {
    // Window sizes are in number of succesfull blocks, not number of rounds.
    // i.e. we can be looking at different number of rounds for the same window,
    // dependig on how many failures we have.
    voter_window_size: usize,
    proposer_window_size: usize,
}

impl NewBlockEventAggregation {
    pub fn new(voter_window_size: usize, proposer_window_size: usize) -> Self {
        Self {
            voter_window_size,
            proposer_window_size,
        }
    }

    pub fn bitvec_to_voters<'a>(
        validators: &'a [Author],
        bitvec: &BitVec,
    ) -> Result<Vec<&'a Author>, String> {
        if BitVec::required_buckets(validators.len() as u16) != bitvec.num_buckets() {
            return Err(format!(
                "bitvec bucket {} does not match validators len {}",
                bitvec.num_buckets(),
                validators.len()
            ));
        }

        Ok(validators
            .iter()
            .enumerate()
            .filter_map(|(index, validator)| {
                if bitvec.is_set(index as u16) {
                    Some(validator)
                } else {
                    None
                }
            })
            .collect())
    }

    pub fn indices_to_validators<'a>(
        validators: &'a [Author],
        indices: &[u64],
    ) -> Result<Vec<&'a Author>, String> {
        indices
            .iter()
            .map(|index| {
                usize::try_from(*index)
                    .map_err(|_err| format!("index {} out of bounds", index))
                    .and_then(|index| {
                        validators.get(index).ok_or(format!(
                            "index {} is larger than number of validators {}",
                            index,
                            validators.len()
                        ))
                    })
            })
            .collect()
    }

    fn history_iter<'a>(
        history: &'a [NewBlockEvent],
        epoch_to_candidates: &'a HashMap<u64, Vec<Author>>,
        window_size: usize,
    ) -> impl Iterator<Item = &'a NewBlockEvent> {
        let start = if history.len() > window_size {
            history.len() - window_size
        } else {
            0
        };

        (&history[start..])
            .iter()
            .filter(move |&meta| epoch_to_candidates.contains_key(&meta.epoch()))
    }

    pub fn get_aggregated_metrics(
        &self,
        epoch: u64,
        epoch_to_candidates: &HashMap<u64, Vec<Author>>,
        history: &[NewBlockEvent],
    ) -> (
        HashMap<Author, u32>,
        HashMap<Author, u32>,
        HashMap<Author, u32>,
    ) {
        let votes = self.count_votes(epoch_to_candidates, history);
        let proposals = self.count_proposals(epoch_to_candidates, history);
        let failed_proposals = self.count_failed_proposals(epoch_to_candidates, history);

        for candidate in &epoch_to_candidates[&epoch] {
            COMMITTED_PROPOSALS_IN_WINDOW
                .with_label_values(&[candidate.short_str().as_str()])
                .set(*proposals.get(candidate).unwrap_or(&0) as i64);
            FAILED_PROPOSALS_IN_WINDOW
                .with_label_values(&[candidate.short_str().as_str()])
                .set(*failed_proposals.get(candidate).unwrap_or(&0) as i64);
            COMMITTED_VOTES_IN_WINDOW
                .with_label_values(&[candidate.short_str().as_str()])
                .set(*votes.get(candidate).unwrap_or(&0) as i64);
        }

        LEADER_REPUTATION_ROUND_HISTORY_SIZE.set(
            proposals.values().sum::<u32>() as i64 + failed_proposals.values().sum::<u32>() as i64,
        );
        (votes, proposals, failed_proposals)
    }

    pub fn count_votes(
        &self,
        epoch_to_candidates: &HashMap<u64, Vec<Author>>,
        history: &[NewBlockEvent],
    ) -> HashMap<Author, u32> {
        Self::history_iter(history, epoch_to_candidates, self.voter_window_size).fold(
            HashMap::new(),
            |mut map, meta| {
                match Self::bitvec_to_voters(
                    &epoch_to_candidates[&meta.epoch()],
                    &meta.previous_block_votes_bitvec().clone().into(),
                ) {
                    Ok(voters) => {
                        for &voter in voters {
                            let count = map.entry(voter).or_insert(0);
                            *count += 1;
                        }
                    }
                    Err(msg) => {
                        error!(
                            "Voter conversion from bitmap failed at epoch {}, round {}: {}",
                            meta.epoch(),
                            meta.round(),
                            msg
                        )
                    }
                }
                map
            },
        )
    }

    pub fn count_proposals(
        &self,
        epoch_to_candidates: &HashMap<u64, Vec<Author>>,
        history: &[NewBlockEvent],
    ) -> HashMap<Author, u32> {
        Self::history_iter(history, epoch_to_candidates, self.proposer_window_size).fold(
            HashMap::new(),
            |mut map, meta| {
                let count = map.entry(meta.proposer()).or_insert(0);
                *count += 1;
                map
            },
        )
    }

    pub fn count_failed_proposals(
        &self,
        epoch_to_candidates: &HashMap<u64, Vec<Author>>,
        history: &[NewBlockEvent],
    ) -> HashMap<Author, u32> {
        Self::history_iter(history, epoch_to_candidates, self.proposer_window_size).fold(
            HashMap::new(),
            |mut map, meta| {
                match Self::indices_to_validators(&epoch_to_candidates[&meta.epoch()], meta.failed_proposer_indices()) {
                    Ok(failed_proposers) => {
                        for &failed_proposer in failed_proposers {
                            let count = map.entry(failed_proposer).or_insert(0);
                            *count += 1;
                        }
                    }
                    Err(msg) => {
                        error!(
                            "Failed proposer conversion from indices failed at epoch {}, round {}: {}",
                            meta.epoch(),
                            meta.round(),
                            msg
                        )
                    }
                }
                map
            },
        )
    }
}

/// Heuristic that looks at successful and failed proposals, as well as voting history,
/// to define node reputation, used for leader selection.
///
/// We want to optimize leader selection to primarily maximize network's throughput,
/// but we also, in combinatoin with staking rewards logic, need to be reasonably fair.
///
/// Logic is:
///  * if proposer round failure rate within the proposer window is strictly above threshold, use failed_weight (default 1).
///  * otherwise, if node had no proposal rounds and no successful votes, use inactive_weight (default 10).
///  * otherwise, use the default active_weight (default 100).
///
/// We primarily want to avoid failed rounds, as they have a largest negative effect on the network.
/// So if we see a node having failures to propose, when it was the leader, we want to avoid that node.
/// We add a threshold (instead of penalizing on a single failure), so that transient issues in the network,
/// or malicious behaviour of the next leader is avoided. In general, we expect there to be
/// proposer_window_size/num_validators opportunities for a node to be a leader, so a single failure, or a
/// subset of following leaders being malicious will not be enough to exclude a node.
/// On the other hand, single failure, without any successes before will exclude the note.
/// Threshold probably makes the most sense to be between:
///  * 10% (aggressive exclusion with 1 failure in 10 proposals being enough for exclusion)
///  * and 33% (much less aggressive exclusion, with 1 failure for every 2 successes, should still reduce failed
///    rounds by at least 66%, and is enough to avoid byzantine attacks as well as the rest of the protocol)
pub struct ProposerAndVoterHeuristic {
    #[allow(unused)]
    author: Author,
    active_weight: u64,
    inactive_weight: u64,
    failed_weight: u64,
    failure_threshold_percent: u32,
    aggregation: NewBlockEventAggregation,
}

impl ProposerAndVoterHeuristic {
    pub fn new(
        author: Author,
        active_weight: u64,
        inactive_weight: u64,
        failed_weight: u64,
        failure_threshold_percent: u32,
        voter_window_size: usize,
        proposer_window_size: usize,
    ) -> Self {
        Self {
            author,
            active_weight,
            inactive_weight,
            failed_weight,
            failure_threshold_percent,
            aggregation: NewBlockEventAggregation::new(voter_window_size, proposer_window_size),
        }
    }
}

impl ReputationHeuristic for ProposerAndVoterHeuristic {
    fn get_weights(
        &self,
        epoch: u64,
        epoch_to_candidates: &HashMap<u64, Vec<Author>>,
        history: &[NewBlockEvent],
    ) -> Vec<u64> {
        assert!(epoch_to_candidates.contains_key(&epoch));

        let (votes, proposals, failed_proposals) =
            self.aggregation
                .get_aggregated_metrics(epoch, epoch_to_candidates, history);

        epoch_to_candidates[&epoch]
            .iter()
            .map(|author| {
                let cur_votes = *votes.get(author).unwrap_or(&0);
                let cur_proposals = *proposals.get(author).unwrap_or(&0);
                let cur_failed_proposals = *failed_proposals.get(author).unwrap_or(&0);

                if cur_failed_proposals * 100
                    > (cur_proposals + cur_failed_proposals) * self.failure_threshold_percent
                {
                    self.failed_weight
                } else if cur_proposals > 0 || cur_votes > 0 {
                    self.active_weight
                } else {
                    self.inactive_weight
                }
            })
            .collect()
    }
}

/// Committed history based proposer election implementation that could help bias towards
/// successful leaders to help improve performance.
pub struct LeaderReputation {
    epoch: u64,
    epoch_to_proposers: HashMap<u64, Vec<Author>>,
    voting_powers: Vec<u64>,
    backend: Box<dyn MetadataBackend>,
    heuristic: Box<dyn ReputationHeuristic>,
    exclude_round: u64,
}

impl LeaderReputation {
    pub fn new(
        epoch: u64,
        epoch_to_proposers: HashMap<u64, Vec<Author>>,
        voting_powers: Vec<u64>,
        backend: Box<dyn MetadataBackend>,
        heuristic: Box<dyn ReputationHeuristic>,
        exclude_round: u64,
    ) -> Self {
        assert!(epoch_to_proposers.contains_key(&epoch));
        assert_eq!(epoch_to_proposers[&epoch].len(), voting_powers.len());

        Self {
            epoch,
            epoch_to_proposers,
            voting_powers,
            backend,
            heuristic,
            exclude_round,
        }
    }
}

impl ProposerElection for LeaderReputation {
    fn get_valid_proposer(&self, round: Round) -> Author {
        let target_round = round.saturating_sub(self.exclude_round);
        let sliding_window = self.backend.get_block_metadata(self.epoch, target_round);
        let mut weights =
            self.heuristic
                .get_weights(self.epoch, &self.epoch_to_proposers, &sliding_window);
        let proposers = &self.epoch_to_proposers[&self.epoch];
        assert_eq!(weights.len(), proposers.len());
        // Multiply weights by voting power:
        weights
            .iter_mut()
            .enumerate()
            .for_each(|(i, w)| *w *= self.voting_powers[i]);

        let state = [self.epoch.to_le_bytes(), round.to_le_bytes()]
            .concat()
            .to_vec();

        let chosen_index = choose_index(weights, state);
        proposers[chosen_index]
    }
}

pub(crate) fn extract_epoch_to_proposers_impl(
    next_epoch_states_and_cur_epoch_rounds: &[(&EpochState, u64)],
    epoch: u64,
    proposers: &[Author],
    needed_rounds: u64,
) -> Result<HashMap<u64, Vec<Author>>> {
    let last_index = next_epoch_states_and_cur_epoch_rounds.len() - 1;
    let mut num_rounds = 0;
    let mut result = HashMap::new();
    for (index, (next_epoch_state, cur_epoch_rounds)) in next_epoch_states_and_cur_epoch_rounds
        .iter()
        .enumerate()
        .rev()
    {
        let next_epoch_proposers = next_epoch_state
            .verifier
            .get_ordered_account_addresses_iter()
            .collect::<Vec<_>>();
        if index == last_index {
            ensure!(
                epoch == next_epoch_state.epoch,
                "fetched epoch_ending ledger_infos are for a wrong epoch {} vs {}",
                epoch,
                next_epoch_state.epoch
            );
            ensure!(
                proposers == next_epoch_proposers,
                "proposers from state and fetched epoch_ending ledger_infos are missaligned"
            );
        }
        result.insert(next_epoch_state.epoch, next_epoch_proposers);

        if num_rounds > needed_rounds {
            break;
        }
        // LI contains validator set for next epoch (i.e. in next_epoch_state)
        // so after adding the number of rounds in the epoch, we need to process the
        // corresponding ValidatorSet on the previous ledger info,
        // before checking if we are done.
        num_rounds += cur_epoch_rounds;
    }

    ensure!(
        result.contains_key(&epoch),
        "Current epoch ({}) not in fetched map ({:?})",
        epoch,
        result.keys().collect::<Vec<_>>()
    );
    Ok(result)
}

pub fn extract_epoch_to_proposers(
    proof: EpochChangeProof,
    epoch: u64,
    proposers: &[Author],
    needed_rounds: u64,
) -> Result<HashMap<u64, Vec<Author>>> {
    extract_epoch_to_proposers_impl(
        &proof
            .ledger_info_with_sigs
            .iter()
            .map::<Result<(&EpochState, u64)>, _>(|ledger_info| {
                let cur_epoch_rounds = ledger_info.ledger_info().round();
                let next_epoch_state = ledger_info
                    .ledger_info()
                    .next_epoch_state()
                    .ok_or_else(|| anyhow::anyhow!("no cur_epoch_state"))?;
                Ok((next_epoch_state, cur_epoch_rounds))
            })
            .collect::<Result<Vec<_>, _>>()?,
        epoch,
        proposers,
        needed_rounds,
    )
}
