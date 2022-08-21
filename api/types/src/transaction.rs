// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use crate::{
    Address, EntryFunctionId, EventKey, HashValue, HexEncodedBytes, MoveModuleBytecode,
    MoveModuleId, MoveResource, MoveScriptBytecode, MoveStructTag, MoveType, MoveValue, U64,
};

use anyhow::{bail, Context as AnyhowContext};
use aptos_crypto::{
    ed25519::{self, Ed25519PublicKey},
    multi_ed25519::{self, MultiEd25519PublicKey},
};
use aptos_types::{
    account_address::AccountAddress,
    block_metadata::BlockMetadata,
    contract_event::{ContractEvent, EventWithVersion},
    transaction::{
        authenticator::{AccountAuthenticator, TransactionAuthenticator},
        Script, SignedTransaction, TransactionOutput, TransactionWithProof,
    },
};

use poem_openapi::{Object, Union};
use serde::{Deserialize, Serialize};
use std::{
    boxed::Box,
    convert::{From, Into, TryFrom, TryInto},
    fmt,
    str::FromStr,
};

// TODO: Add read_only / write_only (and their all variants) where appropriate.
// TODO: Investigate the use of discriminator_name, see https://github.com/poem-web/poem/issues/329.
// TODO: See https://github.com/poem-web/poem/issues/347 re mapping stuff. UPDATE: Wait for 2.0.7 to be released.

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TransactionData {
    OnChain(TransactionOnChainData),
    Pending(Box<SignedTransaction>),
}

impl From<TransactionOnChainData> for TransactionData {
    fn from(txn: TransactionOnChainData) -> Self {
        Self::OnChain(txn)
    }
}

