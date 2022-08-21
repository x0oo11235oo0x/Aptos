// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

#![forbid(unsafe_code)]

mod genesis_context;

use crate::genesis_context::GenesisStateView;
use aptos_crypto::{
    bls12381,
    ed25519::{Ed25519PrivateKey, Ed25519PublicKey},
    HashValue, PrivateKey, Uniform,
};
use aptos_gas::{
    AptosGasParameters, InitialGasSchedule, NativeGasParameters, ToOnChainGasSchedule,
};
use aptos_types::account_config::aptos_test_root_address;
use aptos_types::{
    account_config::{self, events::NewEpochEvent, CORE_CODE_ADDRESS},
    chain_id::ChainId,
    contract_event::ContractEvent,
    on_chain_config::{ConsensusConfigV1, OnChainConsensusConfig, APTOS_MAX_KNOWN_VERSION},
    transaction::{authenticator::AuthenticationKey, ChangeSet, Transaction, WriteSetPayload},
};
use aptos_vm::{
    data_cache::{IntoMoveResolver, StateViewCache},
    move_vm_ext::{MoveVmExt, SessionExt, SessionId},
};
use framework::{ReleaseBundle, ReleasePackage};
use move_deps::{
    move_core_types::{
        account_address::AccountAddress,
        identifier::Identifier,
        language_storage::{ModuleId, TypeTag},
        resolver::MoveResolver,
        value::{serialize_values, MoveValue},
    },
    move_vm_types::gas::UnmeteredGasMeter,
};
use once_cell::sync::Lazy;
use rand::prelude::*;
use serde::{Deserialize, Serialize};

// The seed is arbitrarily picked to produce a consistent key. XXX make this more formal?
const GENESIS_SEED: [u8; 32] = [42; 32];

const GENESIS_MODULE_NAME: &str = "genesis";
const GOVERNANCE_MODULE_NAME: &str = "aptos_governance";
const CODE_MODULE_NAME: &str = "code";
const VERSION_MODULE_NAME: &str = "version";

const NUM_SECONDS_PER_YEAR: u64 = 365 * 24 * 60 * 60;
const MICRO_SECONDS_PER_SECOND: u64 = 1_000_000;
const APTOS_COINS_BASE_WITH_DECIMALS: u64 = u64::pow(10, 8);

pub struct GenesisConfiguration {
    pub allow_new_validators: bool,
    pub epoch_duration_secs: u64,
    // If true, genesis will create a special core resources account that can mint coins.
    pub is_test: bool,
    pub max_stake: u64,
    pub min_stake: u64,
    pub min_voting_threshold: u128,
    pub recurring_lockup_duration_secs: u64,
    pub required_proposer_stake: u64,
    pub rewards_apy_percentage: u64,
    pub voting_duration_secs: u64,
    pub voting_power_increase_limit: u64,
}

pub static GENESIS_KEYPAIR: Lazy<(Ed25519PrivateKey, Ed25519PublicKey)> = Lazy::new(|| {
    let mut rng = StdRng::from_seed(GENESIS_SEED);
    let private_key = Ed25519PrivateKey::generate(&mut rng);
    let public_key = private_key.public_key();
    (private_key, public_key)
});

pub fn encode_genesis_transaction(
    aptos_root_key: Ed25519PublicKey,
    validators: &[Validator],
    framework: &ReleaseBundle,
    chain_id: ChainId,
    genesis_config: GenesisConfiguration,
) -> Transaction {
    let consensus_config = OnChainConsensusConfig::V1(ConsensusConfigV1::default());

    Transaction::GenesisTransaction(WriteSetPayload::Direct(encode_genesis_change_set(
        &aptos_root_key,
        validators,
        framework,
        consensus_config,
        chain_id,
        &genesis_config,
    )))
}

