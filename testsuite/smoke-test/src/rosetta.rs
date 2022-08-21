// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use crate::smoke_test_environment::SwarmBuilder;
use anyhow::anyhow;
use aptos::common::types::{GasOptions, DEFAULT_GAS_UNIT_PRICE, DEFAULT_MAX_GAS};
use aptos::test::INVALID_ACCOUNT;
use aptos::{account::create::DEFAULT_FUNDED_COINS, test::CliTestFramework};
use aptos_config::config::PersistableConfig;
use aptos_config::{config::ApiConfig, utils::get_available_port};
use aptos_crypto::HashValue;
use aptos_rest_client::aptos_api_types::UserTransaction;
use aptos_rest_client::Transaction;
use aptos_rosetta::types::{
    AccountIdentifier, BlockResponse, Operation, OperationStatusType, OperationType,
    TransactionType,
};
use aptos_rosetta::{
    client::RosettaClient,
    common::{native_coin, BLOCKCHAIN, Y2K_MS},
    types::{
        AccountBalanceRequest, AccountBalanceResponse, BlockIdentifier, BlockRequest,
        NetworkIdentifier, NetworkRequest, PartialBlockIdentifier,
    },
    ROSETTA_VERSION,
};
use aptos_types::{account_address::AccountAddress, chain_id::ChainId};
use forge::{LocalSwarm, Node, NodeExt};
use std::collections::BTreeMap;
use std::str::FromStr;
use std::{future::Future, time::Duration};
use tokio::{task::JoinHandle, time::Instant};

const DEFAULT_MAX_WAIT_MS: u64 = 5000;
const DEFAULT_INTERVAL_MS: u64 = 100;
static DEFAULT_MAX_WAIT_DURATION: Duration = Duration::from_millis(DEFAULT_MAX_WAIT_MS);
static DEFAULT_INTERVAL_DURATION: Duration = Duration::from_millis(DEFAULT_INTERVAL_MS);

pub async fn setup_test(
    num_nodes: usize,
    num_accounts: usize,
) -> (LocalSwarm, CliTestFramework, JoinHandle<()>, RosettaClient) {
    let (swarm, cli, faucet) = SwarmBuilder::new_local(num_nodes)
        .with_aptos()
        .build_with_cli(num_accounts)
        .await;
    let validator = swarm.validators().next().unwrap();

    // And the client
    let rosetta_port = get_available_port();
    let rosetta_socket_addr = format!("127.0.0.1:{}", rosetta_port);
    let rosetta_url = format!("http://{}", rosetta_socket_addr.clone())
        .parse()
        .unwrap();
    let rosetta_client = RosettaClient::new(rosetta_url);
    let api_config = ApiConfig {
        enabled: true,
        address: rosetta_socket_addr.parse().unwrap(),
        tls_cert_path: None,
        tls_key_path: None,
        content_length_limit: None,
        failpoints_enabled: false,
    };

    // Start the server
    let _rosetta = aptos_rosetta::bootstrap_async(
        swarm.chain_id(),
        api_config,
        Some(aptos_rest_client::Client::new(
            validator.rest_api_endpoint(),
        )),
    )
    .await
    .unwrap();

    // Ensure rosetta can take requests
    try_until_ok_default(|| rosetta_client.network_list())
        .await
        .unwrap();

    (swarm, cli, faucet, rosetta_client)
}

#[tokio::test]
async fn test_network() {
    let (swarm, _, _, rosetta_client) = setup_test(1, 1).await;
    let chain_id = swarm.chain_id();

    // We only support one network, this network
    let networks = try_until_ok_default(|| rosetta_client.network_list())
        .await
        .unwrap();
    assert_eq!(1, networks.network_identifiers.len());
    let network_id = networks.network_identifiers.first().unwrap();
    assert_eq!(BLOCKCHAIN, network_id.blockchain);
    assert_eq!(chain_id.to_string(), network_id.network);

    let request = NetworkRequest {
        network_identifier: NetworkIdentifier::from(chain_id),
    };
    let options = rosetta_client.network_options(&request).await.unwrap();
    assert_eq!(ROSETTA_VERSION, options.version.rosetta_version);

    // TODO: Check other options

    let request = NetworkRequest {
        network_identifier: NetworkIdentifier::from(chain_id),
    };
    let status = try_until_ok_default(|| rosetta_client.network_status(&request))
        .await
        .unwrap();
    assert!(status.current_block_timestamp >= Y2K_MS);
    assert_eq!(
        BlockIdentifier {
            index: 0,
            hash: HashValue::zero().to_hex()
        },
        status.genesis_block_identifier
    );
    assert_eq!(
        Some(status.genesis_block_identifier),
        status.oldest_block_identifier,
    );
}

