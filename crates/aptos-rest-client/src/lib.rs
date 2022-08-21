// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

pub mod aptos;
pub mod error;
pub mod faucet;

pub use faucet::FaucetClient;
use std::collections::BTreeMap;
pub mod response;
pub use response::Response;
pub mod state;
pub mod types;

pub use aptos_api_types::{
    self, IndexResponse, MoveModuleBytecode, PendingTransaction, Transaction,
};
pub use state::State;
pub use types::{Account, Resource};

use crate::aptos::{AptosVersion, Balance};
use anyhow::{anyhow, Result};
use aptos_api_types::mime_types::BCS;
use aptos_api_types::{
    mime_types::BCS_SIGNED_TRANSACTION as BCS_CONTENT_TYPE, AptosError, BcsBlock, Block,
    HexEncodedBytes, MoveModuleId, TransactionData, TransactionOnChainData, UserTransaction,
    VersionedEvent,
};
use aptos_crypto::HashValue;
use aptos_types::account_config::AccountResource;
use aptos_types::contract_event::EventWithVersion;
use aptos_types::transaction::ExecutionStatus;
use aptos_types::{
    account_address::AccountAddress,
    account_config::{NewBlockEvent, CORE_CODE_ADDRESS},
    transaction::SignedTransaction,
};
use move_deps::move_core_types::language_storage::StructTag;
use poem_openapi::types::ParseFromJSON;
use reqwest::header::ACCEPT;
use reqwest::{header::CONTENT_TYPE, Client as ReqwestClient, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use types::{deserialize_from_prefixed_hex_string, deserialize_from_string};
use url::Url;

pub const USER_AGENT: &str = concat!("aptos-client-sdk-rust / ", env!("CARGO_PKG_VERSION"));
pub const DEFAULT_VERSION_PATH_BASE: &str = "v1/";

#[derive(Clone, Debug)]
pub struct Client {
    inner: ReqwestClient,
    base_url: Url,
    version_path_base: String,
}

impl Client {
    pub fn new(base_url: Url) -> Self {
        let inner = ReqwestClient::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .cookie_store(true)
            .build()
            .unwrap();

        // If the user provided no version in the path, use the default. If the
        // provided version has no trailing slash, add it, otherwise url.join
        // will ignore the version path base.
        let version_path_base = match base_url.path() {
            "/" => DEFAULT_VERSION_PATH_BASE.to_string(),
            path => {
                if !path.ends_with('/') {
                    format!("{}/", path)
                } else {
                    path.to_string()
                }
            }
        };

        Self {
            inner,
            base_url,
            version_path_base,
        }
    }

    /// Set a different version path base, e.g. "v1/" See
    /// DEFAULT_VERSION_PATH_BASE for the default value.
    pub fn version_path_base(mut self, version_path_base: String) -> Result<Self> {
        if !version_path_base.ends_with('/') {
            return Err(anyhow!("version_path_base must end with '/', e.g. 'v1/'"));
        }
        self.version_path_base = version_path_base;
        Ok(self)
    }

    fn build_path(&self, path: &str) -> Result<Url> {
        Ok(self.base_url.join(&self.version_path_base)?.join(path)?)
    }

    pub async fn get_aptos_version(&self) -> Result<Response<AptosVersion>> {
        self.get_resource::<AptosVersion>(CORE_CODE_ADDRESS, "0x1::version::Version")
            .await
    }

    pub async fn get_block_by_height(
        &self,
        height: u64,
        with_transactions: bool,
    ) -> Result<Response<Block>> {
        self.get(self.build_path(&format!(
            "blocks/by_height/{}?with_transactions={}",
            height, with_transactions
        ))?)
        .await
    }

    pub async fn get_block_by_height_bcs(
        &self,
        height: u64,
        with_transactions: bool,
    ) -> Result<Response<BcsBlock>> {
        let url = self.build_path(&format!(
            "blocks/by_height/{}?with_transactions={}",
            height, with_transactions
        ))?;
        let response = self.get_bcs(url).await?;
        Ok(response.and_then(|inner| bcs::from_bytes(&inner))?)
    }

    pub async fn get_block_by_version(
        &self,
        version: u64,
        with_transactions: bool,
    ) -> Result<Response<Block>> {
        self.get(self.build_path(&format!(
            "blocks/by_version/{}?with_transactions={}",
            version, with_transactions
        ))?)
        .await
    }

    pub async fn get_block_by_version_bcs(
        &self,
        height: u64,
        with_transactions: bool,
    ) -> Result<Response<BcsBlock>> {
        let url = self.build_path(&format!(
            "blocks/by_version/{}?with_transactions={}",
            height, with_transactions
        ))?;
        let response = self.get_bcs(url).await?;
        Ok(response.and_then(|inner| bcs::from_bytes(&inner))?)
    }

    pub async fn get_account_balance(&self, address: AccountAddress) -> Result<Response<Balance>> {
        let resp = self
            .get_account_resource(address, "0x1::coin::CoinStore<0x1::aptos_coin::AptosCoin>")
            .await?;
        resp.and_then(|resource| {
            if let Some(res) = resource {
                Ok(serde_json::from_value::<Balance>(res.data)?)
            } else {
                Err(anyhow!("No data returned"))
            }
        })
    }

    pub async fn get_account_balance_at_version(
        &self,
        address: AccountAddress,
        version: u64,
    ) -> Result<Response<Balance>> {
        let resp = self
            .get_account_resource_at_version(
                address,
                "0x1::coin::CoinStore<0x1::aptos_coin::AptosCoin>",
                version,
            )
            .await?;
        resp.and_then(|resource| {
            if let Some(res) = resource {
                Ok(serde_json::from_value::<Balance>(res.data)?)
            } else {
                Err(anyhow!("No data returned"))
            }
        })
    }

    pub async fn get_index(&self) -> Result<Response<IndexResponse>> {
        self.get(self.build_path("")?).await
    }

    pub async fn get_ledger_information(&self) -> Result<Response<State>> {
        let response = self.get_index().await?.map(|r| State {
            chain_id: r.chain_id,
            epoch: r.epoch.into(),
            version: r.ledger_version.into(),
            timestamp_usecs: r.ledger_timestamp.into(),
            oldest_ledger_version: r.oldest_ledger_version.into(),
            oldest_block_height: r.oldest_block_height.into(),
            block_height: r.block_height.into(),
        });

        Ok(response)
    }

    pub async fn simulate(
        &self,
        txn: &SignedTransaction,
    ) -> Result<Response<Vec<UserTransaction>>> {
        let txn_payload = bcs::to_bytes(txn)?;
        let url = self.build_path("transactions/simulate")?;

        let response = self
            .inner
            .post(url)
            .header(CONTENT_TYPE, BCS_CONTENT_TYPE)
            .body(txn_payload)
            .send()
            .await?;

        self.json(response).await
    }

    pub async fn simulate_bcs(
        &self,
        txn: &SignedTransaction,
    ) -> Result<Response<TransactionOnChainData>> {
        let txn_payload = bcs::to_bytes(txn)?;
        let url = self.build_path("transactions/simulate")?;

        let response = self
            .inner
            .post(url)
            .header(CONTENT_TYPE, BCS_CONTENT_TYPE)
            .header(ACCEPT, BCS)
            .body(txn_payload)
            .send()
            .await?;

        let response = self.check_and_parse_bcs_response(response).await?;
        Ok(response.and_then(|bytes| bcs::from_bytes(&bytes))?)
    }

    pub async fn submit(&self, txn: &SignedTransaction) -> Result<Response<PendingTransaction>> {
        let txn_payload = bcs::to_bytes(txn)?;
        let url = self.build_path("transactions")?;

        let response = self
            .inner
            .post(url)
            .header(CONTENT_TYPE, BCS_CONTENT_TYPE)
            .body(txn_payload)
            .send()
            .await?;

        self.json(response).await
    }

    pub async fn submit_bcs(&self, txn: &SignedTransaction) -> Result<Response<()>> {
        let txn_payload = bcs::to_bytes(txn)?;
        let url = self.build_path("transactions")?;

        let response = self
            .inner
            .post(url)
            .header(CONTENT_TYPE, BCS_CONTENT_TYPE)
            .header(ACCEPT, BCS)
            .body(txn_payload)
            .send()
            .await?;

        let response = self.check_and_parse_bcs_response(response).await?;
        Ok(response.and_then(|bytes| bcs::from_bytes(&bytes))?)
    }

    pub async fn submit_and_wait(&self, txn: &SignedTransaction) -> Result<Response<Transaction>> {
        self.submit(txn).await?;
        self.wait_for_signed_transaction(txn).await
    }

    pub async fn submit_and_wait_bcs(
        &self,
        txn: &SignedTransaction,
    ) -> Result<Response<TransactionOnChainData>, (Option<Response<TransactionData>>, anyhow::Error)>
    {
        self.submit_bcs(txn).await.map_err(|err| (None, err))?;
        self.wait_for_signed_transaction_bcs(txn).await
    }

    pub async fn wait_for_transaction(
        &self,
        pending_transaction: &PendingTransaction,
    ) -> Result<Response<Transaction>> {
        self.wait_for_transaction_by_hash(
            pending_transaction.hash.into(),
            *pending_transaction
                .request
                .expiration_timestamp_secs
                .inner(),
        )
        .await
    }

    pub async fn wait_for_signed_transaction(
        &self,
        transaction: &SignedTransaction,
    ) -> Result<Response<Transaction>> {
        let expiration_timestamp = transaction.expiration_timestamp_secs();
        self.wait_for_transaction_by_hash(
            transaction.clone().committed_hash(),
            expiration_timestamp,
        )
        .await
    }

    pub async fn wait_for_signed_transaction_bcs(
        &self,
        transaction: &SignedTransaction,
    ) -> Result<Response<TransactionOnChainData>, (Option<Response<TransactionData>>, anyhow::Error)>
    {
        let expiration_timestamp = transaction.expiration_timestamp_secs();
        self.wait_for_transaction_by_hash_bcs(
            transaction.clone().committed_hash(),
            expiration_timestamp,
        )
        .await
    }

    pub async fn wait_for_transaction_by_hash(
        &self,
        hash: HashValue,
        expiration_timestamp_secs: u64,
    ) -> Result<Response<Transaction>> {
        const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
        const DEFAULT_DELAY: Duration = Duration::from_millis(500);

        let start = std::time::Instant::now();
        while start.elapsed() < DEFAULT_TIMEOUT {
            let resp = self.get_transaction_by_hash_inner(hash).await?;
            if resp.status() != StatusCode::NOT_FOUND {
                let txn_resp: Response<Transaction> = self.json(resp).await?;
                let (transaction, state) = txn_resp.into_parts();

                if !transaction.is_pending() {
                    if !transaction.success() {
                        return Err(anyhow!(
                            "transaction execution failed: {}",
                            transaction.vm_status()
                        ));
                    }
                    return Ok(Response::new(transaction, state));
                }
                if expiration_timestamp_secs <= state.timestamp_usecs / 1_000_000 {
                    return Err(anyhow!("transaction expired"));
                }
            }

            tokio::time::sleep(DEFAULT_DELAY).await;
        }

        Err(anyhow!("timeout"))
    }

    pub async fn wait_for_transaction_by_hash_bcs(
        &self,
        hash: HashValue,
        expiration_timestamp_secs: u64,
    ) -> Result<Response<TransactionOnChainData>, (Option<Response<TransactionData>>, anyhow::Error)>
    {
        const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
        const DEFAULT_DELAY: Duration = Duration::from_millis(500);

        let start = std::time::Instant::now();
        while start.elapsed() < DEFAULT_TIMEOUT {
            let resp = self
                .get_transaction_by_hash_bcs_inner(hash)
                .await
                .map_err(|err| (None, err))?;

            // If it's not found, keep waiting for it
            if resp.status() != StatusCode::NOT_FOUND {
                let resp = self
                    .check_and_parse_bcs_response(resp)
                    .await
                    .map_err(|err| (None, err))?;
                let resp = resp
                    .and_then(|bytes| bcs::from_bytes(&bytes))
                    .map_err(|err| (None, err.into()))?;
                let (maybe_pending_txn, state) = resp.into_parts();

                // If we have a committed transaction, determine if it failed or not
                if let TransactionData::OnChain(txn) = maybe_pending_txn {
                    let status = txn.info.status();

                    // The user can handle the error
                    return match status {
                        ExecutionStatus::Success => Ok(Response::new(txn, state)),
                        _ => Err((
                            Some(Response::new(TransactionData::OnChain(txn), state)),
                            anyhow!("Transaction failed"),
                        )),
                    };
                }

                // If it's expired lets give up
                if Duration::from_secs(expiration_timestamp_secs)
                    <= Duration::from_micros(state.timestamp_usecs)
                {
                    return Err((
                        Some(Response::new(maybe_pending_txn, state)),
                        anyhow!("Transaction expired"),
                    ));
                }
            }

            tokio::time::sleep(DEFAULT_DELAY).await;
        }

        return Err((None, anyhow!("Timed out waiting for transaction")));
    }

    pub async fn get_transactions(
        &self,
        start: Option<u64>,
        limit: Option<u16>,
    ) -> Result<Response<Vec<Transaction>>> {
        let url = self.build_path("transactions")?;

        let mut request = self.inner.get(url);
        if let Some(start) = start {
            request = request.query(&[("start", start)])
        }

        if let Some(limit) = limit {
            request = request.query(&[("limit", limit)])
        }

        let response = request.send().await?;

        self.json(response).await
    }

    pub async fn get_transactions_bcs(
        &self,
        start: Option<u64>,
        limit: Option<u16>,
    ) -> Result<Response<Vec<TransactionOnChainData>>> {
        let url = self.build_path("transactions")?;
        let response = self.get_bcs_with_page(url, start, limit).await?;
        Ok(response.and_then(|inner| bcs::from_bytes(&inner))?)
    }

    pub async fn get_transaction_by_hash(&self, hash: HashValue) -> Result<Response<Transaction>> {
        self.json(self.get_transaction_by_hash_inner(hash).await?)
            .await
    }

    pub async fn get_transaction_by_hash_bcs(
        &self,
        hash: HashValue,
    ) -> Result<Response<TransactionData>> {
        let response = self.get_transaction_by_hash_bcs_inner(hash).await?;
        let response = self.check_and_parse_bcs_response(response).await?;
        Ok(response.and_then(|inner| bcs::from_bytes(&inner))?)
    }

    pub async fn get_transaction_by_hash_bcs_inner(
        &self,
        hash: HashValue,
    ) -> Result<reqwest::Response> {
        let url = self.build_path(&format!("transactions/by_hash/{}", hash.to_hex_literal()))?;
        let response = self.inner.get(url).header(ACCEPT, BCS).send().await?;
        Ok(response)
    }

    async fn get_transaction_by_hash_inner(&self, hash: HashValue) -> Result<reqwest::Response> {
        let url = self.build_path(&format!("transactions/by_hash/{}", hash.to_hex_literal()))?;
        Ok(self.inner.get(url).send().await?)
    }

    pub async fn get_transaction_by_version(&self, version: u64) -> Result<Response<Transaction>> {
        self.json(self.get_transaction_by_version_inner(version).await?)
            .await
    }

    pub async fn get_transaction_by_version_bcs(
        &self,
        version: u64,
    ) -> Result<Response<TransactionData>> {
        let url = self.build_path(&format!("transactions/by_version/{}", version))?;
        let response = self.get_bcs(url).await?;
        Ok(response.and_then(|inner| bcs::from_bytes(&inner))?)
    }

    async fn get_transaction_by_version_inner(&self, version: u64) -> Result<reqwest::Response> {
        let url = self.build_path(&format!("transactions/by_version/{}", version))?;
        Ok(self.inner.get(url).send().await?)
    }

    pub async fn get_account_transactions(
        &self,
        address: AccountAddress,
        start: Option<u64>,
        limit: Option<u64>,
    ) -> Result<Response<Vec<Transaction>>> {
        let url = self.build_path(&format!("accounts/{}/transactions", address))?;

        let mut request = self.inner.get(url);
        if let Some(start) = start {
            request = request.query(&[("start", start)])
        }

        if let Some(limit) = limit {
            request = request.query(&[("limit", limit)])
        }

        let response = request.send().await?;

        self.json(response).await
    }

    pub async fn get_account_transactions_bcs(
        &self,
        address: AccountAddress,
        start: Option<u64>,
        limit: Option<u16>,
    ) -> Result<Response<Vec<TransactionOnChainData>>> {
        let url = self.build_path(&format!("accounts/{}/transactions", address))?;
        let response = self.get_bcs_with_page(url, start, limit).await?;
        Ok(response.and_then(|inner| bcs::from_bytes(&inner))?)
    }

    pub async fn get_account_resources(
        &self,
        address: AccountAddress,
    ) -> Result<Response<Vec<Resource>>> {
        let url = self.build_path(&format!("accounts/{}/resources", address))?;

        let response = self.inner.get(url).send().await?;

        self.json(response).await
    }

    pub async fn get_account_resources_bcs(
        &self,
        address: AccountAddress,
    ) -> Result<Response<BTreeMap<StructTag, Vec<u8>>>> {
        let url = self.build_path(&format!("accounts/{}/resources", address))?;
        let response = self.get_bcs(url).await?;
        Ok(response.and_then(|inner| bcs::from_bytes(&inner))?)
    }

    pub async fn get_account_resources_at_version(
        &self,
        address: AccountAddress,
        version: u64,
    ) -> Result<Response<Vec<Resource>>> {
        let url = self.build_path(&format!(
            "accounts/{}/resources?ledger_version={}",
            address, version
        ))?;

        let response = self.inner.get(url).send().await?;

        self.json(response).await
    }

    pub async fn get_resource<T: DeserializeOwned>(
        &self,
        address: AccountAddress,
        resource_type: &str,
    ) -> Result<Response<T>> {
        let resp = self.get_account_resource(address, resource_type).await?;
        resp.and_then(|conf| {
            if let Some(res) = conf {
                serde_json::from_value(res.data)
                    .map_err(|e| anyhow!("deserialize {} failed: {}", resource_type, e))
            } else {
                Err(anyhow!(
                    "could not find resource {} in account {}",
                    resource_type,
                    address
                ))
            }
        })
    }

    pub async fn get_account_resource(
        &self,
        address: AccountAddress,
        resource_type: &str,
    ) -> Result<Response<Option<Resource>>> {
        let url = self.build_path(&format!("accounts/{}/resource/{}", address, resource_type))?;

        let response = self.inner.get(url).send().await?;
        self.json(response).await
    }

    pub async fn get_account_resource_bcs<T: DeserializeOwned>(
        &self,
        address: AccountAddress,
        resource_type: &str,
    ) -> Result<Response<T>> {
        let url = self.build_path(&format!("accounts/{}/resource/{}", address, resource_type))?;
        let response = self.get_bcs(url).await?;
        Ok(response.and_then(|inner| bcs::from_bytes(&inner))?)
    }

    pub async fn get_account_resource_at_version(
        &self,
        address: AccountAddress,
        resource_type: &str,
        version: u64,
    ) -> Result<Response<Option<Resource>>> {
        let url = self.build_path(&format!(
            "accounts/{}/resource/{}?ledger_version={}",
            address, resource_type, version
        ))?;

        let response = self.inner.get(url).send().await?;
        self.json(response).await
    }

    pub async fn get_account_modules(
        &self,
        address: AccountAddress,
    ) -> Result<Response<Vec<MoveModuleBytecode>>> {
        let url = self.build_path(&format!("accounts/{}/modules", address))?;

        let response = self.inner.get(url).send().await?;
        self.json(response).await
    }

    pub async fn get_account_modules_bcs(
        &self,
        address: AccountAddress,
    ) -> Result<Response<BTreeMap<MoveModuleId, Vec<u8>>>> {
        let url = self.build_path(&format!("accounts/{}/modules", address))?;
        let response = self.get_bcs(url).await?;
        Ok(response.and_then(|inner| bcs::from_bytes(&inner))?)
    }

    pub async fn get_account_module(
        &self,
        address: AccountAddress,
        module_name: &str,
    ) -> Result<Response<MoveModuleBytecode>> {
        let url = self.build_path(&format!("accounts/{}/module/{}", address, module_name))?;
        self.get(url).await
    }

    pub async fn get_account_module_bcs(
        &self,
        address: AccountAddress,
        module_name: &str,
    ) -> Result<Response<bytes::Bytes>> {
        let url = self.build_path(&format!("accounts/{}/module/{}", address, module_name))?;
        self.get_bcs(url).await
    }

    pub async fn get_account_events(
        &self,
        address: AccountAddress,
        struct_tag: &str,
        field_name: &str,
        start: Option<u64>,
        limit: Option<u16>,
    ) -> Result<Response<Vec<VersionedEvent>>> {
        let url = self.build_path(&format!(
            "accounts/{}/events/{}/{}",
            address.to_hex_literal(),
            struct_tag,
            field_name
        ))?;
        let mut request = self.inner.get(url);
        if let Some(start) = start {
            request = request.query(&[("start", start)])
        }

        if let Some(limit) = limit {
            request = request.query(&[("limit", limit)])
        }

        let response = request.send().await?;
        self.json(response).await
    }

    pub async fn get_account_events_bcs(
        &self,
        address: AccountAddress,
        struct_tag: &str,
        field_name: &str,
        start: Option<u64>,
        limit: Option<u16>,
    ) -> Result<Response<Vec<EventWithVersion>>> {
        let url = self.build_path(&format!(
            "accounts/{}/events/{}/{}",
            address.to_hex_literal(),
            struct_tag,
            field_name
        ))?;

        let response = self.get_bcs_with_page(url, start, limit).await?;
        Ok(response.and_then(|inner| bcs::from_bytes(&inner))?)
    }

    pub async fn get_new_block_events(
        &self,
        start: Option<u64>,
        limit: Option<u16>,
    ) -> Result<Response<Vec<VersionedNewBlockEvent>>> {
        #[derive(Clone, Debug, Serialize, Deserialize)]
        pub struct NewBlockEventResponse {
            hash: String,
            #[serde(deserialize_with = "deserialize_from_string")]
            epoch: u64,
            #[serde(deserialize_with = "deserialize_from_string")]
            round: u64,
            #[serde(deserialize_with = "deserialize_from_string")]
            height: u64,
            #[serde(deserialize_with = "deserialize_from_prefixed_hex_string")]
            previous_block_votes_bitvec: HexEncodedBytes,
            proposer: String,
            failed_proposer_indices: Vec<String>,
            #[serde(deserialize_with = "deserialize_from_string")]
            time_microseconds: u64,
        }

        let response = self
            .get_account_events(
                CORE_CODE_ADDRESS,
                "0x1::block::BlockResource",
                "new_block_events",
                start,
                limit,
            )
            .await?;

        response.and_then(|events| {
            let new_events: Result<Vec<_>> = events
                .into_iter()
                .map(|event| {
                    let version = event.version.into();
                    let sequence_number = event.sequence_number.into();
                    serde_json::from_value::<NewBlockEventResponse>(event.data)
                        .map_err(|e| anyhow!(e))
                        .and_then(|e| {
                            Ok(VersionedNewBlockEvent {
                                event: NewBlockEvent::new(
                                    AccountAddress::from_hex_literal(&e.hash)
                                        .map_err(|e| anyhow!(e))?,
                                    e.epoch,
                                    e.round,
                                    e.height,
                                    e.previous_block_votes_bitvec.0,
                                    AccountAddress::from_hex_literal(&e.proposer)
                                        .map_err(|e| anyhow!(e))?,
                                    e.failed_proposer_indices
                                        .iter()
                                        .map(|v| v.parse())
                                        .collect::<Result<Vec<_>, _>>()?,
                                    e.time_microseconds,
                                ),
                                version,
                                sequence_number,
                            })
                        })
                })
                .collect();
            new_events
        })
    }

    pub async fn get_table_item<K: Serialize>(
        &self,
        table_handle: u128,
        key_type: &str,
        value_type: &str,
        key: K,
    ) -> Result<Response<Value>> {
        let url = self.build_path(&format!("tables/{}/item", table_handle))?;
        let data = json!({
            "key_type": key_type,
            "value_type": value_type,
            "key": json!(key),
        });

        let response = self.inner.post(url).json(&data).send().await?;
        self.json(response).await
    }

    pub async fn get_account(&self, address: AccountAddress) -> Result<Response<Account>> {
        let url = self.build_path(&format!("accounts/{}", address))?;
        let response = self.inner.get(url).send().await?;
        self.json(response).await
    }

    pub async fn get_account_bcs(
        &self,
        address: AccountAddress,
    ) -> Result<Response<AccountResource>> {
        let url = self.build_path(&format!("accounts/{}", address))?;
        let response = self.get_bcs(url).await?;
        Ok(response.and_then(|inner| bcs::from_bytes(&inner))?)
    }

    pub async fn set_failpoint(&self, name: String, actions: String) -> Result<String> {
        let mut base = self.build_path("set_failpoint")?;
        let url = base
            .query_pairs_mut()
            .append_pair("name", &name)
            .append_pair("actions", &actions)
            .finish();
        let response = self.inner.get(url.clone()).send().await?;

        if !response.status().is_success() {
            let error_response = AptosError::parse_from_json(Some(response.json().await?));
            return Err(anyhow::anyhow!("Request failed: {:?}", error_response));
        }

        response
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("To text failed: {:?}", e))
    }

    async fn check_response(
        &self,
        response: reqwest::Response,
    ) -> Result<(reqwest::Response, State)> {
        if !response.status().is_success() {
            let error_response = AptosError::parse_from_json(Some(response.json().await?));
            return Err(anyhow::anyhow!("Request failed: {:?}", error_response));
        }
        let state = State::from_headers(response.headers())?;

        Ok((response, state))
    }

    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<Response<T>> {
        let (response, state) = self.check_response(response).await?;
        let json = response.json().await?;
        Ok(Response::new(json, state))
    }

    pub async fn health_check(&self, seconds: u64) -> Result<()> {
        let url = self.build_path("-/healthy")?;
        let response = self
            .inner
            .get(url)
            .query(&[("duration_secs", seconds)])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("health check failed"));
        }

        Ok(())
    }

    async fn get<T: DeserializeOwned>(&self, url: Url) -> Result<Response<T>> {
        self.json(self.inner.get(url).send().await?).await
    }

    async fn get_bcs(&self, url: Url) -> Result<Response<bytes::Bytes>> {
        let response = self.inner.get(url).header(ACCEPT, BCS).send().await?;
        self.check_and_parse_bcs_response(response).await
    }

    async fn get_bcs_with_page(
        &self,
        url: Url,
        start: Option<u64>,
        limit: Option<u16>,
    ) -> Result<Response<bytes::Bytes>> {
        let mut request = self.inner.get(url).header(ACCEPT, BCS);
        if let Some(start) = start {
            request = request.query(&[("start", start)])
        }

        if let Some(limit) = limit {
            request = request.query(&[("limit", limit)])
        }

        let response = request.send().await?;
        self.check_and_parse_bcs_response(response).await
    }

    async fn check_and_parse_bcs_response(
        &self,
        response: reqwest::Response,
    ) -> Result<Response<bytes::Bytes>> {
        let (response, state) = self.check_response(response).await?;
        Ok(Response::new(response.bytes().await?, state))
    }
}

impl From<(ReqwestClient, Url)> for Client {
    fn from((inner, base_url): (ReqwestClient, Url)) -> Self {
        Client {
            inner,
            base_url,
            version_path_base: DEFAULT_VERSION_PATH_BASE.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VersionedNewBlockEvent {
    /// event
    pub event: NewBlockEvent,
    /// version
    pub version: u64,
    /// sequence number
    pub sequence_number: u64,
}
