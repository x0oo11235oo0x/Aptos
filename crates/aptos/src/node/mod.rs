// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

pub mod analyze;

use crate::common::types::{
    ConfigSearchMode, OptionalPoolAddressArgs, PromptOptions, TransactionSummary,
};
use crate::common::utils::prompt_yes_with_override;
use crate::config::GlobalConfig;
use crate::node::analyze::analyze_validators::AnalyzeValidators;
use crate::node::analyze::fetch_metadata::FetchMetadata;
use crate::{
    common::{
        types::{
            CliCommand, CliError, CliResult, CliTypedResult, ProfileOptions, RestOptions,
            TransactionOptions,
        },
        utils::read_from_file,
    },
    genesis::git::from_yaml,
};
use aptos_config::config::NodeConfig;
use aptos_crypto::{bls12381, x25519, ValidCryptoMaterialStringExt};
use aptos_faucet::FaucetArgs;
use aptos_genesis::config::{HostAndPort, OperatorConfiguration};
use aptos_types::chain_id::ChainId;
use aptos_types::{account_address::AccountAddress, account_config::CORE_CODE_ADDRESS};
use async_trait::async_trait;
use cached_packages::aptos_stdlib;
use clap::Parser;
use hex::FromHex;
use rand::rngs::StdRng;
use rand::SeedableRng;
use reqwest::Url;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::{path::PathBuf, thread, time::Duration};
use tokio::time::Instant;

/// Tool for operations related to nodes
///
/// This tool allows you to run a local test node for testing,
/// identify issues with nodes, and show related information.
#[derive(Parser)]
pub enum NodeTool {
    InitializeValidator(InitializeValidator),
    JoinValidatorSet(JoinValidatorSet),
    LeaveValidatorSet(LeaveValidatorSet),
    ShowValidatorConfig(ShowValidatorConfig),
    ShowValidatorSet(ShowValidatorSet),
    ShowValidatorStake(ShowValidatorStake),
    RunLocalTestnet(RunLocalTestnet),
    UpdateConsensusKey(UpdateConsensusKey),
    UpdateValidatorNetworkAddresses(UpdateValidatorNetworkAddresses),
    AnalyzeValidatorPerformance(AnalyzeValidatorPerformance),
}

impl NodeTool {
    pub async fn execute(self) -> CliResult {
        use NodeTool::*;
        match self {
            InitializeValidator(tool) => tool.execute_serialized().await,
            JoinValidatorSet(tool) => tool.execute_serialized().await,
            LeaveValidatorSet(tool) => tool.execute_serialized().await,
            ShowValidatorSet(tool) => tool.execute_serialized().await,
            ShowValidatorStake(tool) => tool.execute_serialized().await,
            ShowValidatorConfig(tool) => tool.execute_serialized().await,
            RunLocalTestnet(tool) => tool.execute_serialized_without_logger().await,
            UpdateConsensusKey(tool) => tool.execute_serialized().await,
            UpdateValidatorNetworkAddresses(tool) => tool.execute_serialized().await,
            AnalyzeValidatorPerformance(tool) => tool.execute_serialized().await,
        }
    }
}

#[derive(Parser)]
pub struct OperatorConfigFileArgs {
    /// Operator Configuration file, created from the `genesis set-validator-configuration` command
    #[clap(long, parse(from_os_str))]
    pub(crate) operator_config_file: Option<PathBuf>,
}

impl OperatorConfigFileArgs {
    fn load(&self) -> CliTypedResult<Option<OperatorConfiguration>> {
        if let Some(ref file) = self.operator_config_file {
            Ok(from_yaml(
                &String::from_utf8(read_from_file(file)?).map_err(CliError::from)?,
            )?)
        } else {
            Ok(None)
        }
    }
}

#[derive(Parser)]
pub struct ValidatorConsensusKeyArgs {
    /// Hex encoded Consensus public key
    #[clap(long, parse(try_from_str = bls12381::PublicKey::from_encoded_string))]
    pub(crate) consensus_public_key: Option<bls12381::PublicKey>,