#[tokio::test]
async fn test_account_balance() {
    let (swarm, cli, _faucet, rosetta_client) = setup_test(1, 2).await;

    let account_1 = cli.account_id(0);
    let account_2 = cli.account_id(1);
    let chain_id = swarm.chain_id();

    // At time 0, there should be 0 balance
    let response = get_balance(&rosetta_client, chain_id, account_1, Some(0))
        .await
        .unwrap();
    assert_eq!(
        response.block_identifier,
        BlockIdentifier {
            index: 0,
            hash: HashValue::zero().to_hex(),
        }
    );

    // First fund account 1 with lots more gas
    cli.fund_account(0, Some(DEFAULT_FUNDED_COINS * 2))
        .await
        .unwrap();

    let mut account_1_balance = DEFAULT_FUNDED_COINS * 3;
    let mut account_2_balance = DEFAULT_FUNDED_COINS;
    // At some time both accounts should exist with initial amounts
    try_until_ok(Duration::from_secs(5), DEFAULT_INTERVAL_DURATION, || {
        account_has_balance(&rosetta_client, chain_id, account_1, account_1_balance, 0)
    })
    .await
    .unwrap();
    try_until_ok_default(|| {
        account_has_balance(&rosetta_client, chain_id, account_2, account_2_balance, 0)
    })
    .await
    .unwrap();

    // Send money, and expect the gas and fees to show up accordingly
    const TRANSFER_AMOUNT: u64 = 5000;
    let response = cli
        .transfer_coins(
            0,
            1,
            TRANSFER_AMOUNT,
            Some(GasOptions {
                gas_unit_price: DEFAULT_GAS_UNIT_PRICE * 2,
                max_gas: DEFAULT_MAX_GAS,
            }),
        )
        .await
        .unwrap();
    account_1_balance -= TRANSFER_AMOUNT + response.gas_used * response.gas_unit_price;
    account_2_balance += TRANSFER_AMOUNT;
    account_has_balance(&rosetta_client, chain_id, account_1, account_1_balance, 1)
        .await
        .unwrap();
    account_has_balance(&rosetta_client, chain_id, account_2, account_2_balance, 0)
        .await
        .unwrap();

    // Failed transaction spends gas
    let _ = cli
        .transfer_invalid_addr(
            0,
            TRANSFER_AMOUNT,
            Some(GasOptions {
                gas_unit_price: DEFAULT_GAS_UNIT_PRICE * 2,
                max_gas: DEFAULT_MAX_GAS,
            }),
        )
        .await
        .unwrap_err();

    // Make a bad transaction, which will cause gas to be spent but no transfer
    let validator = swarm.validators().next().unwrap();
    let rest_client = validator.rest_client();
    let txns = rest_client
        .get_account_transactions(account_1, None, None)
        .await
        .unwrap()
        .into_inner();
    let failed_txn = txns.last().unwrap();
    if let Transaction::UserTransaction(txn) = failed_txn {
        account_1_balance -= txn.request.gas_unit_price.0 * txn.info.gas_used.0;
        account_has_balance(&rosetta_client, chain_id, account_1, account_1_balance, 2)
            .await
            .unwrap();
    }

    // Check that the balance hasn't changed (and should be 0) in the invalid account
    account_has_balance(
        &rosetta_client,
        chain_id,
        AccountAddress::from_hex_literal(INVALID_ACCOUNT).unwrap(),
        0,
        0,
    )
    .await
    .unwrap();
}

