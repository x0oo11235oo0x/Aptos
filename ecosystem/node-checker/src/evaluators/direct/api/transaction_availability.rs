// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use super::{super::DirectEvaluatorInput, ApiEvaluatorError, API_CATEGORY};
use crate::{
    configuration::EvaluatorArgs,
    evaluator::{EvaluationResult, Evaluator},
    evaluators::EvaluatorType,
};
use anyhow::Result;
use aptos_rest_client::{aptos_api_types::TransactionInfo, Client as AptosRestClient, Transaction};
use clap::Parser;
use poem_openapi::Object as PoemObject;
use serde::{Deserialize, Serialize};
use std::cmp::{max, min};

const TRANSACTIONS_ENDPOINT: &str = "/transactions";

#[derive(Clone, Debug, Deserialize, Parser, PoemObject, Serialize)]
pub struct TransactionAvailabilityEvaluatorArgs {}

#[derive(Debug)]
pub struct TransactionAvailabilityEvaluator {
    #[allow(dead_code)]
    args: TransactionAvailabilityEvaluatorArgs,
}

impl TransactionAvailabilityEvaluator {
    pub fn new(args: TransactionAvailabilityEvaluatorArgs) -> Self {
        Self { args }
    }

    /// Fetch a transaction by version and return it.
    async fn get_transaction_by_version(
        client: &AptosRestClient,
        version: u64,
    ) -> Result<Transaction, ApiEvaluatorError> {
        Ok(client
            .get_transaction_by_version(version)
            .await
            .map_err(|e| {
                ApiEvaluatorError::EndpointError(
                    TRANSACTIONS_ENDPOINT.to_string(),
                    e.context(format!(
                        "The node API failed to return the requested transaction with version: {}",
                        version
                    )),
                )
            })?
            .into_inner())
    }

    /// Helper to get transaction info from a transaction.
    fn unwrap_transaction_info(
        transaction: Transaction,
    ) -> Result<TransactionInfo, ApiEvaluatorError> {
        transaction
            .transaction_info()
            .map_err(|e| {
                ApiEvaluatorError::EndpointError(
                    "/transactions".to_string(),
                    e.context("The node API returned a transaction with no info".to_string()),
                )
            })
            .map(|info| info.clone())
    }
}

#[async_trait::async_trait]
impl Evaluator for TransactionAvailabilityEvaluator {
    type Input = DirectEvaluatorInput;
    type Error = ApiEvaluatorError;

    /// Assert that the target node can produce the same transaction that the
    /// baseline produced after a delay. We confirm that the transactions are
    /// same by looking at the version.
    async fn evaluate(&self, input: &Self::Input) -> Result<Vec<EvaluationResult>, Self::Error> {
        let oldest_baseline_version = input.baseline_index_response.oldest_ledger_version.0;
        let oldest_target_version = input.target_index_response.oldest_ledger_version.0;
        let latest_baseline_version = input.baseline_index_response.ledger_version.0;
        let latest_target_version = input.target_index_response.ledger_version.0;

        // Get the oldest ledger version between the two nodes.
        let oldest_shared_version = max(oldest_baseline_version, oldest_target_version);

        // Get the least up to date latest ledger version between the two nodes.
        let latest_shared_version = min(latest_baseline_version, latest_target_version);

        // Ensure that there is a window between the oldest shared version and
        // latest shared version. If there is not, it will not be possible to
        // pull a transaction that both nodes have.
        if oldest_shared_version > latest_shared_version {
            return Ok(vec![self.build_evaluation_result(
                "Unable to pull transaction from both nodes".to_string(),
                0,
                format!(
                    "We were unable to find a ledger version window between \
                        the baseline and target nodes. The oldest and latest \
                        ledger versions on the baseline node are {} and {}. \
                        The oldest and latest ledger versions on the target \
                        node are {} and {}. This means your API cannot return \
                        a transaction that the baseline has for us to verify. \
                        Likely this means your node is too out of sync with \
                        the network, but it could also indicate an \
                        over-aggressive pruner.",
                    oldest_baseline_version,
                    latest_baseline_version,
                    oldest_target_version,
                    latest_target_version,
                ),
            )]);
        }

        // We've asserted that both nodes are sufficiently up to date relative
        // to each other, we should be able to pull the same transaction from
        // both nodes.

        let baseline_client =
            AptosRestClient::new(input.baseline_node_information.node_address.get_api_url());

        let latest_baseline_transaction_info = Self::unwrap_transaction_info(
            Self::get_transaction_by_version(&baseline_client, latest_shared_version).await?,
        )?;

        let target_client = AptosRestClient::new(input.target_node_address.get_api_url());
        let evaluation =
            match Self::get_transaction_by_version(&target_client, latest_shared_version).await {
                Ok(latest_target_transaction) => {
                    match Self::unwrap_transaction_info(latest_target_transaction) {
                        Ok(latest_target_transaction_info) => {
                            if latest_baseline_transaction_info.accumulator_root_hash
                                == latest_target_transaction_info.accumulator_root_hash
                            {
                                self.build_evaluation_result(
                                    "Target node produced valid recent transaction".to_string(),
                                    100,
                                    format!(
                                        "We were able to pull the same transaction (version: {}) \
                                    from both your node and the baseline node. Great! This \
                                    implies that your node is keeping up with other nodes \
                                    in the network.",
                                        latest_shared_version,
                                    ),
                                )
                            } else {
                                self.build_evaluation_result(
                                    "Target node produced recent transaction, but it was invalid"
                                        .to_string(),
                                    0,
                                    format!(
                                        "We were able to pull the same transaction (version: {}) \
                                    from both your node and the baseline node. However, the \
                                    transaction was invalid compared to the baseline as the \
                                    accumulator root hash of the transaction ({}) was different \
                                    compared to the baseline ({}).",
                                        latest_shared_version,
                                        latest_target_transaction_info.accumulator_root_hash,
                                        latest_baseline_transaction_info.accumulator_root_hash,
                                    ),
                                )
                            }
                        }
                        Err(error) => self.build_evaluation_result(
                            "Target node produced recent transaction, but it was missing metadata"
                                .to_string(),
                            10,
                            format!(
                                "We were able to pull the same transaction (version: {}) \
                            from both your node and the baseline node. However, the \
                            the transaction was missing metadata such as the version, \
                            accumulator root hash, etc. Error: {}",
                                latest_shared_version, error,
                            ),
                        ),
                    }
                }
                Err(error) => self.build_evaluation_result(
                    "Target node failed to produce transaction".to_string(),
                    25,
                    format!(
                        "The target node claims it has transactions between versions {} and {}, \
                    but it was unable to return the transaction with version {}. This implies \
                    something is wrong with your node's API. Error: {}",
                        oldest_target_version, latest_target_version, latest_shared_version, error,
                    ),
                ),
            };

        Ok(vec![evaluation])
    }

    fn get_category_name() -> String {
        API_CATEGORY.to_string()
    }

    fn get_evaluator_name() -> String {
        "transaction_availability".to_string()
    }

    fn from_evaluator_args(evaluator_args: &EvaluatorArgs) -> Result<Self> {
        Ok(Self::new(
            evaluator_args.transaction_availability_args.clone(),
        ))
    }

    fn evaluator_type_from_evaluator_args(evaluator_args: &EvaluatorArgs) -> Result<EvaluatorType> {
        Ok(EvaluatorType::Api(Box::new(Self::from_evaluator_args(
            evaluator_args,
        )?)))
    }
}