    /// Hex encoded Consensus proof of possession
    #[clap(long, parse(try_from_str = bls12381::ProofOfPossession::from_encoded_string))]
    pub(crate) proof_of_possession: Option<bls12381::ProofOfPossession>,
}

impl ValidatorConsensusKeyArgs {
    fn get_consensus_public_key<'a>(
        &'a self,
        operator_config: &'a Option<OperatorConfiguration>,
    ) -> CliTypedResult<&'a bls12381::PublicKey> {
        let consensus_public_key = if let Some(ref consensus_public_key) = self.consensus_public_key
        {
            consensus_public_key
        } else if let Some(ref operator_config) = operator_config {
            &operator_config.consensus_public_key
        } else {
            return Err(CliError::CommandArgumentError(
                "Must provide either --operator-config-file or --consensus-public-key".to_string(),
            ));
        };
        Ok(consensus_public_key)
    }

    fn get_consensus_proof_of_possession<'a>(
        &'a self,
        operator_config: &'a Option<OperatorConfiguration>,
    ) -> CliTypedResult<&'a bls12381::ProofOfPossession> {
        let proof_of_possession = if let Some(ref proof_of_possession) = self.proof_of_possession {
            proof_of_possession
        } else if let Some(ref operator_config) = operator_config {
            &operator_config.consensus_proof_of_possession
        } else {
            return Err(CliError::CommandArgumentError(
                "Must provide either --operator-config-file or --proof-of-possession".to_string(),
            ));
        };
        Ok(proof_of_possession)
    }
}

#[derive(Parser)]
pub struct ValidatorNetworkAddressesArgs {
    /// Host and port pair for the validator e.g. 127.0.0.1:6180
    #[clap(long)]
    pub(crate) validator_host: Option<HostAndPort>,

    /// Validator x25519 public network key
    #[clap(long, parse(try_from_str = x25519::PublicKey::from_encoded_string))]
    pub(crate) validator_network_public_key: Option<x25519::PublicKey>,

    /// Host and port pair for the fullnode e.g. 127.0.0.1:6180.  Optional
    #[clap(long)]
    pub(crate) full_node_host: Option<HostAndPort>,

    /// Full node x25519 public network key
    #[clap(long, parse(try_from_str = x25519::PublicKey::from_encoded_string))]
    pub(crate) full_node_network_public_key: Option<x25519::PublicKey>,
}

impl ValidatorNetworkAddressesArgs {
    fn get_network_configs<'a>(
        &'a self,
        operator_config: &'a Option<OperatorConfiguration>,
    ) -> CliTypedResult<(
        x25519::PublicKey,
        Option<x25519::PublicKey>,
        &'a HostAndPort,
        Option<&'a HostAndPort>,
    )> {
        let validator_network_public_key =
            if let Some(public_key) = self.validator_network_public_key {
                public_key
            } else if let Some(ref operator_config) = operator_config {
                operator_config.validator_network_public_key
            } else {
                return Err(CliError::CommandArgumentError(
                    "Must provide either --operator-config-file or --validator-network-public-key"
                        .to_string(),
                ));
            };

        let full_node_network_public_key =
            if let Some(public_key) = self.full_node_network_public_key {
                Some(public_key)
            } else if let Some(ref operator_config) = operator_config {
                operator_config.full_node_network_public_key
            } else {
                None
            };

        let validator_host = if let Some(ref host) = self.validator_host {
            host
        } else if let Some(ref operator_config) = operator_config {
            &operator_config.validator_host
        } else {
            return Err(CliError::CommandArgumentError(
                "Must provide either --operator-config-file or --validator-host".to_string(),
            ));
        };

        let full_node_host = if let Some(ref host) = self.full_node_host {
            Some(host)
        } else if let Some(ref operator_config) = operator_config {
            operator_config.full_node_host.as_ref()
        } else {
            None
        };

        Ok((
            validator_network_public_key,
            full_node_network_public_key,
            validator_host,
            full_node_host,
        ))
    }
}