async fn account_has_balance(
    rosetta_client: &RosettaClient,
    chain_id: ChainId,
    account: AccountAddress,
    expected_balance: u64,
    expected_sequence_number: u64,
) -> anyhow::Result<u64> {
    let response = get_balance(rosetta_client, chain_id, account, None).await?;
    assert_eq!(expected_sequence_number, response.metadata.sequence_number);

    if response.balances.iter().any(|amount| {
        amount.currency == native_coin() && amount.value == expected_balance.to_string()
    }) {
        Ok(response.block_identifier.index)
    } else {
        Err(anyhow!(
            "Failed to find account with {} {:?}, received {:?}",
            expected_balance,
            native_coin(),
            response
        ))
    }
}

async fn get_balance(
    rosetta_client: &RosettaClient,
    chain_id: ChainId,
    account: AccountAddress,
    index: Option<u64>,
) -> anyhow::Result<AccountBalanceResponse> {
    let request = AccountBalanceRequest {
        network_identifier: chain_id.into(),
        account_identifier: account.into(),
        block_identifier: Some(PartialBlockIdentifier { index, hash: None }),
        currencies: Some(vec![native_coin()]),
    };
    try_until_ok_default(|| rosetta_client.account_balance(&request)).await
}

/// This test tests all of Rosetta's functionality from the read side in one go.  Since
/// it's block based and it needs time to run, we do all the checks in a single test.
#[tokio::test]
async fn test_block() {
    let (swarm, cli, _faucet, rosetta_client) = setup_test(1, 5).await;
    let chain_id = swarm.chain_id();
    let validator = swarm.validators().next().unwrap();
    let rest_client = validator.rest_client();

    // Mapping of account to block and balance mappings
    let mut balances = BTreeMap::<AccountAddress, BTreeMap<u64, u64>>::new();

    // Do some transfers
    // TODO: Convert these to operations made by Rosetta
    cli.transfer_coins(0, 1, 20, None)
        .await
        .expect("Should transfer coins");
    cli.transfer_coins(1, 0, 20, None)
        .await
        .expect("Should transfer coins");
    cli.transfer_invalid_addr(2, 20, None)
        .await
        .expect_err("Should fail transaction");
    cli.transfer_coins(3, 0, 20, None)
        .await
        .expect("Should transfer coins");
    let summary = cli
        .transfer_coins(1, 3, 20, None)
        .await
        .expect("Should transfer coins");
    let final_block_to_check = rest_client
        .get_block_by_version(summary.version, false)
        .await
        .expect("Should be able to get block info for completed txns");
    let final_block_height = final_block_to_check.into_inner().block_height.0 + 2;

    // TODO: Track total supply?
    // TODO: Check no repeated block hashes
    // TODO: Check no repeated txn hashes (in a block)
    // TODO: Check account balance block hashes?
    // TODO: Handle multiple coin types

    eprintln!("Checking blocks 0..{}", final_block_height);

    // Wait until the Rosetta service is ready
    let request = NetworkRequest {
        network_identifier: NetworkIdentifier::from(chain_id),
    };

    loop {
        let status = try_until_ok_default(|| rosetta_client.network_status(&request))
            .await
            .unwrap();
        if status.current_block_identifier.index >= final_block_height {
            break;
        }
    }

    // Now we have to watch all the changes
    let mut current_version = 0;
    let mut previous_block_index = 0;
    let mut previous_block_hash = format!("{:x}", HashValue::zero());
    for block_height in 0..final_block_height {
        let request = BlockRequest::by_index(chain_id, block_height);
        let response: BlockResponse = rosetta_client
            .block(&request)
            .await
            .expect("Should be able to get blocks that are already known");
        let block = response.block.expect("Every response should have a block");
        let actual_block = rest_client
            .get_block_by_height(block_height, true)
            .await
            .expect("Should be able to get block for a known block")
            .into_inner();

        assert_eq!(
            block.block_identifier.index, block_height,
            "The block should match the requested block"
        );
        assert_eq!(
            block.block_identifier.hash,
            format!("{:x}", actual_block.block_hash),
            "Block hash should match the actual block"
        );
        assert_eq!(
            block.parent_block_identifier.index, previous_block_index,
            "Parent block index should be previous block"
        );
        assert_eq!(
            block.parent_block_identifier.hash, previous_block_hash,
            "Parent block hash should be previous block"
        );

        // It's only greater or equal because microseconds are cut off
        let expected_timestamp = if block_height == 0 {
            Y2K_MS
        } else {
            actual_block.block_timestamp.0.saturating_div(1000)
        };
        assert_eq!(
            expected_timestamp, block.timestamp,
            "Block timestamp should match actual timestamp but in ms"
        );

        // First transaction should be first in block
        assert_eq!(
            current_version, actual_block.first_version.0,
            "First transaction in block should be the current version"
        );

        let actual_txns = actual_block
            .transactions
            .as_ref()
            .expect("Every actual block should have transactions");
        parse_block_transactions(&block, &mut balances, actual_txns, &mut current_version).await;

        // The full block must have been processed
        assert_eq!(current_version - 1, actual_block.last_version.0);

        // Keep track of the previous
        previous_block_hash = block.block_identifier.hash;
        previous_block_index = block_height;
    }

    // Reconcile and ensure all balances are calculated correctly
    check_balances(&rosetta_client, chain_id, balances).await;
}