impl From<SignedTransaction> for TransactionData {
    fn from(txn: SignedTransaction) -> Self {
        Self::Pending(Box::new(txn))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransactionOnChainData {
    pub version: u64,
    pub transaction: aptos_types::transaction::Transaction,
    pub info: aptos_types::transaction::TransactionInfo,
    pub events: Vec<ContractEvent>,
    pub accumulator_root_hash: aptos_crypto::HashValue,
    pub changes: aptos_types::write_set::WriteSet,
}

impl From<(TransactionWithProof, aptos_crypto::HashValue)> for TransactionOnChainData {
    fn from((txn, accumulator_root_hash): (TransactionWithProof, aptos_crypto::HashValue)) -> Self {
        Self {
            version: txn.version,
            transaction: txn.transaction,
            info: txn.proof.transaction_info,
            events: txn.events.unwrap_or_default(),
            accumulator_root_hash,
            changes: Default::default(),
        }
    }
}

impl
    From<(
        TransactionWithProof,
        aptos_crypto::HashValue,
        &TransactionOutput,
    )> for TransactionOnChainData
{
    fn from(
        (txn, accumulator_root_hash, txn_output): (
            TransactionWithProof,
            aptos_crypto::HashValue,
            &TransactionOutput,
        ),
    ) -> Self {
        Self {
            version: txn.version,
            transaction: txn.transaction,
            info: txn.proof.transaction_info,
            events: txn.events.unwrap_or_default(),
            accumulator_root_hash,
            changes: txn_output.write_set().clone(),
        }
    }
}

impl
    From<(
        u64,
        aptos_types::transaction::Transaction,
        aptos_types::transaction::TransactionInfo,
        Vec<ContractEvent>,
        aptos_crypto::HashValue,
        aptos_types::write_set::WriteSet,
    )> for TransactionOnChainData
{
    fn from(
        (version, transaction, info, events, accumulator_root_hash, write_set): (
            u64,
            aptos_types::transaction::Transaction,
            aptos_types::transaction::TransactionInfo,
            Vec<ContractEvent>,
            aptos_crypto::HashValue,
            aptos_types::write_set::WriteSet,
        ),
    ) -> Self {
        Self {
            version,
            transaction,
            info,
            events,
            accumulator_root_hash,
            changes: write_set,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Union)]
#[serde(tag = "type", rename_all = "snake_case")]
#[oai(one_of, discriminator_name = "type", rename_all = "snake_case")]
pub enum Transaction {
    PendingTransaction(PendingTransaction),
    UserTransaction(Box<UserTransaction>),
    GenesisTransaction(GenesisTransaction),
    BlockMetadataTransaction(BlockMetadataTransaction),
    StateCheckpointTransaction(StateCheckpointTransaction),
}

impl Transaction {
    pub fn timestamp(&self) -> u64 {
        match self {
            Transaction::UserTransaction(txn) => txn.timestamp.0,
            Transaction::BlockMetadataTransaction(txn) => txn.timestamp.0,
            Transaction::PendingTransaction(_) => 0,
            Transaction::GenesisTransaction(_) => 0,
            Transaction::StateCheckpointTransaction(txn) => txn.timestamp.0,
        }
    }

    pub fn version(&self) -> Option<u64> {
        match self {
            Transaction::UserTransaction(txn) => Some(txn.info.version.into()),
            Transaction::BlockMetadataTransaction(txn) => Some(txn.info.version.into()),
            Transaction::PendingTransaction(_) => None,
            Transaction::GenesisTransaction(txn) => Some(txn.info.version.into()),
            Transaction::StateCheckpointTransaction(txn) => Some(txn.info.version.into()),
        }
    }

    pub fn success(&self) -> bool {
        match self {
            Transaction::UserTransaction(txn) => txn.info.success,
            Transaction::BlockMetadataTransaction(txn) => txn.info.success,
            Transaction::PendingTransaction(_txn) => false,
            Transaction::GenesisTransaction(txn) => txn.info.success,
            Transaction::StateCheckpointTransaction(txn) => txn.info.success,
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, Transaction::PendingTransaction(_))
    }

    pub fn vm_status(&self) -> String {
        match self {
            Transaction::UserTransaction(txn) => txn.info.vm_status.clone(),
            Transaction::BlockMetadataTransaction(txn) => txn.info.vm_status.clone(),
            Transaction::PendingTransaction(_txn) => "pending".to_owned(),
            Transaction::GenesisTransaction(txn) => txn.info.vm_status.clone(),
            Transaction::StateCheckpointTransaction(txn) => txn.info.vm_status.clone(),
        }
    }

    pub fn type_str(&self) -> &'static str {
        match self {
            Transaction::PendingTransaction(_) => "pending_transaction",
            Transaction::UserTransaction(_) => "user_transaction",
            Transaction::GenesisTransaction(_) => "genesis_transaction",
            Transaction::BlockMetadataTransaction(_) => "block_metadata_transaction",
            Transaction::StateCheckpointTransaction(_) => "state_checkpoint_transaction",
        }
    }

    pub fn transaction_info(&self) -> anyhow::Result<&TransactionInfo> {
        Ok(match self {
            Transaction::UserTransaction(txn) => &txn.info,
            Transaction::BlockMetadataTransaction(txn) => &txn.info,
            Transaction::PendingTransaction(_txn) => {
                bail!("pending transaction does not have TransactionInfo")
            }
            Transaction::GenesisTransaction(txn) => &txn.info,
            Transaction::StateCheckpointTransaction(txn) => &txn.info,
        })
    }
}

// TODO: Remove this when we cut over to the new API fully.
impl From<(SignedTransaction, TransactionPayload)> for Transaction {
    fn from((txn, payload): (SignedTransaction, TransactionPayload)) -> Self {
        Transaction::PendingTransaction(PendingTransaction {
            request: (&txn, payload).into(),
            hash: txn.committed_hash().into(),
        })
    }
}

impl
    From<(
        &SignedTransaction,
        TransactionInfo,
        TransactionPayload,
        Vec<Event>,
        u64,
    )> for Transaction
{
    fn from(
        (txn, info, payload, events, timestamp): (
            &SignedTransaction,
            TransactionInfo,
            TransactionPayload,
            Vec<Event>,
            u64,
        ),
    ) -> Self {
        Transaction::UserTransaction(Box::new(UserTransaction {
            info,
            request: (txn, payload).into(),
            events,
            timestamp: timestamp.into(),
        }))
    }
}

impl From<(TransactionInfo, WriteSetPayload, Vec<Event>)> for Transaction {
    fn from((info, payload, events): (TransactionInfo, WriteSetPayload, Vec<Event>)) -> Self {
        Transaction::GenesisTransaction(GenesisTransaction {
            info,
            payload: GenesisPayload::WriteSetPayload(payload),
            events,
        })
    }
}

impl From<(&BlockMetadata, TransactionInfo, Vec<Event>)> for Transaction {
    fn from((txn, info, events): (&BlockMetadata, TransactionInfo, Vec<Event>)) -> Self {
        Transaction::BlockMetadataTransaction(BlockMetadataTransaction {
            info,
            id: txn.id().into(),
            epoch: txn.epoch().into(),
            round: txn.round().into(),
            events,
            previous_block_votes_bitvec: txn.previous_block_votes_bitvec().clone(),
            proposer: txn.proposer().into(),
            failed_proposer_indices: txn.failed_proposer_indices().clone(),
            timestamp: txn.timestamp_usecs().into(),
        })
    }
}

impl From<(&SignedTransaction, TransactionPayload)> for UserTransactionRequest {
    fn from((txn, payload): (&SignedTransaction, TransactionPayload)) -> Self {
        Self {
            sender: txn.sender().into(),
            sequence_number: txn.sequence_number().into(),
            max_gas_amount: txn.max_gas_amount().into(),
            gas_unit_price: txn.gas_unit_price().into(),
            expiration_timestamp_secs: txn.expiration_timestamp_secs().into(),
            signature: Some(txn.authenticator().into()),
            payload,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct TransactionInfo {
    pub version: U64,
    pub hash: HashValue,
    pub state_change_hash: HashValue,
    pub event_root_hash: HashValue,
    pub state_checkpoint_hash: Option<HashValue>,
    pub gas_used: U64,
    pub success: bool,
    pub vm_status: String,
    pub accumulator_root_hash: HashValue,
    pub changes: Vec<WriteSetChange>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct PendingTransaction {
    pub hash: HashValue,
    #[serde(flatten)]
    #[oai(flatten)]
    pub request: UserTransactionRequest,
}

impl From<(SignedTransaction, TransactionPayload)> for PendingTransaction {
    fn from((txn, payload): (SignedTransaction, TransactionPayload)) -> Self {
        PendingTransaction {
            request: (&txn, payload).into(),
            hash: txn.committed_hash().into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct UserTransaction {
    #[serde(flatten)]
    #[oai(flatten)]
    pub info: TransactionInfo,
    #[serde(flatten)]
    #[oai(flatten)]
    pub request: UserTransactionRequest,
    pub events: Vec<Event>,
    pub timestamp: U64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct StateCheckpointTransaction {
    #[serde(flatten)]
    #[oai(flatten)]
    pub info: TransactionInfo,
    pub timestamp: U64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct SubmitTransactionRequest {
    #[serde(flatten)]
    #[oai(flatten)]
    pub user_transaction_request: UserTransactionRequestInner,

    pub signature: TransactionSignature,
}

// TODO: Rename this to remove the Inner when we cut over.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct UserTransactionRequestInner {
    pub sender: Address,
    pub sequence_number: U64,
    pub max_gas_amount: U64,
    pub gas_unit_price: U64,
    pub expiration_timestamp_secs: U64,
    pub payload: TransactionPayload,
}

// TODO: Remove this when we cut over.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct UserTransactionRequest {
    pub sender: Address,
    pub sequence_number: U64,
    pub max_gas_amount: U64,
    pub gas_unit_price: U64,
    pub expiration_timestamp_secs: U64,
    pub payload: TransactionPayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<TransactionSignature>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct UserCreateSigningMessageRequest {
    #[serde(flatten)]
    #[oai(flatten)]
    pub transaction: UserTransactionRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_signers: Option<Vec<Address>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct EncodeSubmissionRequest {
    #[serde(flatten)]
    #[oai(flatten)]
    pub transaction: UserTransactionRequestInner,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_signers: Option<Vec<Address>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct GenesisTransaction {
    #[serde(flatten)]
    #[oai(flatten)]
    pub info: TransactionInfo,
    pub payload: GenesisPayload,
    pub events: Vec<Event>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct BlockMetadataTransaction {
    #[serde(flatten)]
    #[oai(flatten)]
    pub info: TransactionInfo,
    pub id: HashValue,
    pub epoch: U64,
    pub round: U64,
    pub events: Vec<Event>,
    pub previous_block_votes_bitvec: Vec<u8>,
    pub proposer: Address,
    pub failed_proposer_indices: Vec<u32>,
    pub timestamp: U64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct Event {
    pub key: EventKey,
    pub sequence_number: U64,
    #[serde(rename = "type")]
    #[oai(rename = "type")]
    pub typ: MoveType,
    // TODO: Use the real data here, not a JSON representation.
    pub data: serde_json::Value,
}

impl From<(&ContractEvent, serde_json::Value)> for Event {
    fn from((event, data): (&ContractEvent, serde_json::Value)) -> Self {
        match event {
            ContractEvent::V0(v0) => Self {
                key: (*v0.key()).into(),
                sequence_number: v0.sequence_number().into(),
                typ: v0.type_tag().clone().into(),
                data,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct VersionedEvent {
    pub version: U64,
    pub key: EventKey,
    pub sequence_number: U64,
    #[serde(rename = "type")]
    #[oai(rename = "type")]
    pub typ: MoveType,
    // TODO: Use the real data here, not a JSON representation.
    pub data: serde_json::Value,
}

impl From<(&EventWithVersion, serde_json::Value)> for VersionedEvent {
    fn from((event, data): (&EventWithVersion, serde_json::Value)) -> Self {
        match &event.event {
            ContractEvent::V0(v0) => Self {
                version: event.transaction_version.into(),
                key: (*v0.key()).into(),
                sequence_number: v0.sequence_number().into(),
                typ: v0.type_tag().clone().into(),
                data,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Union)]
#[serde(tag = "type", rename_all = "snake_case")]
#[oai(one_of, discriminator_name = "type", rename_all = "snake_case")]
pub enum GenesisPayload {
    WriteSetPayload(WriteSetPayload),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Union)]
#[serde(tag = "type", rename_all = "snake_case")]
#[oai(one_of, discriminator_name = "type", rename_all = "snake_case")]
pub enum TransactionPayload {
    EntryFunctionPayload(EntryFunctionPayload),
    ScriptPayload(ScriptPayload),
    ModuleBundlePayload(ModuleBundlePayload),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct ModuleBundlePayload {
    pub modules: Vec<MoveModuleBytecode>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct EntryFunctionPayload {
    pub function: EntryFunctionId,
    pub type_arguments: Vec<MoveType>,
    // TODO: Use the real data here, not a JSON representation.
    pub arguments: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct ScriptPayload {
    pub code: MoveScriptBytecode,
    pub type_arguments: Vec<MoveType>,
    // TODO: Use the real data here, not a JSON representation.
    pub arguments: Vec<serde_json::Value>,
}

impl TryFrom<Script> for ScriptPayload {
    type Error = anyhow::Error;

    fn try_from(script: Script) -> anyhow::Result<Self> {
        let (code, ty_args, args) = script.into_inner();
        Ok(Self {
            code: MoveScriptBytecode::new(code).try_parse_abi(),
            type_arguments: ty_args.into_iter().map(|arg| arg.into()).collect(),
            arguments: args
                .into_iter()
                .map(|arg| MoveValue::from(arg).json())
                .collect::<anyhow::Result<_>>()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct WriteSetPayload {
    pub write_set: WriteSet,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Union)]
#[serde(tag = "type", rename_all = "snake_case")]
#[oai(one_of, discriminator_name = "type", rename_all = "snake_case")]
pub enum WriteSet {
    ScriptWriteSet(ScriptWriteSet),
    DirectWriteSet(DirectWriteSet),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct ScriptWriteSet {
    pub execute_as: Address,
    pub script: ScriptPayload,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct DirectWriteSet {
    pub changes: Vec<WriteSetChange>,
    pub events: Vec<Event>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Union)]
#[serde(tag = "type", rename_all = "snake_case")]
#[oai(one_of, discriminator_name = "type", rename_all = "snake_case")]
pub enum WriteSetChange {
    DeleteModule(DeleteModule),
    DeleteResource(DeleteResource),
    DeleteTableItem(DeleteTableItem),
    WriteModule(WriteModule),
    WriteResource(WriteResource),
    WriteTableItem(WriteTableItem),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct DeleteModule {
    pub address: Address,
    pub state_key_hash: String,
    pub module: MoveModuleId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct DeleteResource {
    pub address: Address,
    pub state_key_hash: String,
    pub resource: MoveStructTag,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct DeleteTableItem {
    pub state_key_hash: String,
    pub handle: HexEncodedBytes,
    pub key: HexEncodedBytes,
    // This is optional, and only possible to populate if the table indexer is enabled for this node
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub data: Option<DeletedTableData>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct WriteModule {
    pub address: Address,
    pub state_key_hash: String,
    pub data: MoveModuleBytecode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct WriteResource {
    pub address: Address,
    pub state_key_hash: String,
    pub data: MoveResource,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct DecodedTableData {
    pub key: serde_json::Value,
    pub key_type: String,
    pub value: serde_json::Value,
    pub value_type: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct DeletedTableData {
    pub key: serde_json::Value,
    pub key_type: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct WriteTableItem {
    pub state_key_hash: String,
    pub handle: HexEncodedBytes,
    pub key: HexEncodedBytes,
    pub value: HexEncodedBytes,
    // This is optional, and only possible to populate if the table indexer is enabled for this node
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub data: Option<DecodedTableData>,
}

impl WriteSetChange {
    pub fn type_str(&self) -> &'static str {
        match self {
            WriteSetChange::DeleteModule { .. } => "delete_module",
            WriteSetChange::DeleteResource { .. } => "delete_resource",
            WriteSetChange::DeleteTableItem { .. } => "delete_table_item",
            WriteSetChange::WriteModule { .. } => "write_module",
            WriteSetChange::WriteResource { .. } => "write_resource",
            WriteSetChange::WriteTableItem { .. } => "write_table_item",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Union)]
#[serde(tag = "type", rename_all = "snake_case")]
#[oai(one_of, discriminator_name = "type", rename_all = "snake_case")]
pub enum TransactionSignature {
    Ed25519Signature(Ed25519Signature),
    MultiEd25519Signature(MultiEd25519Signature),
    MultiAgentSignature(MultiAgentSignature),
}

impl TryFrom<TransactionSignature> for TransactionAuthenticator {
    type Error = anyhow::Error;
    fn try_from(ts: TransactionSignature) -> anyhow::Result<Self> {
        Ok(match ts {
            TransactionSignature::Ed25519Signature(sig) => sig.try_into()?,
            TransactionSignature::MultiEd25519Signature(sig) => sig.try_into()?,
            TransactionSignature::MultiAgentSignature(sig) => sig.try_into()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct Ed25519Signature {
    pub public_key: HexEncodedBytes,
    pub signature: HexEncodedBytes,
}

impl TryFrom<Ed25519Signature> for TransactionAuthenticator {
    type Error = anyhow::Error;

    fn try_from(value: Ed25519Signature) -> Result<Self, Self::Error> {
        let Ed25519Signature {
            public_key,
            signature,
        } = value;
        Ok(TransactionAuthenticator::ed25519(
            public_key
                .inner()
                .try_into()
                .context("Failed to parse given public_key bytes as a Ed25519PublicKey")?,
            signature
                .inner()
                .try_into()
                .context("Failed to parse given signature as a Ed25519Signature")?,
        ))
    }
}

impl TryFrom<Ed25519Signature> for AccountAuthenticator {
    type Error = anyhow::Error;

    fn try_from(value: Ed25519Signature) -> Result<Self, Self::Error> {
        let Ed25519Signature {
            public_key,
            signature,
        } = value;
        Ok(AccountAuthenticator::ed25519(
            public_key
                .inner()
                .try_into()
                .context("Failed to parse given public_key bytes as a Ed25519PublicKey")?,
            signature
                .inner()
                .try_into()
                .context("Failed to parse given signature as a Ed25519Signature")?,
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct MultiEd25519Signature {
    pub public_keys: Vec<HexEncodedBytes>,
    pub signatures: Vec<HexEncodedBytes>,
    pub threshold: u8,
    pub bitmap: HexEncodedBytes,
}

impl TryFrom<MultiEd25519Signature> for TransactionAuthenticator {
    type Error = anyhow::Error;

    fn try_from(value: MultiEd25519Signature) -> Result<Self, Self::Error> {
        let MultiEd25519Signature {
            public_keys,
            signatures,
            threshold,
            bitmap,
        } = value;

        let ed25519_public_keys = public_keys
            .into_iter()
            .map(|s| Ok(s.inner().try_into()?))
            .collect::<anyhow::Result<_>>()?;
        let ed25519_signatures = signatures
            .into_iter()
            .map(|s| Ok(s.inner().try_into()?))
            .collect::<anyhow::Result<_>>()?;

        Ok(TransactionAuthenticator::multi_ed25519(
            MultiEd25519PublicKey::new(ed25519_public_keys, threshold)?,
            aptos_crypto::multi_ed25519::MultiEd25519Signature::new_with_signatures_and_bitmap(
                ed25519_signatures,
                bitmap.inner().try_into()?,
            ),
        ))
    }
}

impl TryFrom<MultiEd25519Signature> for AccountAuthenticator {
    type Error = anyhow::Error;

    fn try_from(value: MultiEd25519Signature) -> Result<Self, Self::Error> {
        let MultiEd25519Signature {
            public_keys,
            signatures,
            threshold,
            bitmap,
        } = value;

        let ed25519_public_keys = public_keys
            .into_iter()
            .map(|s| Ok(s.inner().try_into()?))
            .collect::<anyhow::Result<_>>()?;
        let ed25519_signatures = signatures
            .into_iter()
            .map(|s| Ok(s.inner().try_into()?))
            .collect::<anyhow::Result<_>>()?;

        Ok(AccountAuthenticator::multi_ed25519(
            MultiEd25519PublicKey::new(ed25519_public_keys, threshold)?,
            aptos_crypto::multi_ed25519::MultiEd25519Signature::new_with_signatures_and_bitmap(
                ed25519_signatures,
                bitmap.inner().try_into()?,
            ),
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Union)]
#[serde(tag = "type", rename_all = "snake_case")]
#[oai(one_of, discriminator_name = "type", rename_all = "snake_case")]
pub enum AccountSignature {
    Ed25519Signature(Ed25519Signature),
    MultiEd25519Signature(MultiEd25519Signature),
}

impl TryFrom<AccountSignature> for AccountAuthenticator {
    type Error = anyhow::Error;

    fn try_from(sig: AccountSignature) -> anyhow::Result<Self> {
        Ok(match sig {
            AccountSignature::Ed25519Signature(s) => s.try_into()?,
            AccountSignature::MultiEd25519Signature(s) => s.try_into()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct MultiAgentSignature {
    pub sender: AccountSignature,
    pub secondary_signer_addresses: Vec<Address>,
    pub secondary_signers: Vec<AccountSignature>,
}

impl TryFrom<MultiAgentSignature> for TransactionAuthenticator {
    type Error = anyhow::Error;

    fn try_from(value: MultiAgentSignature) -> Result<Self, Self::Error> {
        let MultiAgentSignature {
            sender,
            secondary_signer_addresses,
            secondary_signers,
        } = value;
        Ok(TransactionAuthenticator::multi_agent(
            sender.try_into()?,
            secondary_signer_addresses
                .into_iter()
                .map(|a| a.into())
                .collect(),
            secondary_signers
                .into_iter()
                .map(|s| s.try_into())
                .collect::<anyhow::Result<_>>()?,
        ))
    }
}

impl From<(&Ed25519PublicKey, &ed25519::Ed25519Signature)> for Ed25519Signature {
    fn from((pk, sig): (&Ed25519PublicKey, &ed25519::Ed25519Signature)) -> Self {
        Self {
            public_key: pk.to_bytes().to_vec().into(),
            signature: sig.to_bytes().to_vec().into(),
        }
    }
}

impl
    From<(
        &MultiEd25519PublicKey,
        &multi_ed25519::MultiEd25519Signature,
    )> for MultiEd25519Signature
{
    fn from(
        (pk, sig): (
            &MultiEd25519PublicKey,
            &multi_ed25519::MultiEd25519Signature,
        ),
    ) -> Self {
        Self {
            public_keys: pk
                .public_keys()
                .iter()
                .map(|k| k.to_bytes().to_vec().into())
                .collect(),
            signatures: sig
                .signatures()
                .iter()
                .map(|s| s.to_bytes().to_vec().into())
                .collect(),
            threshold: *pk.threshold(),
            bitmap: sig.bitmap().to_vec().into(),
        }
    }
}

impl From<&AccountAuthenticator> for AccountSignature {
    fn from(auth: &AccountAuthenticator) -> Self {
        use AccountAuthenticator::*;
        match auth {
            Ed25519 {
                public_key,
                signature,
            } => Self::Ed25519Signature((public_key, signature).into()),
            MultiEd25519 {
                public_key,
                signature,
            } => Self::MultiEd25519Signature((public_key, signature).into()),
        }
    }
}

impl
    From<(
        &AccountAuthenticator,
        &Vec<AccountAddress>,
        &Vec<AccountAuthenticator>,
    )> for MultiAgentSignature
{
    fn from(
        (sender, addresses, signers): (
            &AccountAuthenticator,
            &Vec<AccountAddress>,
            &Vec<AccountAuthenticator>,
        ),
    ) -> Self {
        Self {
            sender: sender.into(),
            secondary_signer_addresses: addresses.iter().map(|address| (*address).into()).collect(),
            secondary_signers: signers.iter().map(|s| s.into()).collect(),
        }
    }
}

impl From<TransactionAuthenticator> for TransactionSignature {
    fn from(auth: TransactionAuthenticator) -> Self {
        use TransactionAuthenticator::*;
        match &auth {
            Ed25519 {
                public_key,
                signature,
            } => Self::Ed25519Signature((public_key, signature).into()),
            MultiEd25519 {
                public_key,
                signature,
            } => Self::MultiEd25519Signature((public_key, signature).into()),
            MultiAgent {
                sender,
                secondary_signer_addresses,
                secondary_signers,
            } => Self::MultiAgentSignature(
                (sender, secondary_signer_addresses, secondary_signers).into(),
            ),
        }
    }
}

/// There are 2 types transaction ids from HTTP request inputs:
/// 1. Transaction hash: hex-encoded string, e.g. "0x374eda71dce727c6cd2dd4a4fd47bfb85c16be2e3e95ab0df4948f39e1af9981"
/// 2. Transaction version: u64 number string (as we encode u64 into string in JSON), e.g. "122"
#[derive(Clone, Debug, Union)]
#[oai(one_of, discriminator_name = "type", rename_all = "snake_case")]
pub enum TransactionId {
    Hash(HashValue),
    Version(U64),
}

impl FromStr for TransactionId {
    type Err = anyhow::Error;

    fn from_str(hash_or_version: &str) -> Result<Self, anyhow::Error> {
        let id = match hash_or_version.parse::<u64>() {
            Ok(version) => TransactionId::Version(U64::from(version)),
            Err(_) => TransactionId::Hash(hash_or_version.parse()?),
        };
        Ok(id)
    }
}

impl fmt::Display for TransactionId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Hash(h) => write!(f, "hash({})", h),
            Self::Version(v) => write!(f, "version({})", v),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Object)]
pub struct TransactionSigningMessage {
    pub message: HexEncodedBytes,
}

impl TransactionSigningMessage {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            message: bytes.into(),
        }
    }
}