/// Register the current account as a validator node operator of it's own owned stake.
///
/// Use InitializeStakeOwner whenever stake owner
/// and validator operator are different accounts.
#[derive(Parser)]
pub struct InitializeValidator {
    #[clap(flatten)]
    pub(crate) txn_options: TransactionOptions,
    #[clap(flatten)]
    pub(crate) operator_config_file_args: OperatorConfigFileArgs,
    #[clap(flatten)]
    pub(crate) validator_consensus_key_args: ValidatorConsensusKeyArgs,
    #[clap(flatten)]
    pub(crate) validator_network_addresses_args: ValidatorNetworkAddressesArgs,
}

#[async_trait]
impl CliCommand<TransactionSummary> for InitializeValidator {
    fn command_name(&self) -> &'static str {
        "InitializeValidator"
    }

    async fn execute(mut self) -> CliTypedResult<TransactionSummary> {
        let operator_config = self.operator_config_file_args.load()?;
        let consensus_public_key = self
            .validator_consensus_key_args
            .get_consensus_public_key(&operator_config)?;
        let consensus_proof_of_possession = self
            .validator_consensus_key_args
            .get_consensus_proof_of_possession(&operator_config)?;
        let (
            validator_network_public_key,
            full_node_network_public_key,
            validator_host,
            full_node_host,
        ) = self
            .validator_network_addresses_args
            .get_network_configs(&operator_config)?;
        let validator_network_addresses =
            vec![validator_host.as_network_address(validator_network_public_key)?];
        let full_node_network_addresses =
            match (full_node_host.as_ref(), full_node_network_public_key) {
                (Some(host), Some(public_key)) => vec![host.as_network_address(public_key)?],
                (None, None) => vec![],
                _ => {
                    return Err(CliError::CommandArgumentError(
                        "If specifying fullnode addresses, both host and public key are required."
                            .to_string(),
                    ))
                }
            };

        self.txn_options
            .submit_transaction(aptos_stdlib::stake_initialize_validator(
                consensus_public_key.to_bytes().to_vec(),
                consensus_proof_of_possession.to_bytes().to_vec(),
                // BCS encode, so that we can hide the original type
                bcs::to_bytes(&validator_network_addresses)?,
                bcs::to_bytes(&full_node_network_addresses)?,
            ))
            .await
            .map(|inner| inner.into())
    }
}

/// Arguments used for operator of the staking pool
#[derive(Parser)]
pub struct OperatorArgs {
    #[clap(flatten)]
    pub(crate) pool_address_args: OptionalPoolAddressArgs,
}

impl OperatorArgs {
    fn address_fallback_to_profile(
        &self,
        profile_options: &ProfileOptions,
    ) -> CliTypedResult<AccountAddress> {
        if let Some(address) = self.pool_address_args.pool_address {
            Ok(address)
        } else {
            profile_options.account_address()
        }
    }

    fn address_fallback_to_txn(
        &self,
        transaction_options: &TransactionOptions,
    ) -> CliTypedResult<AccountAddress> {
        if let Some(address) = self.pool_address_args.pool_address {
            Ok(address)
        } else {
            transaction_options.sender_address()
        }
    }
}

/// Join the validator set after meeting staking requirements
#[derive(Parser)]
pub struct JoinValidatorSet {
    #[clap(flatten)]
    pub(crate) txn_options: TransactionOptions,
    #[clap(flatten)]
    pub(crate) operator_args: OperatorArgs,
}

#[async_trait]
impl CliCommand<TransactionSummary> for JoinValidatorSet {
    fn command_name(&self) -> &'static str {
        "JoinValidatorSet"
    }

    async fn execute(mut self) -> CliTypedResult<TransactionSummary> {
        let address = self
            .operator_args
            .address_fallback_to_txn(&self.txn_options)?;

        self.txn_options
            .submit_transaction(aptos_stdlib::stake_join_validator_set(address))
            .await
            .map(|inner| inner.into())
    }
}

