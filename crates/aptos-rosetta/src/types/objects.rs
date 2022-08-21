// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

//! Objects of the Rosetta spec
//!
//! [Spec](https://www.rosetta-api.org/docs/api_objects.html)

use crate::common::native_coin_tag;
use crate::types::{
    account_module_identifier, aptos_coin_module_identifier, aptos_coin_resource_identifier,
    coin_module_identifier, create_account_function_identifier,
    set_operator_events_field_identifier, set_operator_function_identifier,
    stake_module_identifier, stake_pool_resource_identifier, transfer_function_identifier,
};
use crate::{
    common::{is_native_coin, native_coin},
    error::ApiResult,
    types::{
        account_resource_identifier, coin_store_resource_identifier,
        deposit_events_field_identifier, sequence_number_field_identifier,
        withdraw_events_field_identifier, AccountIdentifier, BlockIdentifier, Error,
        NetworkIdentifier, OperationIdentifier, OperationStatus, OperationStatusType,
        OperationType, TransactionIdentifier,
    },
    ApiError,
};
use anyhow::anyhow;
use aptos_crypto::{ed25519::Ed25519PublicKey, ValidCryptoMaterialStringExt};
use aptos_rest_client::aptos_api_types::{
    Address, Event, MoveStructTag, MoveType, TransactionPayload, UserTransactionRequest,
    WriteResource,
};
use aptos_rest_client::{
    aptos::Balance,
    aptos_api_types::{WriteSetChange, U64},
};
use aptos_types::{account_address::AccountAddress, event::EventKey};
use serde::{de::Error as SerdeError, Deserialize, Deserializer, Serialize};
use std::{
    collections::HashMap,
    convert::{TryFrom, TryInto},
    fmt::{Display, Formatter},
    hash::Hash,
    str::FromStr,
};

/// A description of all types used by the Rosetta implementation.
///
/// This is used to verify correctness of the implementation and to check things like
/// operation names, and error names.
///
/// [API Spec](https://www.rosetta-api.org/docs/models/Allow.html)
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Allow {
    /// List of all possible operation statuses
    pub operation_statuses: Vec<OperationStatus>,
    /// List of all possible writeset types
    pub operation_types: Vec<String>,
    /// List of all possible errors
    pub errors: Vec<Error>,
    /// If the server is allowed to lookup historical transactions
    pub historical_balance_lookup: bool,
    /// All times after this are valid timestamps
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_start_index: Option<u64>,
    /// All call methods supported
    pub call_methods: Vec<String>,
    /// A list of balance exemptions.  These should be as minimal as possible, otherwise it becomes
    /// more complicated for users
    pub balance_exemptions: Vec<BalanceExemption>,
    /// Determines if mempool can change the balance on an account
    /// This should be set to false
    pub mempool_coins: bool,
    /// Case specifics for block hashes.  Set to None if case insensitive
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_hash_case: Option<Case>,
    /// Case specifics for transaction hashes.  Set to None if case insensitive
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_hash_case: Option<Case>,
}

/// Amount of a [`Currency`] in atomic units
///
/// [API Spec](https://www.rosetta-api.org/docs/models/Amount.html)
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Amount {
    /// Value of transaction as a String representation of an integer
    pub value: String,
    /// [`Currency`]
    pub currency: Currency,
}

impl From<Balance> for Amount {
    fn from(balance: Balance) -> Self {
        Amount {
            value: balance.coin.value.to_string(),
            // TODO: Support other currencies
            currency: native_coin(),
        }
    }
}

/// Balance exemptions where the current balance of an account can change without a transaction
/// operation.  This is typically e
///
/// [API Spec](https://www.rosetta-api.org/docs/models/BalanceExemption.html)
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BalanceExemption {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_account_address: Option<String>,
    /// The currency that can change based on the exemption
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<Currency>,
    /// The exemption type of which direction a balance can change
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exemption_type: Option<ExemptionType>,
}