pub fn encode_genesis_change_set(
    core_resources_key: &Ed25519PublicKey,
    validators: &[Validator],
    framework: &ReleaseBundle,
    consensus_config: OnChainConsensusConfig,
    chain_id: ChainId,
    genesis_config: &GenesisConfiguration,
) -> ChangeSet {
    validate_genesis_config(genesis_config);

    // Create a Move VM session so we can invoke on-chain genesis intializations.
    let mut state_view = GenesisStateView::new();
    for (module_bytes, module) in framework.code_and_compiled_modules() {
        state_view.add_module(&module.self_id(), module_bytes);
    }
    let data_cache = StateViewCache::new(&state_view).into_move_resolver();
    let move_vm = MoveVmExt::new(NativeGasParameters::zeros()).unwrap();
    let id1 = HashValue::zero();
    let mut session = move_vm.new_session(&data_cache, SessionId::genesis(id1));

    // On-chain genesis process.
    initialize(&mut session, consensus_config, chain_id, genesis_config);
    if genesis_config.is_test {
        initialize_core_resources_and_aptos_coin(&mut session, core_resources_key);
    } else {
        initialize_aptos_coin(&mut session);
    }
    initialize_on_chain_governance(&mut session, genesis_config);
    create_and_initialize_validators(&mut session, validators);
    if genesis_config.is_test {
        allow_core_resources_to_set_version(&mut session);
    }

    // Reconfiguration should happen after all on-chain invocations.
    emit_new_block_and_epoch_event(&mut session);

    let mut session1_out = session.finish().unwrap();

    let state_view = GenesisStateView::new();
    let data_cache = StateViewCache::new(&state_view).into_move_resolver();

    // Publish the framework, using a different session id, in case both scripts creates tables
    let mut id2_arr = [0u8; 32];
    id2_arr[31] = 1;
    let id2 = HashValue::new(id2_arr);
    let mut session = move_vm.new_session(&data_cache, SessionId::genesis(id2));
    publish_framework(&mut session, framework);
    let session2_out = session.finish().unwrap();

    session1_out.squash(session2_out).unwrap();

    let change_set_ext = session1_out.into_change_set(&mut ()).unwrap();
    let (delta_change_set, change_set) = change_set_ext.into_inner();

    // Publishing stdlib should not produce any deltas.
    // Proof: First session output is obtained during genesis when we initialize
    // the data. This implies that all values which are aggregators know their
    // real values, and therefore map to write ops and not deltas. For example,
    // `create_and_initialize_validators` mints aptos coins which has aggregatable
    // supply, but since supply is initialized during the same session there will
    // be no deltas. The second session pusblishes framework module bundle which
    // does not produce deltas either.
    debug_assert!(
        delta_change_set.is_empty(),
        "non-empty delta change set in genesis"
    );

    assert!(!change_set
        .write_set()
        .iter()
        .any(|(_, op)| op.is_deletion()));
    verify_genesis_write_set(change_set.events());
    change_set
}

fn validate_genesis_config(genesis_config: &GenesisConfiguration) {
    assert!(
        genesis_config.min_stake <= genesis_config.max_stake,
        "Min stake must be smaller than or equal to max stake"
    );
    assert!(
        genesis_config.epoch_duration_secs > 0,
        "Epoch duration must be > 0"
    );
    assert!(
        genesis_config.recurring_lockup_duration_secs > 0,
        "Recurring lockup duration must be > 0"
    );
    assert!(
        genesis_config.recurring_lockup_duration_secs >= genesis_config.epoch_duration_secs,
        "Recurring lockup duration must be at least as long as epoch duration"
    );
    assert!(
        genesis_config.rewards_apy_percentage > 0 && genesis_config.rewards_apy_percentage < 100,
        "Rewards APY must be > 0% and < 100%"
    );
    assert!(
        genesis_config.voting_duration_secs > 0,
        "On-chain voting duration must be > 0"
    );
    assert!(
        genesis_config.voting_duration_secs < genesis_config.recurring_lockup_duration_secs,
        "Voting duration must be strictly smaller than recurring lockup"
    );
    assert!(
        genesis_config.voting_power_increase_limit > 0
            && genesis_config.voting_power_increase_limit <= 50,
        "voting_power_increase_limit must be > 0 and <= 50"
    );
}

fn exec_function(
    session: &mut SessionExt<impl MoveResolver>,
    module_name: &str,
    function_name: &str,
    ty_args: Vec<TypeTag>,
    args: Vec<Vec<u8>>,
) {
    session
        .execute_function_bypass_visibility(
            &ModuleId::new(
                account_config::CORE_CODE_ADDRESS,
                Identifier::new(module_name).unwrap(),
            ),
            &Identifier::new(function_name).unwrap(),
            ty_args,
            args,
            &mut UnmeteredGasMeter,
        )
        .unwrap_or_else(|e| {
            panic!(
                "Error calling {}.{}: {}",
                module_name,
                function_name,
                e.into_vm_status()
            )
        });
}