/// Leave the validator set
#[derive(Parser)]
pub struct LeaveValidatorSet {
    #[clap(flatten)]
    pub(crate) txn_options: TransactionOptions,
    #[clap(flatten)]
    pub(crate) operator_args: OperatorArgs,
}

#[async_trait]
impl CliCommand<TransactionSummary> for LeaveValidatorSet {
    fn command_name(&self) -> &'static str {
        "LeaveValidatorSet"
    }

    async fn execute(mut self) -> CliTypedResult<TransactionSummary> {
        let address = self
            .operator_args
            .address_fallback_to_txn(&self.txn_options)?;

        self.txn_options
            .submit_transaction(aptos_stdlib::stake_leave_validator_set(address))
            .await
            .map(|inner| inner.into())
    }
}

/// Show validator details of the current validator
#[derive(Parser)]
pub struct ShowValidatorStake {
    #[clap(flatten)]
    pub(crate) profile_options: ProfileOptions,
    #[clap(flatten)]
    pub(crate) rest_options: RestOptions,
    #[clap(flatten)]
    pub(crate) operator_args: OperatorArgs,
}

#[async_trait]
impl CliCommand<serde_json::Value> for ShowValidatorStake {
    fn command_name(&self) -> &'static str {
        "ShowValidatorStake"
    }

    async fn execute(mut self) -> CliTypedResult<serde_json::Value> {
        let client = self.rest_options.client(&self.profile_options.profile)?;
        let address = self
            .operator_args
            .address_fallback_to_profile(&self.profile_options)?;
        let response = client
            .get_resource(address, "0x1::stake::StakePool")
            .await?;
        Ok(response.into_inner())
    }
}

/// Show validator details of the current validator
#[derive(Parser)]
pub struct ShowValidatorConfig {
    #[clap(flatten)]
    pub(crate) profile_options: ProfileOptions,
    #[clap(flatten)]
    pub(crate) rest_options: RestOptions,
    #[clap(flatten)]
    pub(crate) operator_args: OperatorArgs,
}

#[async_trait]
impl CliCommand<serde_json::Value> for ShowValidatorConfig {
    fn command_name(&self) -> &'static str {
        "ShowValidatorConfig"
    }

    async fn execute(mut self) -> CliTypedResult<serde_json::Value> {
        let client = self.rest_options.client(&self.profile_options.profile)?;
        let address = self
            .operator_args
            .address_fallback_to_profile(&self.profile_options)?;
        let response = client
            .get_resource(address, "0x1::stake::ValidatorConfig")
            .await?;
        Ok(response.into_inner())
    }
}

/// Show validator details of the validator set
#[derive(Parser)]
pub struct ShowValidatorSet {
    #[clap(flatten)]
    pub(crate) profile_options: ProfileOptions,
    #[clap(flatten)]
    pub(crate) rest_options: RestOptions,
}

#[async_trait]
impl CliCommand<serde_json::Value> for ShowValidatorSet {
    fn command_name(&self) -> &'static str {
        "ShowValidatorSet"
    }

    async fn execute(mut self) -> CliTypedResult<serde_json::Value> {
        let client = self.rest_options.client(&self.profile_options.profile)?;
        let response = client
            .get_resource(CORE_CODE_ADDRESS, "0x1::stake::ValidatorSet")
            .await?;
        Ok(response.into_inner())
    }
}

const MAX_WAIT_S: u64 = 30;
const WAIT_INTERVAL_MS: u64 = 100;
const TESTNET_FOLDER: &str = "testnet";

/// Run local testnet
///
/// This local testnet will run it's own Genesis and run as a single node
/// network locally.  Optionally, a faucet can be added for minting coins.
#[derive(Parser)]
pub struct RunLocalTestnet {
    /// An overridable config template for the test node
    #[clap(long, parse(from_os_str))]
    config_path: Option<PathBuf>,