/// Parse the transactions in each block
async fn parse_block_transactions(
    block: &aptos_rosetta::types::Block,
    balances: &mut BTreeMap<AccountAddress, BTreeMap<u64, u64>>,
    actual_txns: &[Transaction],
    current_version: &mut u64,
) {
    for (txn_number, transaction) in block.transactions.iter().enumerate() {
        let actual_txn = actual_txns
            .get(txn_number)
            .expect("There should be the same number of transactions in the actual block");
        let actual_txn_info = actual_txn
            .transaction_info()
            .expect("Actual transaction should not be pending and have transaction info");
        let txn_metadata = transaction
            .metadata
            .as_ref()
            .expect("Metadata must always be present in a block");

        // Ensure transaction identifier is correct
        assert_eq!(
            *current_version, txn_metadata.version.0,
            "There should be no gaps in transaction versions"
        );
        assert_eq!(
            format!("{:x}", actual_txn_info.hash.0),
            transaction.transaction_identifier.hash,
            "Transaction hash should match the actual hash"
        );

        // Type specific checks
        match txn_metadata.transaction_type {
            TransactionType::Genesis => {
                assert_eq!(0, *current_version);
            }
            TransactionType::User => {}
            TransactionType::BlockMetadata | TransactionType::StateCheckpoint => {
                assert!(transaction.operations.is_empty());
            }
        }

        parse_operations(
            block.block_identifier.index,
            balances,
            transaction,
            actual_txn,
        )
        .await;

        // Increment to next version
        *current_version += 1;
    }
}