fn initialize(
    session: &mut SessionExt<impl MoveResolver>,
    consensus_config: OnChainConsensusConfig,
    chain_id: ChainId,
    genesis_config: &GenesisConfiguration,
) {
    let genesis_gas_params = AptosGasParameters::initial();
    let gas_schedule_blob = bcs::to_bytes(&genesis_gas_params.to_on_chain_gas_schedule())
        .expect("Failure serializing genesis gas schedule");

    let consensus_config_bytes =
        bcs::to_bytes(&consensus_config).expect("Failure serializing genesis consensus config");

    // Calculate the per-epoch rewards rate, represented as 2 separate ints (numerator and
    // denominator).
    let rewards_rate_denominator = 1_000_000_000;
    let num_epochs_in_a_year = NUM_SECONDS_PER_YEAR / genesis_config.epoch_duration_secs;
    // Multiplication before division to minimize rounding errors due to integer division.
    let rewards_rate_numerator = (genesis_config.rewards_apy_percentage * rewards_rate_denominator
        / 100)
        / num_epochs_in_a_year;

    // Block timestamps are in microseconds and epoch_interval is used to check if a block timestamp
    // has crossed into a new epoch. So epoch_interval also needs to be in micro seconds.
    let epoch_interval_usecs = genesis_config.epoch_duration_secs * MICRO_SECONDS_PER_SECOND;
    exec_function(
        session,
        GENESIS_MODULE_NAME,
        "initialize",
        vec![],
        serialize_values(&vec![
            MoveValue::vector_u8(gas_schedule_blob),
            MoveValue::U8(chain_id.id()),
            MoveValue::U64(APTOS_MAX_KNOWN_VERSION.major),
            MoveValue::vector_u8(consensus_config_bytes),
            MoveValue::U64(epoch_interval_usecs),
            MoveValue::U64(genesis_config.min_stake),
            MoveValue::U64(genesis_config.max_stake),
            MoveValue::U64(genesis_config.recurring_lockup_duration_secs),
            MoveValue::Bool(genesis_config.allow_new_validators),
            MoveValue::U64(rewards_rate_numerator),
            MoveValue::U64(rewards_rate_denominator),
            MoveValue::U64(genesis_config.voting_power_increase_limit),
        ]),
    );
}

fn initialize_aptos_coin(session: &mut SessionExt<impl MoveResolver>) {
    exec_function(
        session,
        GENESIS_MODULE_NAME,
        "initialize_aptos_coin",
        vec![],
        serialize_values(&vec![MoveValue::Signer(CORE_CODE_ADDRESS)]),
    );
}

fn initialize_core_resources_and_aptos_coin(
    session: &mut SessionExt<impl MoveResolver>,
    core_resources_key: &Ed25519PublicKey,
) {
    let core_resources_auth_key = AuthenticationKey::ed25519(core_resources_key);
    exec_function(
        session,
        GENESIS_MODULE_NAME,
        "initialize_core_resources_and_aptos_coin",
        vec![],
        serialize_values(&vec![
            MoveValue::Signer(CORE_CODE_ADDRESS),
            MoveValue::vector_u8(core_resources_auth_key.to_vec()),
        ]),
    );
}

/// Create and initialize Association and Core Code accounts.
fn initialize_on_chain_governance(
    session: &mut SessionExt<impl MoveResolver>,
    genesis_config: &GenesisConfiguration,
) {
    exec_function(
        session,
        GOVERNANCE_MODULE_NAME,
        "initialize",
        vec![],
        serialize_values(&vec![
            MoveValue::Signer(CORE_CODE_ADDRESS),
            MoveValue::U128(genesis_config.min_voting_threshold),
            MoveValue::U64(genesis_config.required_proposer_stake),
            MoveValue::U64(genesis_config.voting_duration_secs),
        ]),
    );
}

/// Creates and initializes each validator owner and validator operator. This method creates all
/// the required accounts, sets the validator operators for each validator owner, and sets the
/// validator config on-chain.
fn create_and_initialize_validators(
    session: &mut SessionExt<impl MoveResolver>,
    validators: &[Validator],
) {
    let validators_bytes = bcs::to_bytes(validators).expect("Validators can be serialized");
    let mut serialized_values = serialize_values(&vec![MoveValue::Signer(CORE_CODE_ADDRESS)]);
    serialized_values.push(validators_bytes);
    exec_function(
        session,
        GENESIS_MODULE_NAME,
        "create_initialize_validators",
        vec![],
        serialized_values,
    );
}

fn allow_core_resources_to_set_version(session: &mut SessionExt<impl MoveResolver>) {
    exec_function(
        session,
        VERSION_MODULE_NAME,
        "initialize_for_test",
        vec![],
        serialize_values(&vec![MoveValue::Signer(aptos_test_root_address())]),
    );
}

/// Publish the framework release bundle.
fn publish_framework(session: &mut SessionExt<impl MoveResolver>, framework: &ReleaseBundle) {
    for pack in &framework.packages {
        publish_package(session, pack)
    }
}