    /// The directory to save all files for the node
    #[clap(long, parse(from_os_str))]
    test_dir: Option<PathBuf>,

    /// Random seed for key generation in test mode
    #[clap(long, parse(try_from_str = FromHex::from_hex))]
    seed: Option<[u8; 32]>,

    /// Clean the state and start with a new chain at genesis
    #[clap(long)]
    force_restart: bool,

    /// Run a faucet alongside the node
    #[clap(long)]
    with_faucet: bool,

    /// Port to run the faucet on
    #[clap(long, default_value = "8081")]
    faucet_port: u16,

    #[clap(flatten)]
    prompt_options: PromptOptions,

    /// Disable the delegation of minting to a dedicated account
    #[clap(long)]
    do_not_delegate: bool,
}

#[async_trait]
impl CliCommand<()> for RunLocalTestnet {
    fn command_name(&self) -> &'static str {
        "RunLocalTestnet"
    }

    async fn execute(mut self) -> CliTypedResult<()> {
        let rng = self
            .seed
            .map(StdRng::from_seed)
            .unwrap_or_else(StdRng::from_entropy);

        let global_config = GlobalConfig::load()?;
        let test_dir = global_config
            .get_config_location(ConfigSearchMode::CurrentDirAndParents)?
            .join(TESTNET_FOLDER);

        // Remove the current test directory and start with a new node
        if self.force_restart && test_dir.exists() {
            prompt_yes_with_override(
                "Are you sure you want to delete the existing chain?",
                self.prompt_options,
            )?;
            std::fs::remove_dir_all(test_dir.as_path()).map_err(|err| {
                CliError::IO(format!("Failed to delete {}", test_dir.display()), err)
            })?;
        }

        // Spawn the node in a separate thread
        let config_path = self.config_path.clone();
        let test_dir_copy = test_dir.clone();
        let _node = thread::spawn(move || {
            aptos_node::load_test_environment(
                config_path,
                Some(test_dir_copy),
                false,
                false,
                cached_packages::head_release_bundle(),
                rng,
            )
            .map_err(|err| CliError::UnexpectedError(format!("Node failed to run {}", err)))
        });

        // Run faucet if selected
        let _maybe_faucet = if self.with_faucet {
            let max_wait = Duration::from_secs(MAX_WAIT_S);
            let wait_interval = Duration::from_millis(WAIT_INTERVAL_MS);

            // Load the config to get the rest port
            let config_path = test_dir.join("0").join("node.yaml");

            // We have to wait for the node to be configured above in the other thread
            let mut config = None;
            let start = Instant::now();
            while start.elapsed() < max_wait {
                if let Ok(loaded_config) = NodeConfig::load(&config_path) {
                    config = Some(loaded_config);
                    break;
                }
                tokio::time::sleep(wait_interval).await;
            }

            // Retrieve the port from the local node
            let port = if let Some(config) = config {
                config.api.address.port()
            } else {
                return Err(CliError::UnexpectedError(
                    "Failed to find node configuration to start faucet".to_string(),
                ));
            };

            // Check that the REST API is ready
            let rest_url = Url::parse(&format!("http://localhost:{}", port)).map_err(|err| {
                CliError::UnexpectedError(format!("Failed to parse localhost URL {}", err))
            })?;
            let rest_client = aptos_rest_client::Client::new(rest_url.clone());
            let start = Instant::now();
            let mut started_successfully = false;

            while start.elapsed() < max_wait {
                if rest_client.get_index().await.is_ok() {
                    started_successfully = true;
                    break;
                }
                tokio::time::sleep(wait_interval).await
            }

            if !started_successfully {
                return Err(CliError::UnexpectedError(
                    "Failed to startup local node before faucet".to_string(),
                ));
            }

            // Start the faucet
            FaucetArgs {
                address: "0.0.0.0".to_string(),
                port: self.faucet_port,
                server_url: rest_url,
                mint_key_file_path: test_dir.join("mint.key"),
                mint_key: None,
                mint_account_address: None,
                chain_id: ChainId::test(),
                maximum_amount: None,
                do_not_delegate: self.do_not_delegate,
            }
            .run()
            .await;
            Some(())
        } else {
            None
        };

        // Wait for an interrupt
        let term = Arc::new(AtomicBool::new(false));
        while !term.load(Ordering::Acquire) {
            std::thread::park();
        }
        Ok(())
    }
}