/// Representation of a Block for a blockchain.  For aptos it is the version
///
/// [API Spec](https://www.rosetta-api.org/docs/models/Block.html)
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Block {
    /// Block identifier of the current block
    pub block_identifier: BlockIdentifier,
    /// Block identifier of the previous block
    pub parent_block_identifier: BlockIdentifier,
    /// Timestamp in milliseconds to the block from the UNIX_EPOCH
    pub timestamp: u64,
    /// Transactions associated with the version.  In aptos there should only be one transaction
    pub transactions: Vec<Transaction>,
}

/// Events that allow lighter weight block updates of add and removing block
///
/// [API Spec](https://www.rosetta-api.org/docs/models/BlockEvent.html)
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BlockEvent {
    /// Ordered event index for events on a NetworkIdentifier (likely the same as version)
    pub sequence: u64,
    /// Block identifier of the block to change
    pub block_identifier: BlockIdentifier,
    /// Block event type add or remove block
    #[serde(rename = "type")]
    pub block_event_type: BlockEventType,
    /// Transactions associated with the update, it should be only one transaction in Aptos.
    pub transactions: Vec<Transaction>,
}

/// Determines if the event is about adding or removing blocks
///
/// [API Spec](https://www.rosetta-api.org/docs/models/BlockEventType.html)
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockEventType {
    BlockAdded,
    BlockRemoved,
}

/// A combination of a transaction and the block associated.  In Aptos, this is just the same
/// as the version associated with the transaction
///
/// [API Spec](https://www.rosetta-api.org/docs/models/BlockTransaction.html)
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BlockTransaction {
    /// Block associated with transaction
    block_identifier: BlockIdentifier,
    /// Transaction associated with block
    transaction: Transaction,
}

/// Tells what cases are supported in hashes. Having no value is case insensitive.
///
///[API Spec](https://www.rosetta-api.org/docs/models/Case.html)
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Case {
    UpperCase,
    LowerCase,
    CaseSensitive,
}

/// Currency represented as atomic units including decimals
///
/// [API Spec](https://www.rosetta-api.org/docs/models/Currency.html)
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Currency {
    /// Symbol of currency
    pub symbol: String,
    /// Number of decimals to be considered in the currency
    pub decimals: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<CurrencyMetadata>,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct CurrencyMetadata {
    pub move_type: String,
}

/// Various signing curves supported by Rosetta.  We only use [`CurveType::Edwards25519`]
/// [API Spec](https://www.rosetta-api.org/docs/models/CurveType.html)
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveType {
    Edwards25519,
    Secp256k1,
    Secp256r1,
    Tweedle,
    Pallas,
}

/// Used for related transactions to determine direction of relation
///
/// [API Spec](https://www.rosetta-api.org/docs/models/Direction.html)
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Associated to a later transaction
    Forward,
    /// Associated to an earlier transaction
    Backward,
}

/// Tells how balances can change without a specific transaction on the account
///
/// Balance exemptions are not necessary, because staking rewards go to the staking
/// pool and not to the account.  When they are removed from the pool, normal events
/// for transfer will occur.
///
/// [API Spec](https://www.rosetta-api.org/docs/models/ExemptionType.html)
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExemptionType {
    /// Balance can be greater than or equal to the current balance e.g. staking
    GreaterOrEqual,
    /// Balance can be less than or equal to the current balance
    LessOrEqual,
    /// Balance can be less than or greater than the current balance e.g. dynamic supplies
    Dynamic,
}

/// A representation of a single account change in a transaction
///
/// This is known as a write set change within Aptos
/// [API Spec](https://www.rosetta-api.org/docs/models/Operation.html)
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Operation {
    /// Identifier of an operation within a transaction
    pub operation_identifier: OperationIdentifier,
    /// Related operations e.g. multiple operations that are related to a transfer
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_operations: Option<Vec<OperationIdentifier>>,
    /// Type of operation
    #[serde(rename = "type")]
    pub operation_type: String,
    /// Status of operation.  Must be populated if the transaction is in the past.  If submitting
    /// new transactions, it must NOT be populated.
    pub status: Option<String>,
    /// AccountIdentifier should be provided to point at which account the change is
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<AccountIdentifier>,
    /// Amount in the operation
    ///
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<Amount>,
    /// Operation specific metadata for any operation that's missing information it needs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<OperationSpecificMetadata>,
}