/// Publish the given package.
fn publish_package(session: &mut SessionExt<impl MoveResolver>, pack: &ReleasePackage) {
    let modules = pack.sorted_code_and_modules();
    let addr = *modules.get(0).unwrap().1.self_id().address();
    let code = modules
        .into_iter()
        .map(|(c, _)| c.to_vec())
        .collect::<Vec<_>>();
    session
        .publish_module_bundle(code, addr, &mut UnmeteredGasMeter)
        .unwrap_or_else(|e| {
            panic!(
                "Failure publishing package `{}`: {:?}",
                pack.package_metadata().name,
                e
            )
        });

    // Call the initialize function with the metadata.
    exec_function(
        session,
        CODE_MODULE_NAME,
        "initialize",
        vec![],
        vec![
            MoveValue::Signer(CORE_CODE_ADDRESS)
                .simple_serialize()
                .unwrap(),
            MoveValue::Signer(addr).simple_serialize().unwrap(),
            bcs::to_bytes(pack.package_metadata()).unwrap(),
        ],
    );
}

/// Trigger a reconfiguration. This emits an event that will be passed along to the storage layer.
fn emit_new_block_and_epoch_event(session: &mut SessionExt<impl MoveResolver>) {
    exec_function(
        session,
        "block",
        "emit_genesis_block_event",
        vec![],
        serialize_values(&vec![MoveValue::Signer(
            account_config::reserved_vm_address(),
        )]),
    );
    exec_function(
        session,
        "reconfiguration",
        "emit_genesis_reconfiguration_event",
        vec![],
        vec![],
    );
}

/// Verify the consistency of the genesis `WriteSet`
fn verify_genesis_write_set(events: &[ContractEvent]) {
    let new_epoch_events: Vec<&ContractEvent> = events
        .iter()
        .filter(|e| e.key() == &NewEpochEvent::event_key())
        .collect();
    assert_eq!(
        new_epoch_events.len(),
        1,
        "There should only be exactly one NewEpochEvent"
    );
    assert_eq!(new_epoch_events[0].sequence_number(), 0,);
}

/// An enum specifying whether the compiled stdlib/scripts should be used or freshly built versions
/// should be used.
#[derive(Debug, Eq, PartialEq)]
pub enum GenesisOptions {
    Compiled,
    Fresh,
}

/// Generate an artificial genesis `ChangeSet` for testing
pub fn generate_genesis_change_set_for_testing(genesis_options: GenesisOptions) -> ChangeSet {
    let framework = match genesis_options {
        GenesisOptions::Compiled => cached_packages::head_release_bundle(),
        GenesisOptions::Fresh => cached_packages::devnet_release_bundle(),
    };

    generate_test_genesis(framework, Some(1)).0
}

/// Generate a genesis `ChangeSet` for mainnet
pub fn generate_genesis_change_set_for_mainnet(genesis_options: GenesisOptions) -> ChangeSet {
    let framework = match genesis_options {
        GenesisOptions::Compiled => cached_packages::head_release_bundle(),
        GenesisOptions::Fresh => cached_packages::devnet_release_bundle(),
    };

    generate_mainnet_genesis(framework, Some(1)).0
}

pub fn test_genesis_transaction() -> Transaction {
    let changeset = test_genesis_change_set_and_validators(None).0;
    Transaction::GenesisTransaction(WriteSetPayload::Direct(changeset))
}