/// Update consensus key for the validator node.
#[derive(Parser)]
pub struct UpdateConsensusKey {
    #[clap(flatten)]
    pub(crate) txn_options: TransactionOptions,
    #[clap(flatten)]
    pub(crate) operator_args: OperatorArgs,
    #[clap(flatten)]
    pub(crate) operator_config_file_args: OperatorConfigFileArgs,
    #[clap(flatten)]
    pub(crate) validator_consensus_key_args: ValidatorConsensusKeyArgs,
}

#[async_trait]
impl CliCommand<TransactionSummary> for UpdateConsensusKey {
    fn command_name(&self) -> &'static str {
        "UpdateConsensusKey"
    }

    async fn execute(mut self) -> CliTypedResult<TransactionSummary> {
        let address = self
            .operator_args
            .address_fallback_to_txn(&self.txn_options)?;

        let operator_config = self.operator_config_file_args.load()?;
        let consensus_public_key = self
            .validator_consensus_key_args
            .get_consensus_public_key(&operator_config)?;
        let consensus_proof_of_possession = self
            .validator_consensus_key_args
            .get_consensus_proof_of_possession(&operator_config)?;
        self.txn_options
            .submit_transaction(aptos_stdlib::stake_rotate_consensus_key(
                address,
                consensus_public_key.to_bytes().to_vec(),
                consensus_proof_of_possession.to_bytes().to_vec(),
            ))
            .await
            .map(|inner| inner.into())
    }
}

/// Update the current validator's network and fullnode addresses
#[derive(Parser)]
pub struct UpdateValidatorNetworkAddresses {
    #[clap(flatten)]
    pub(crate) txn_options: TransactionOptions,
    #[clap(flatten)]
    pub(crate) operator_args: OperatorArgs,
    #[clap(flatten)]
    pub(crate) operator_config_file_args: OperatorConfigFileArgs,
    #[clap(flatten)]
    pub(crate) validator_network_addresses_args: ValidatorNetworkAddressesArgs,
}

#[async_trait]
impl CliCommand<TransactionSummary> for UpdateValidatorNetworkAddresses {
    fn command_name(&self) -> &'static str {
        "UpdateValidatorNetworkAddresses"
    }

    async fn execute(mut self) -> CliTypedResult<TransactionSummary> {
        let address = self
            .operator_args
            .address_fallback_to_txn(&self.txn_options)?;

        let validator_config = self.operator_config_file_args.load()?;
        let (
            validator_network_public_key,
            full_node_network_public_key,
            validator_host,
            full_node_host,
        ) = self
            .validator_network_addresses_args
            .get_network_configs(&validator_config)?;
        let validator_network_addresses =
            vec![validator_host.as_network_address(validator_network_public_key)?];
        let full_node_network_addresses =
            match (full_node_host.as_ref(), full_node_network_public_key) {
                (Some(host), Some(public_key)) => vec![host.as_network_address(public_key)?],
                (None, None) => vec![],
                _ => {
                    return Err(CliError::CommandArgumentError(
                        "If specifying fullnode addresses, both host and public key are required."
                            .to_string(),
                    ))
                }
            };

        self.txn_options
            .submit_transaction(aptos_stdlib::stake_update_network_and_fullnode_addresses(
                address,
                // BCS encode, so that we can hide the original type
                bcs::to_bytes(&validator_network_addresses)?,
                bcs::to_bytes(&full_node_network_addresses)?,
            ))
            .await
            .map(|inner| inner.into())
    }
}