impl Operation {
    fn new(
        operation_type: OperationType,
        operation_index: u64,
        status: Option<OperationStatusType>,
        address: AccountAddress,
        amount: Option<Amount>,
        metadata: Option<OperationSpecificMetadata>,
    ) -> Operation {
        Operation {
            operation_identifier: OperationIdentifier {
                index: operation_index,
                network_index: None,
            },
            related_operations: None,
            operation_type: operation_type.to_string(),
            status: status.map(|inner| inner.to_string()),
            account: Some(address.into()),
            amount,
            metadata,
        }
    }

    pub fn create_account(
        operation_index: u64,
        status: Option<OperationStatusType>,
        address: AccountAddress,
        sender: AccountAddress,
    ) -> Operation {
        Operation::new(
            OperationType::CreateAccount,
            operation_index,
            status,
            address,
            None,
            Some(OperationSpecificMetadata::create_account(sender)),
        )
    }

    pub fn deposit(
        operation_index: u64,
        status: Option<OperationStatusType>,
        address: AccountAddress,
        currency: Currency,
        amount: u64,
    ) -> Operation {
        Operation::new(
            OperationType::Deposit,
            operation_index,
            status,
            address,
            Some(Amount {
                value: amount.to_string(),
                currency,
            }),
            None,
        )
    }

    pub fn withdraw(
        operation_index: u64,
        status: Option<OperationStatusType>,
        address: AccountAddress,
        currency: Currency,
        amount: u64,
    ) -> Operation {
        Operation::new(
            OperationType::Withdraw,
            operation_index,
            status,
            address,
            Some(Amount {
                value: format!("-{}", amount),
                currency,
            }),
            None,
        )
    }

    pub fn gas_fee(
        operation_index: u64,
        address: AccountAddress,
        gas_used: u64,
        gas_price_per_unit: u64,
    ) -> Operation {
        Operation::new(
            OperationType::Fee,
            operation_index,
            Some(OperationStatusType::Success),
            address,
            Some(Amount {
                value: format!("-{}", gas_used.saturating_mul(gas_price_per_unit)),
                currency: native_coin(),
            }),
            None,
        )
    }