pub fn test_genesis_change_set_and_validators(
    count: Option<usize>,
) -> (ChangeSet, Vec<TestValidator>) {
    generate_test_genesis(cached_packages::head_release_bundle(), count)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Validator {
    /// The Aptos account address of the validator.
    pub owner_address: AccountAddress,
    /// The Aptos account address of the validator's operator (same as `address` if the validator is
    /// its own operator).
    pub operator_address: AccountAddress,
    pub voter_address: AccountAddress,
    /// Amount to stake for consensus. Also the intial amount minted to the owner account.
    pub stake_amount: u64,

    /// bls12381 public key used to sign consensus messages.
    pub consensus_pubkey: Vec<u8>,
    /// Proof of Possession of the consensus pubkey.
    pub proof_of_possession: Vec<u8>,
    /// `NetworkAddress` for the validator.
    pub network_addresses: Vec<u8>,
    /// `NetworkAddress` for the validator's full node.
    pub full_node_network_addresses: Vec<u8>,
}

pub struct TestValidator {
    pub key: Ed25519PrivateKey,
    pub consensus_key: bls12381::PrivateKey,
    pub data: Validator,
}

impl TestValidator {
    pub fn new_test_set(count: Option<usize>, initial_stake: Option<u64>) -> Vec<TestValidator> {
        let mut rng = rand::SeedableRng::from_seed([1u8; 32]);
        (0..count.unwrap_or(10))
            .map(|_| TestValidator::gen(&mut rng, initial_stake))
            .collect()
    }

    fn gen(rng: &mut StdRng, initial_stake: Option<u64>) -> TestValidator {
        let key = Ed25519PrivateKey::generate(rng);
        let auth_key = AuthenticationKey::ed25519(&key.public_key());
        let owner_address = auth_key.derived_address();
        let consensus_key = bls12381::PrivateKey::generate(rng);
        let consensus_pubkey = consensus_key.public_key().to_bytes().to_vec();
        let proof_of_possession = bls12381::ProofOfPossession::create(&consensus_key)
            .to_bytes()
            .to_vec();
        let network_address = [0u8; 0].to_vec();
        let full_node_network_address = [0u8; 0].to_vec();

        let stake_amount = if let Some(amount) = initial_stake {
            amount
        } else {
            1
        };
        let data = Validator {
            owner_address,
            consensus_pubkey,
            proof_of_possession,
            operator_address: owner_address,
            voter_address: owner_address,
            network_addresses: network_address,
            full_node_network_addresses: full_node_network_address,
            stake_amount,
        };
        Self {
            key,
            consensus_key,
            data,
        }
    }
}

pub fn generate_test_genesis(
    framework: &ReleaseBundle,
    count: Option<usize>,
) -> (ChangeSet, Vec<TestValidator>) {
    let test_validators = TestValidator::new_test_set(count, Some(100_000_000));
    let validators_: Vec<Validator> = test_validators.iter().map(|t| t.data.clone()).collect();
    let validators = &validators_;

    let genesis = encode_genesis_change_set(
        &GENESIS_KEYPAIR.1,
        validators,
        framework,
        OnChainConsensusConfig::default(),
        ChainId::test(),
        &GenesisConfiguration {
            allow_new_validators: true,
            epoch_duration_secs: 3600,
            is_test: true,
            min_stake: 0,
            min_voting_threshold: 0,
            // 1M APTOS coins (with 8 decimals).
            max_stake: 100_000_000_000_000,
            recurring_lockup_duration_secs: 7200,
            required_proposer_stake: 0,
            rewards_apy_percentage: 10,
            voting_duration_secs: 3600,
            voting_power_increase_limit: 50,
        },
    );
    (genesis, test_validators)
}

pub fn generate_mainnet_genesis(
    framework: &ReleaseBundle,
    count: Option<usize>,
) -> (ChangeSet, Vec<TestValidator>) {
    // TODO: Update to have custom validators/accounts with initial balances at genesis.
    let test_validators = TestValidator::new_test_set(count, Some(1_000_000_000_000_000));
    let validators_: Vec<Validator> = test_validators.iter().map(|t| t.data.clone()).collect();
    let validators = &validators_;

    let genesis = encode_genesis_change_set(
        &GENESIS_KEYPAIR.1,
        validators,
        framework,
        OnChainConsensusConfig::default(),
        ChainId::test(),
        // TODO: Update once mainnet numbers are decided. These numbers are just placeholders.
        &GenesisConfiguration {
            allow_new_validators: true,
            epoch_duration_secs: 2 * 3600, // 2 hours
            is_test: false,
            min_stake: 1_000_000 * APTOS_COINS_BASE_WITH_DECIMALS, // 1M APT
            // 400M APT
            min_voting_threshold: (400_000_000 * APTOS_COINS_BASE_WITH_DECIMALS as u128),
            max_stake: 50_000_000 * APTOS_COINS_BASE_WITH_DECIMALS, // 50M APT.
            recurring_lockup_duration_secs: 30 * 24 * 3600,         // 1 month
            required_proposer_stake: 1_000_000 * APTOS_COINS_BASE_WITH_DECIMALS, // 1M APT
            rewards_apy_percentage: 10,
            voting_duration_secs: 7 * 24 * 3600, // 7 days
            voting_power_increase_limit: 30,
        },
    );
    (genesis, test_validators)
}

#[test]
pub fn test_genesis_module_publishing() {
    // create a state view for move_vm
    let mut state_view = GenesisStateView::new();
    for (module_bytes, module) in cached_packages::head_release_bundle().code_and_compiled_modules()
    {
        state_view.add_module(&module.self_id(), module_bytes);
    }
    let data_cache = StateViewCache::new(&state_view).into_move_resolver();

    let move_vm = MoveVmExt::new(NativeGasParameters::zeros()).unwrap();
    let id1 = HashValue::zero();
    let mut session = move_vm.new_session(&data_cache, SessionId::genesis(id1));
    publish_framework(&mut session, cached_packages::head_release_bundle());
}