/// Tool to analyze the performance of an individual validator
#[derive(Parser)]
pub struct AnalyzeValidatorPerformance {
    /// First epoch to analyze
    #[clap(long)]
    pub start_epoch: Option<u64>,

    /// Last epoch to analyze
    #[clap(long)]
    pub end_epoch: Option<u64>,

    /// Analyze mode for the validator: [All, DetailedEpochTable, ValidatorHealthOverTime, NetworkHealthOverTime]
    #[clap(arg_enum, long)]
    pub(crate) analyze_mode: AnalyzeMode,

    #[clap(flatten)]
    pub(crate) rest_options: RestOptions,
    #[clap(flatten)]
    pub(crate) profile_options: ProfileOptions,
}

#[derive(PartialEq, clap::ArgEnum, Clone)]
pub enum AnalyzeMode {
    /// Print all other modes simultaneously
    All,
    /// For each epoch, print a detailed table containing performance
    /// of each of the validators.
    DetailedEpochTable,
    /// For each validator, summarize it's performance in an epoch into
    /// one of the predefined reliability buckets,
    /// and prints it's performance across epochs.
    ValidatorHealthOverTime,
    /// For each epoch summarize how many validators were in
    /// each of the reliability buckets.
    NetworkHealthOverTime,
}

#[async_trait]
impl CliCommand<()> for AnalyzeValidatorPerformance {
    fn command_name(&self) -> &'static str {
        "AnalyzeValidatorPerformance"
    }

    async fn execute(mut self) -> CliTypedResult<()> {
        let client = self.rest_options.client(&self.profile_options.profile)?;

        let epochs =
            FetchMetadata::fetch_new_block_events(&client, self.start_epoch, self.end_epoch)
                .await?;
        let mut stats = HashMap::new();

        let print_detailed = self.analyze_mode == AnalyzeMode::DetailedEpochTable
            || self.analyze_mode == AnalyzeMode::All;
        for epoch_info in epochs {
            let epoch_stats = AnalyzeValidators::analyze(epoch_info.blocks, &epoch_info.validators);
            if print_detailed {
                println!("Detailed table for epoch {}:", epoch_info.epoch);
                AnalyzeValidators::print_detailed_epoch_table(&epoch_stats, None, true);
            }
            stats.insert(epoch_info.epoch, epoch_stats);
        }

        if stats.is_empty() {
            println!("No data found for given input");
            return Ok(());
        }
        let total_stats = stats
            .iter()
            .map(|(_k, v)| v.clone())
            .reduce(|a, b| a + b)
            .unwrap();
        if print_detailed {
            println!(
                "Detailed table for all epochs [{}, {}]:",
                stats.keys().min().unwrap(),
                stats.keys().max().unwrap()
            );
            AnalyzeValidators::print_detailed_epoch_table(&total_stats, None, true);
        }
        let all_validators: Vec<_> = total_stats.validator_stats.keys().cloned().collect();
        if self.analyze_mode == AnalyzeMode::ValidatorHealthOverTime
            || self.analyze_mode == AnalyzeMode::All
        {
            println!(
                "Validator health over epochs [{}, {}]:",
                stats.keys().min().unwrap(),
                stats.keys().max().unwrap()
            );
            AnalyzeValidators::print_validator_health_over_time(&stats, &all_validators, None);
        }
        if self.analyze_mode == AnalyzeMode::NetworkHealthOverTime
            || self.analyze_mode == AnalyzeMode::All
        {
            println!(
                "Network health over epochs [{}, {}]:",
                stats.keys().min().unwrap(),
                stats.keys().max().unwrap()
            );
            AnalyzeValidators::print_network_health_over_time(&stats, &all_validators);
        }
        Ok(())
    }
}