/// Parse the individual operations in a transaction
async fn parse_operations(
    block_height: u64,
    balances: &mut BTreeMap<AccountAddress, BTreeMap<u64, u64>>,
    transaction: &aptos_rosetta::types::Transaction,
    actual_txn: &Transaction,
) {
    // If there are no operations, then there is no gas operation
    let mut has_gas_op = false;
    for (expected_index, operation) in transaction.operations.iter().enumerate() {
        assert_eq!(expected_index as u64, operation.operation_identifier.index);

        // Gas transaction is always last
        let status = OperationStatusType::from_str(
            operation
                .status
                .as_ref()
                .expect("Should have an operation status"),
        )
        .expect("Operation status should be known");
        let operation_type = OperationType::from_str(&operation.operation_type)
            .expect("Operation type should be known");

        // Iterate through every operation, keeping track of balances
        match operation_type {
            OperationType::CreateAccount => {
                // Initialize state for a new account
                let account = operation
                    .account
                    .as_ref()
                    .expect("There should be an account in a create account operation")
                    .account_address()
                    .expect("Account address should be parsable");

                if actual_txn.success() {
                    assert_eq!(OperationStatusType::Success, status);
                    let account_balances = balances.entry(account).or_default();

                    if account_balances.is_empty() {
                        account_balances.insert(block_height, 0u64);
                    } else {
                        panic!("Account already has a balance when being created!");
                    }
                } else {
                    assert_eq!(
                        OperationStatusType::Failure,
                        status,
                        "Failed transaction should have failed create account operation"
                    );
                }
            }
            OperationType::Deposit => {
                let account = operation
                    .account
                    .as_ref()
                    .expect("There should be an account in a deposit operation")
                    .account_address()
                    .expect("Account address should be parsable");

                if actual_txn.success() {
                    assert_eq!(OperationStatusType::Success, status);
                    let account_balances = balances.entry(account).or_insert_with(|| {
                        let mut map = BTreeMap::new();
                        map.insert(block_height, 0);
                        map
                    });
                    let (_, latest_balance) = account_balances.iter().last().unwrap();
                    let amount = operation
                        .amount
                        .as_ref()
                        .expect("Should have an amount in a deposit operation");
                    assert_eq!(
                        amount.currency,
                        native_coin(),
                        "Balance should be the native coin"
                    );
                    let delta =
                        u64::parse(&amount.value).expect("Should be able to parse amount value");

                    // Add with panic on overflow in case of too high of a balance
                    let new_balance = *latest_balance + delta;
                    account_balances.insert(block_height, new_balance);
                } else {
                    assert_eq!(
                        OperationStatusType::Failure,
                        status,
                        "Failed transaction should have failed deposit operation"
                    );
                }
            }
            OperationType::Withdraw => {
                // Gas is always successful
                if actual_txn.success() {
                    assert_eq!(OperationStatusType::Success, status);
                    let account = operation
                        .account
                        .as_ref()
                        .expect("There should be an account in a withdraw operation")
                        .account_address()
                        .expect("Account address should be parsable");

                    let account_balances = balances.entry(account).or_insert_with(|| {
                        let mut map = BTreeMap::new();
                        map.insert(block_height, 0);
                        map
                    });
                    let (_, latest_balance) = account_balances.iter().last().unwrap();
                    let amount = operation
                        .amount
                        .as_ref()
                        .expect("Should have an amount in a deposit operation");
                    assert_eq!(
                        amount.currency,
                        native_coin(),
                        "Balance should be the native coin"
                    );
                    let delta = u64::parse(
                        amount
                            .value
                            .strip_prefix('-')
                            .expect("Should have a negative number"),
                    )
                    .expect("Should be able to parse amount value");

                    // Subtract with panic on overflow in case of a negative balance
                    let new_balance = *latest_balance - delta;
                    account_balances.insert(block_height, new_balance);
                } else {
                    assert_eq!(
                        OperationStatusType::Failure,
                        status,
                        "Failed transaction should have failed withdraw operation"
                    );
                }
            }
            OperationType::SetOperator => {
                if actual_txn.success() {
                    assert_eq!(
                        OperationStatusType::Success,
                        status,
                        "Successful transaction should have successful set operator operation"
                    );
                } else {
                    assert_eq!(
                        OperationStatusType::Failure,
                        status,
                        "Failed transaction should have failed set operator operation"
                    );
                }
            }
            OperationType::Fee => {
                has_gas_op = true;
                assert_eq!(OperationStatusType::Success, status);
                let account = operation
                    .account
                    .as_ref()
                    .expect("There should be an account in a fee operation")
                    .account_address()
                    .expect("Account address should be parsable");

                let account_balances = balances.entry(account).or_insert_with(|| {
                    let mut map = BTreeMap::new();
                    map.insert(block_height, 0);
                    map
                });
                let (_, latest_balance) = account_balances.iter().last().unwrap();
                let amount = operation
                    .amount
                    .as_ref()
                    .expect("Should have an amount in a fee operation");
                assert_eq!(
                    amount.currency,
                    native_coin(),
                    "Balance should be the native coin"
                );
                let delta = u64::parse(
                    amount
                        .value
                        .strip_prefix('-')
                        .expect("Should have a negative number"),
                )
                .expect("Should be able to parse amount value");

                // Subtract with panic on overflow in case of a negative balance
                let new_balance = *latest_balance - delta;
                account_balances.insert(block_height, new_balance);

                match actual_txn {
                    Transaction::UserTransaction(txn) => {
                        assert_eq!(
                            txn.info
                                .gas_used
                                .0
                                .saturating_mul(txn.request.gas_unit_price.0),
                            delta,
                            "Gas operation should always match gas used * gas unit price"
                        )
                    }
                    _ => {
                        panic!("Gas transactions should be user transactions!")
                    }
                };
            }
        }
    }

    assert!(
        has_gas_op
            || transaction.metadata.unwrap().transaction_type == TransactionType::Genesis
            || transaction.operations.is_empty(),
        "Must have a gas operation at least in a transaction except for Genesis",
    );
}