    pub fn set_operator(
        operation_index: u64,
        status: Option<OperationStatusType>,
        address: AccountAddress,
        operator: AccountAddress,
    ) -> Operation {
        Operation::new(
            OperationType::Withdraw,
            operation_index,
            status,
            address,
            None,
            Some(OperationSpecificMetadata::set_operator(operator)),
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OperationSpecificMetadata {
    CreateAccount(CreateAccountArguments),
    SetOperator(SetOperatorArguments),
}

impl OperationSpecificMetadata {
    pub fn create_account(sender: AccountAddress) -> OperationSpecificMetadata {
        OperationSpecificMetadata::CreateAccount(CreateAccountArguments {
            sender: sender.into(),
        })
    }

    pub fn set_operator(operator: AccountAddress) -> OperationSpecificMetadata {
        OperationSpecificMetadata::SetOperator(SetOperatorArguments {
            operator: operator.into(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateAccountArguments {
    /// Sender for operations that affect accounts other than the sender
    pub sender: AccountIdentifier,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetOperatorArguments {
    operator: AccountIdentifier,
}

/// Used for query operations to apply conditions.  Defaults to [`Operator::And`] if no value is
/// present
///
/// [API Spec](https://www.rosetta-api.org/docs/models/Operator.html)
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    And,
    Or,
}

impl Default for Operator {
    fn default() -> Self {
        Operator::And
    }
}

/// Public key used for the rosetta implementation.  All private keys will never be handled
/// in the Rosetta implementation.
///
/// [API Spec](https://www.rosetta-api.org/docs/models/PublicKey.html)
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublicKey {
    /// Hex encoded public key bytes
    pub hex_bytes: String,
    /// Curve type associated with the key
    pub curve_type: CurveType,
}

impl TryFrom<Ed25519PublicKey> for PublicKey {
    type Error = anyhow::Error;

    fn try_from(public_key: Ed25519PublicKey) -> Result<Self, Self::Error> {
        Ok(PublicKey {
            hex_bytes: public_key.to_encoded_string()?,
            curve_type: CurveType::Edwards25519,
        })
    }
}

impl TryFrom<PublicKey> for Ed25519PublicKey {
    type Error = anyhow::Error;

    fn try_from(public_key: PublicKey) -> Result<Self, Self::Error> {
        if public_key.curve_type != CurveType::Edwards25519 {
            return Err(anyhow!("Invalid curve type"));
        }

        Ok(Ed25519PublicKey::from_encoded_string(
            &public_key.hex_bytes,
        )?)
    }
}

/// Related Transaction allows for connecting related transactions across shards, networks or
/// other boundaries.
///
/// [API Spec](https://www.rosetta-api.org/docs/models/RelatedTransaction.html)
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RelatedTransaction {
    /// Network of transaction.  [`None`] means same network as original transaction
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_identifier: Option<NetworkIdentifier>,
    /// Transaction identifier of the related transaction
    pub transaction_identifier: TransactionIdentifier,
    /// Direction of the relation (forward or backward in time)
    pub direction: Direction,
}

/// Signature containing the signed payload and the encoded signed payload
///
/// [API Spec](https://www.rosetta-api.org/docs/models/Signature.html)
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Signature {
    /// Payload to be signed
    pub signing_payload: SigningPayload,
    /// Public key related to the signature
    pub public_key: PublicKey,
    /// Cryptographic signature type
    pub signature_type: SignatureType,
    /// Hex bytes of the signature
    pub hex_bytes: String,
}

/// Cryptographic signature type used for signing transactions.  Aptos only uses
/// [`SignatureType::Ed25519`]
///
/// [API Spec](https://www.rosetta-api.org/docs/models/SignatureType.html)
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureType {
    Ecdsa,
    EcdsaRecovery,
    Ed25519,
    #[serde(rename = "schnoor_1")]
    Schnoor1,
    SchnoorPoseidon,
}

/// Signing payload should be signed by the client with their own private key
///
/// [API Spec](https://www.rosetta-api.org/docs/models/SigningPayload.html)
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SigningPayload {
    /// Deprecated field, replaced with account_identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Account identifier of the signer
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_identifier: Option<AccountIdentifier>,
    /// Hex encoded string of payload bytes to be signed
    pub hex_bytes: String,
    /// Signature type to sign with
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_type: Option<SignatureType>,
}

/// A representation of a transaction by it's underlying operations (write set changes)
///
/// [API Spec](https://www.rosetta-api.org/docs/models/Transaction.html)
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Transaction {
    /// The identifying hash of the transaction
    pub transaction_identifier: TransactionIdentifier,
    /// Individual operations (write set changes) in a transaction
    pub operations: Vec<Operation>,
    /// Related transactions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_transactions: Option<Vec<RelatedTransaction>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<TransactionMetadata>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransactionMetadata {
    pub transaction_type: TransactionType,
    pub version: U64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TransactionType {
    User,
    Genesis,
    BlockMetadata,
    StateCheckpoint,
}

impl Display for TransactionType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        use TransactionType::*;
        f.write_str(match self {
            User => "User",
            Genesis => "Genesis",
            BlockMetadata => "BlockResource",
            StateCheckpoint => "StateCheckpoint",
        })
    }
}

impl Transaction {
    pub async fn from_transaction(txn: aptos_rest_client::Transaction) -> ApiResult<Transaction> {
        use aptos_rest_client::Transaction::*;
        let (txn_type, maybe_user_transaction_request, txn_info, events) = match txn {
            // Pending transactions aren't supported by Rosetta (for now)
            PendingTransaction(_) => return Err(ApiError::TransactionIsPending),
            UserTransaction(txn) => (
                TransactionType::User,
                Some(txn.request),
                txn.info,
                txn.events,
            ),
            GenesisTransaction(txn) => (TransactionType::Genesis, None, txn.info, txn.events),
            BlockMetadataTransaction(txn) => {
                (TransactionType::BlockMetadata, None, txn.info, txn.events)
            }
            StateCheckpointTransaction(txn) => {
                (TransactionType::StateCheckpoint, None, txn.info, vec![])
            }
        };

        // Operations must be sequential and operation index must always be in the same order
        // with no gaps
        let mut operations = vec![];
        let mut operation_index: u64 = 0;
        if txn_info.success {
            // Parse all operations from the writeset changes in a success
            for change in &txn_info.changes {
                let mut ops = parse_operations_from_write_set(
                    change,
                    &events,
                    &maybe_user_transaction_request,
                    operation_index,
                );
                operation_index += ops.len() as u64;
                operations.append(&mut ops);
            }
        } else {
            // Parse all failed operations from the payload
            if let Some(ref request) = maybe_user_transaction_request {
                let mut ops = parse_operations_from_txn_payload(
                    operation_index,
                    *request.sender.inner(),
                    &request.payload,
                );
                operation_index += ops.len() as u64;
                operations.append(&mut ops);
            }
        };

        // Everything committed costs gas
        if let Some(ref request) = maybe_user_transaction_request {
            operations.push(Operation::gas_fee(
                operation_index,
                *request.sender.inner(),
                txn_info.gas_used.0,
                request.gas_unit_price.0,
            ));
        }

        Ok(Transaction {
            transaction_identifier: (&txn_info).into(),
            operations,
            related_transactions: None,
            metadata: Some(TransactionMetadata {
                transaction_type: txn_type,
                version: txn_info.version,
            }),
        })
    }
}

/// Parses operations from the transaction payload
///
/// This case only occurs if the transaction failed, and that's because it's less accurate
/// than just following the state changes
fn parse_operations_from_txn_payload(
    operation_index: u64,
    sender: AccountAddress,
    payload: &TransactionPayload,
) -> Vec<Operation> {
    let mut operations = vec![];
    if let TransactionPayload::EntryFunctionPayload(inner) = payload {
        if AccountAddress::ONE == *inner.function.module.address.inner()
            && coin_module_identifier() == inner.function.module.name.0
            && transfer_function_identifier() == inner.function.name.0
        {
            if let Some(MoveType::Struct(MoveStructTag {
                address,
                module,
                name,
                ..
            })) = inner.type_arguments.first()
            {
                if *address.inner() == AccountAddress::ONE
                    && module.0 == aptos_coin_module_identifier()
                    && name.0 == aptos_coin_resource_identifier()
                {
                    let receiver =
                        serde_json::from_value::<Address>(inner.arguments.get(0).cloned().unwrap())
                            .unwrap();
                    let amount =
                        serde_json::from_value::<U64>(inner.arguments.get(1).cloned().unwrap())
                            .unwrap()
                            .0;
                    operations.push(Operation::withdraw(
                        operation_index,
                        Some(OperationStatusType::Failure),
                        sender,
                        native_coin(),
                        amount,
                    ));
                    operations.push(Operation::deposit(
                        operation_index + 1,
                        Some(OperationStatusType::Failure),
                        receiver.into(),
                        native_coin(),
                        amount,
                    ));
                }
            }
        } else if AccountAddress::ONE == *inner.function.module.address.inner()
            && account_module_identifier() == inner.function.module.name.0
            && create_account_function_identifier() == inner.function.name.0
        {
            let address =
                serde_json::from_value::<Address>(inner.arguments.get(0).cloned().unwrap())
                    .unwrap();
            operations.push(Operation::create_account(
                operation_index,
                Some(OperationStatusType::Failure),
                address.into(),
                sender,
            ));
        } else if AccountAddress::ONE == *inner.function.module.address.inner()
            && stake_module_identifier() == inner.function.module.name.0
            && set_operator_function_identifier() == inner.function.name.0
        {
            let operator =
                serde_json::from_value::<Address>(inner.arguments.get(0).cloned().unwrap())
                    .unwrap();
            operations.push(Operation::set_operator(
                operation_index,
                Some(OperationStatusType::Failure),
                operator.into(),
                sender,
            ));
        }
    }
    operations
}

/// Parses operations from the write set
///
/// This can only be done during a successful transaction because there are actual state changes.
/// It is more accurate because untracked scripts are included in balance operations
fn parse_operations_from_write_set(
    change: &WriteSetChange,
    events: &[Event],
    maybe_request: &Option<UserTransactionRequest>,
    mut operation_index: u64,
) -> Vec<Operation> {
    let mut operations = vec![];
    if let WriteSetChange::WriteResource(WriteResource { address, data, .. }) = change {
        // Determine operation
        let address = *address.inner();
        let account_tag = MoveStructTag::new(
            AccountAddress::ONE.into(),
            account_module_identifier().into(),
            account_resource_identifier().into(),
            vec![],
        );
        let coin_store_tag = MoveStructTag::new(
            AccountAddress::ONE.into(),
            coin_module_identifier().into(),
            coin_store_resource_identifier().into(),
            vec![native_coin_tag().into()],
        );

        let stake_pool_tag = MoveStructTag::new(
            AccountAddress::ONE.into(),
            stake_module_identifier().into(),
            stake_pool_resource_identifier().into(),
            vec![],
        );

        if data.typ == account_tag {
            // Account sequence number increase (possibly creation)
            // Find out if it's the 0th sequence number (creation)
            for (id, value) in data.data.0.iter() {
                if id.0 == sequence_number_field_identifier() {
                    if let Ok(U64(0)) = serde_json::from_value::<U64>(value.clone()) {
                        operations.push(Operation::create_account(
                            operation_index,
                            Some(OperationStatusType::Success),
                            address,
                            maybe_request
                                .as_ref()
                                .map(|inner| *inner.sender.inner())
                                .unwrap_or(AccountAddress::ONE),
                        ));
                        operation_index += 1;
                    }
                }
            }
        } else if data.typ == stake_pool_tag {
            // Account sequence number increase (possibly creation)
            // Find out if it's the 0th sequence number (creation)
            for (id, value) in data.data.0.iter() {
                if id.0 == set_operator_events_field_identifier() {
                    serde_json::from_value::<EventId>(value.clone()).unwrap();
                    if let Ok(event) = serde_json::from_value::<EventId>(value.clone()) {
                        let set_operator_event =
                            EventKey::new(event.guid.id.creation_num.0, event.guid.id.addr);
                        if let Some(operator) =
                            get_set_operator_from_event(events, set_operator_event)
                        {
                            operations.push(Operation::set_operator(
                                operation_index,
                                Some(OperationStatusType::Success),
                                address,
                                operator,
                            ));
                            operation_index += 1;
                        }
                    }
                }
            }
        } else if data.typ == coin_store_tag {
            // Account balance change
            for (id, value) in data.data.0.iter() {
                if id.0 == withdraw_events_field_identifier() {
                    serde_json::from_value::<EventId>(value.clone()).unwrap();
                    if let Ok(event) = serde_json::from_value::<EventId>(value.clone()) {
                        let withdraw_event =
                            EventKey::new(event.guid.id.creation_num.0, event.guid.id.addr);
                        if let Some(amount) = get_amount_from_event(events, withdraw_event) {
                            operations.push(Operation::withdraw(
                                operation_index,
                                Some(OperationStatusType::Success),
                                address,
                                native_coin(),
                                amount,
                            ));
                            operation_index += 1;
                        }
                    }
                } else if id.0 == deposit_events_field_identifier() {
                    serde_json::from_value::<EventId>(value.clone()).unwrap();
                    if let Ok(event) = serde_json::from_value::<EventId>(value.clone()) {
                        let withdraw_event =
                            EventKey::new(event.guid.id.creation_num.0, event.guid.id.addr);
                        if let Some(amount) = get_amount_from_event(events, withdraw_event) {
                            operations.push(Operation::deposit(
                                operation_index,
                                Some(OperationStatusType::Success),
                                address,
                                native_coin(),
                                amount,
                            ));
                            operation_index += 1;
                        }
                    }
                }
            }
        }
    }

    operations
}

/// Pulls the balance change from a withdraw or deposit event
fn get_amount_from_event(events: &[Event], event_key: EventKey) -> Option<u64> {
    if let Some(event) = events
        .iter()
        .find(|event| EventKey::from(event.key) == event_key)
    {
        if let Ok(CoinEvent { amount }) = serde_json::from_value::<CoinEvent>(event.data.clone()) {
            return Some(amount.0);
        }
    }

    None
}

fn get_set_operator_from_event(events: &[Event], event_key: EventKey) -> Option<AccountAddress> {
    if let Some(event) = events
        .iter()
        .find(|event| EventKey::from(event.key) == event_key)
    {
        if let Ok(SetOperatorEvent { new_operator, .. }) =
            serde_json::from_value::<SetOperatorEvent>(event.data.clone())
        {
            return Some(*new_operator.inner());
        }
    }

    None
}

/// An enum for processing which operation is in a transaction
pub enum OperationDetails {
    CreateAccount,
    TransferCoin {
        currency: Currency,
        withdraw_event_key: Option<EventKey>,
        deposit_event_key: Option<EventKey>,
    },
}

/// A holder for all information related to a specific transaction
/// built from [`Operation`]s
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum InternalOperation {
    CreateAccount(CreateAccount),
    Transfer(Transfer),
    SetOperator(SetOperator),
}

impl InternalOperation {
    /// Pulls the [`InternalOperation`] from the set of [`Operation`]
    pub fn extract(operations: &Vec<Operation>) -> ApiResult<InternalOperation> {
        match operations.len() {
            1 => {
                if let Some(operation) = operations.first() {
                    match OperationType::from_str(&operation.operation_type) {
                        Ok(OperationType::CreateAccount) => {
                            if let (
                                Some(OperationSpecificMetadata::CreateAccount(
                                    CreateAccountArguments { sender },
                                )),
                                Some(account),
                            ) = (&operation.metadata, &operation.account)
                            {
                                return Ok(Self::CreateAccount(CreateAccount {
                                    sender: sender.account_address()?,
                                    new_account: account.account_address()?,
                                }));
                            }
                        }
                        Ok(OperationType::SetOperator) => {
                            if let (
                                Some(OperationSpecificMetadata::SetOperator(
                                    SetOperatorArguments { operator },
                                )),
                                Some(account),
                            ) = (&operation.metadata, &operation.account)
                            {
                                return Ok(Self::SetOperator(SetOperator {
                                    owner: account.account_address()?,
                                    operator: operator.account_address()?,
                                }));
                            }
                        }
                        _ => {}
                    }
                }

                // Return invalid operations if for any reason parsing fails
                Err(ApiError::InvalidOperations)
            }
            2 => Ok(Self::Transfer(Transfer::extract_transfer(operations)?)),
            _ => Err(ApiError::InvalidOperations),
        }
    }

    /// The sender of the transaction
    pub fn sender(&self) -> AccountAddress {
        match self {
            Self::CreateAccount(inner) => inner.sender,
            Self::Transfer(inner) => inner.sender,
            Self::SetOperator(inner) => inner.owner,
        }
    }
}

/// Operation to create an account
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateAccount {
    pub sender: AccountAddress,
    pub new_account: AccountAddress,
}

/// Operation to transfer coins between accounts
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Transfer {
    pub sender: AccountAddress,
    pub receiver: AccountAddress,
    pub amount: u64,
    pub currency: Currency,
}

impl Transfer {
    pub fn extract_transfer(operations: &Vec<Operation>) -> ApiResult<Transfer> {
        // Only support 1:1 P2P transfer
        // This is composed of a Deposit and a Withdraw operation
        if operations.len() != 2 {
            return Err(ApiError::InvalidTransferOperations(Some(
                "Must have exactly 2 operations a withdraw and a deposit",
            )));
        }

        let mut op_map = HashMap::new();
        for op in operations {
            let op_type = OperationType::from_str(&op.operation_type)?;
            op_map.insert(op_type, op);
        }
        if !op_map.contains_key(&OperationType::Withdraw) {
            return Err(ApiError::InvalidTransferOperations(Some(
                "Must have a withdraw",
            )));
        }

        if !op_map.contains_key(&OperationType::Deposit) {
            return Err(ApiError::InvalidTransferOperations(Some(
                "Must have a deposit",
            )));
        }

        // Verify accounts and amounts
        let withdraw = op_map.get(&OperationType::Withdraw).unwrap();
        let sender = if let Some(ref account) = withdraw.account {
            account.try_into()?
        } else {
            return Err(ApiError::InvalidTransferOperations(Some(
                "Invalid withdraw account provided",
            )));
        };

        let deposit = op_map.get(&OperationType::Deposit).unwrap();
        let receiver = if let Some(ref account) = deposit.account {
            account.try_into()?
        } else {
            return Err(ApiError::InvalidTransferOperations(Some(
                "Invalid deposit account provided",
            )));
        };

        let (amount, currency): (u64, Currency) =
            if let (Some(withdraw_amount), Some(deposit_amount)) =
                (&withdraw.amount, &deposit.amount)
            {
                // Currencies have to be the same
                if withdraw_amount.currency != deposit_amount.currency {
                    return Err(ApiError::InvalidTransferOperations(Some(
                        "Currency mismatch between withdraw and deposit",
                    )));
                }

                // Check that the currency is supported
                // TODO: in future use currency, since there's more than just 1
                is_native_coin(&withdraw_amount.currency)?;

                let withdraw_value = i64::from_str(&withdraw_amount.value).map_err(|_| {
                    ApiError::InvalidTransferOperations(Some("Withdraw amount is invalid"))
                })?;
                let deposit_value = i64::from_str(&deposit_amount.value).map_err(|_| {
                    ApiError::InvalidTransferOperations(Some("Deposit amount is invalid"))
                })?;

                // We can't create or destroy coins, they must be negatives of each other
                if -withdraw_value != deposit_value {
                    return Err(ApiError::InvalidTransferOperations(Some(
                        "Withdraw amount must be equal to negative of deposit amount",
                    )));
                }

                (deposit_value as u64, deposit_amount.currency.clone())
            } else {
                return Err(ApiError::InvalidTransferOperations(Some(
                    "Must have exactly 1 withdraw and 1 deposit with amounts",
                )));
            };

        Ok(Transfer {
            sender,
            receiver,
            amount,
            currency,
        })
    }
}

/// Set operator
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetOperator {
    pub owner: AccountAddress,
    pub operator: AccountAddress,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CoinEvent {
    amount: U64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SetOperatorEvent {
    _pool_address: Address,
    _old_operator: Address,
    new_operator: Address,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EventId {
    guid: Id,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Id {
    id: EventKeyId,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EventKeyId {
    #[serde(deserialize_with = "deserialize_account_address")]
    addr: AccountAddress,
    creation_num: U64,
}

fn deserialize_account_address<'de, D>(
    deserializer: D,
) -> std::result::Result<AccountAddress, D::Error>
where
    D: Deserializer<'de>,
{
    if deserializer.is_human_readable() {
        let s = <String>::deserialize(deserializer)?;
        AccountAddress::from_hex_literal(&s).map_err(D::Error::custom)
    } else {
        // In order to preserve the Serde data model and help analysis tools,
        // make sure to wrap our value in a container with the same name
        // as the original type.
        #[derive(::serde::Deserialize)]
        #[serde(rename = "AccountAddress")]
        struct Value([u8; AccountAddress::LENGTH]);

        let value = Value::deserialize(deserializer)?;
        Ok(AccountAddress::new(value.0))
    }
}