/// Check that all balances are correct with the account balance command from the blocks
async fn check_balances(
    rosetta_client: &RosettaClient,
    chain_id: ChainId,
    balances: BTreeMap<AccountAddress, BTreeMap<u64, u64>>,
) {
    // TODO: Check some random times that arent on changes?
    for (account, account_balances) in balances {
        for (block_height, expected_balance) in account_balances {
            // Block should match it's calculated balance
            let response = rosetta_client
                .account_balance(&AccountBalanceRequest {
                    network_identifier: NetworkIdentifier::from(chain_id),
                    account_identifier: account.into(),
                    block_identifier: Some(PartialBlockIdentifier {
                        index: Some(block_height),
                        hash: None,
                    }),
                    currencies: Some(vec![native_coin()]),
                })
                .await
                .unwrap();
            assert_eq!(
                block_height, response.block_identifier.index,
                "Block should be the one expected"
            );

            let balance = response.balances.first().unwrap();
            assert_eq!(
                balance.currency,
                native_coin(),
                "Balance should be the native coin"
            );
            assert_eq!(
                expected_balance,
                u64::parse(&balance.value).expect("Should have a balance from account balance")
            );
        }
    }
}

#[tokio::test]
async fn test_invalid_transaction_gas_charged() {
    let (swarm, cli, _faucet, rosetta_client) = setup_test(1, 1).await;
    let chain_id = swarm.chain_id();

    // Make sure first that there's money to transfer
    cli.assert_account_balance_now(0, DEFAULT_FUNDED_COINS)
        .await;

    // Now let's see some transfers
    const TRANSFER_AMOUNT: u64 = 5000;
    let _ = cli
        .transfer_invalid_addr(
            0,
            TRANSFER_AMOUNT,
            Some(GasOptions {
                gas_unit_price: DEFAULT_GAS_UNIT_PRICE * 2,
                max_gas: DEFAULT_MAX_GAS,
            }),
        )
        .await
        .unwrap_err();

    let sender = cli.account_id(0);

    // Find failed transaction
    let validator = swarm.validators().next().unwrap();
    let rest_client = validator.rest_client();
    let txns = rest_client
        .get_account_transactions(sender, None, None)
        .await
        .unwrap()
        .into_inner();
    let actual_txn = txns.iter().find(|txn| !txn.success()).unwrap();
    let actual_txn = if let Transaction::UserTransaction(txn) = actual_txn {
        txn
    } else {
        panic!("Not a user transaction");
    };
    let txn_version = actual_txn.info.version.0;

    let block_info = rest_client
        .get_block_by_version(txn_version, false)
        .await
        .unwrap()
        .into_inner();

    let block_with_transfer = rosetta_client
        .block(&BlockRequest::by_index(chain_id, block_info.block_height.0))
        .await
        .unwrap();
    let block_with_transfer = block_with_transfer.block.unwrap();
    // Verify failed txn
    let rosetta_txn = block_with_transfer
        .transactions
        .get(txn_version.saturating_sub(block_info.first_version.0) as usize)
        .unwrap();

    assert_transfer_transaction(
        sender,
        AccountAddress::from_hex_literal(INVALID_ACCOUNT).unwrap(),
        TRANSFER_AMOUNT,
        actual_txn,
        rosetta_txn,
    );
}

fn assert_transfer_transaction(
    sender: AccountAddress,
    receiver: AccountAddress,
    transfer_amount: u64,
    actual_txn: &UserTransaction,
    rosetta_txn: &aptos_rosetta::types::Transaction,
) {
    // Check the transaction
    assert_eq!(
        format!("{:x}", actual_txn.info.hash),
        rosetta_txn.transaction_identifier.hash
    );

    let rosetta_txn_metadata = rosetta_txn.metadata.as_ref().unwrap();
    assert_eq!(TransactionType::User, rosetta_txn_metadata.transaction_type);
    assert_eq!(actual_txn.info.version.0, rosetta_txn_metadata.version.0);
    assert_eq!(rosetta_txn.operations.len(), 3);

    // Check the operations
    let mut seen_deposit = false;
    let mut seen_withdraw = false;
    for (i, operation) in rosetta_txn.operations.iter().enumerate() {
        assert_eq!(i as u64, operation.operation_identifier.index);
        if !seen_deposit && !seen_withdraw {
            match OperationType::from_str(&operation.operation_type).unwrap() {
                OperationType::Deposit => {
                    seen_deposit = true;
                    assert_deposit(
                        operation,
                        transfer_amount,
                        receiver,
                        actual_txn.info.success,
                    );
                }
                OperationType::Withdraw => {
                    seen_withdraw = true;
                    assert_withdraw(operation, transfer_amount, sender, actual_txn.info.success);
                }
                _ => panic!("Shouldn't get any other operations"),
            }
        } else if !seen_deposit {
            seen_deposit = true;
            assert_deposit(
                operation,
                transfer_amount,
                receiver,
                actual_txn.info.success,
            );
        } else if !seen_withdraw {
            seen_withdraw = true;
            assert_withdraw(operation, transfer_amount, sender, actual_txn.info.success);
        } else {
            // Gas is always last
            assert_gas(
                operation,
                actual_txn.request.gas_unit_price.0 * actual_txn.info.gas_used.0,
                sender,
                true,
            );
        }
    }
}

fn assert_deposit(
    operation: &Operation,
    expected_amount: u64,
    account: AccountAddress,
    success: bool,
) {
    assert_transfer(
        operation,
        OperationType::Deposit,
        expected_amount.to_string(),
        account,
        success,
    );
}

fn assert_withdraw(
    operation: &Operation,
    expected_amount: u64,
    account: AccountAddress,
    success: bool,
) {
    assert_transfer(
        operation,
        OperationType::Withdraw,
        format!("-{}", expected_amount),
        account,
        success,
    );
}

fn assert_gas(operation: &Operation, expected_amount: u64, account: AccountAddress, success: bool) {
    assert_transfer(
        operation,
        OperationType::Fee,
        format!("-{}", expected_amount),
        account,
        success,
    );
}

fn assert_transfer(
    operation: &Operation,
    expected_type: OperationType,
    expected_amount: String,
    account: AccountAddress,
    success: bool,
) {
    assert_eq!(expected_type.to_string(), operation.operation_type);
    let amount = operation.amount.as_ref().unwrap();
    assert_eq!(native_coin(), amount.currency);
    assert_eq!(expected_amount, amount.value);
    assert_eq!(
        &AccountIdentifier::from(account),
        operation.account.as_ref().unwrap()
    );
    let expected_status = if success {
        OperationStatusType::Success
    } else {
        OperationStatusType::Failure
    }
    .to_string();
    assert_eq!(&expected_status, operation.status.as_ref().unwrap());
}

/// Try for 2 seconds to get a response.  This handles the fact that it's starting async
async fn try_until_ok_default<F, Fut, T>(function: F) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    try_until_ok(
        DEFAULT_MAX_WAIT_DURATION,
        DEFAULT_INTERVAL_DURATION,
        function,
    )
    .await
}

async fn try_until_ok<F, Fut, T>(
    total_wait: Duration,
    interval: Duration,
    function: F,
) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut result = Err(anyhow::Error::msg("Failed to get response"));
    let start = Instant::now();
    while start.elapsed() < total_wait {
        result = function().await;
        if result.is_ok() {
            break;
        }
        tokio::time::sleep(interval).await;
    }

    result
}
