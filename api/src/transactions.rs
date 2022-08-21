// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use crate::accept_type::AcceptType;
use crate::bcs_payload::Bcs;
use crate::context::Context;
use crate::failpoint::fail_point_poem;
use crate::page::Page;
use crate::response::{
    transaction_not_found_by_hash, transaction_not_found_by_version, BadRequestError, BasicError,
    BasicErrorWith404, BasicResponse, BasicResponseStatus, BasicResult, BasicResultWith404,
    InsufficientStorageError, InternalError,
};
use crate::ApiTags;
use crate::{generate_error_response, generate_success_response};
use anyhow::Context as AnyhowContext;
use aptos_api_types::{
    Address, AptosErrorCode, AsConverter, EncodeSubmissionRequest, HashValue, HexEncodedBytes,
    LedgerInfo, PendingTransaction, SubmitTransactionRequest, Transaction, TransactionData,
    TransactionOnChainData, UserTransaction, U64,
};
use aptos_crypto::signing_message;
use aptos_types::mempool_status::MempoolStatusCode;
use aptos_types::transaction::{
    ExecutionStatus, RawTransaction, RawTransactionWithData, SignedTransaction, TransactionStatus,
};
use aptos_types::vm_status::StatusCode;
use aptos_vm::AptosVM;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::{ApiRequest, OpenApi};

generate_success_response!(SubmitTransactionResponse, (202, Accepted));
generate_error_response!(
    SubmitTransactionError,
    (400, BadRequest),
    (413, PayloadTooLarge),
    (500, Internal),
    (507, InsufficientStorage)
);

type SubmitTransactionResult<T> =
    poem::Result<SubmitTransactionResponse<T>, SubmitTransactionError>;

type SimulateTransactionResult<T> = poem::Result<BasicResponse<T>, SubmitTransactionError>;

// TODO: Consider making both content types accept either
// SubmitTransactionRequest or SignedTransaction, the way
// it is now is quite confusing.

// We need a custom type here because we use different types for each of the
// content types possible for the POST data.
#[derive(ApiRequest, Debug)]
pub enum SubmitTransactionPost {
    #[oai(content_type = "application/json")]
    Json(Json<SubmitTransactionRequest>),

    // TODO: Since I don't want to impl all the Poem derives on SignedTransaction,
    // find a way to at least indicate in the spec that it expects a SignedTransaction.
    // TODO: https://github.com/aptos-labs/aptos-core/issues/2275
    #[oai(content_type = "application/x.aptos.signed_transaction+bcs")]
    Bcs(Bcs),
}

pub struct TransactionsApi {
    pub context: Arc<Context>,
}

#[OpenApi]
impl TransactionsApi {
    /// Get transactions
    ///
    /// Get on-chain (meaning, committed) transactions. You may specify from
    /// when you want the transactions and how to include in the response.
    #[oai(
        path = "/transactions",
        method = "get",
        operation_id = "get_transactions",
        tag = "ApiTags::Transactions"
    )]
    async fn get_transactions(
        &self,
        accept_type: AcceptType,
        start: Query<Option<U64>>,
        limit: Query<Option<u16>>,
    ) -> BasicResultWith404<Vec<Transaction>> {
        fail_point_poem("endpoint_get_transactions")?;
        let page = Page::new(start.0.map(|v| v.0), limit.0);
        self.list(&accept_type, page)
    }

    /// Get transaction by hash
    ///
    /// Look up a transaction by its hash. This is the same hash that is returned
    /// by the API when submitting a transaction (see PendingTransaction).
    ///
    /// When given a transaction hash, the server first looks for the transaction
    /// in storage (on-chain, committed). If no on-chain transaction is found, it
    /// looks the transaction up by hash in the mempool (pending, not yet committed).
    ///
    /// To create a transaction hash by yourself, do the following:
    ///   1. Hash message bytes: "RawTransaction" bytes + BCS bytes of [Transaction](https://aptos-labs.github.io/aptos-core/aptos_types/transaction/enum.Transaction.html).
    ///   2. Apply hash algorithm `SHA3-256` to the hash message bytes.
    ///   3. Hex-encode the hash bytes with `0x` prefix.
    // TODO: Include a link to an example of how to do this ^
    #[oai(
        path = "/transactions/by_hash/:txn_hash",
        method = "get",
        operation_id = "get_transaction_by_hash",
        tag = "ApiTags::Transactions"
    )]
    async fn get_transaction_by_hash(
        &self,
        accept_type: AcceptType,
        txn_hash: Path<HashValue>,
        // TODO: Use a new request type that can't return 507.
    ) -> BasicResultWith404<Transaction> {
        fail_point_poem("endpoint_transaction_by_hash")?;
        self.get_transaction_by_hash_inner(&accept_type, txn_hash.0)
            .await
    }

    /// Get transaction by version
    ///
    /// todo
    #[oai(
        path = "/transactions/by_version/:txn_version",
        method = "get",
        operation_id = "get_transaction_by_version",
        tag = "ApiTags::Transactions"
    )]
    async fn get_transaction_by_version(
        &self,
        accept_type: AcceptType,
        txn_version: Path<U64>,
    ) -> BasicResultWith404<Transaction> {
        fail_point_poem("endpoint_transaction_by_version")?;
        self.get_transaction_by_version_inner(&accept_type, txn_version.0)
            .await
    }

    /// Get account transactions
    ///
    /// todo
    #[oai(
        path = "/accounts/:address/transactions",
        method = "get",
        operation_id = "get_account_transactions",
        tag = "ApiTags::Transactions"
    )]
    // TODO: https://github.com/aptos-labs/aptos-core/issues/2285
    async fn get_accounts_transactions(
        &self,
        accept_type: AcceptType,
        address: Path<Address>,
        start: Query<Option<U64>>,
        limit: Query<Option<u16>>,
    ) -> BasicResultWith404<Vec<Transaction>> {
        fail_point_poem("endpoint_get_accounts_transactions")?;
        let page = Page::new(start.0.map(|v| v.0), limit.0);
        self.list_by_account(&accept_type, page, address.0)
    }

    /// Submit transaction
    ///
    /// This endpoint accepts transaction submissions in two formats.
    ///
    /// To submit a transaction as JSON, you must submit a SubmitTransactionRequest.
    /// To build this request, do the following:
    ///
    ///   1. Encode the transaction as BCS. If you are using a language that has
    ///      native BCS support, make sure of that library. If not, you may take
    ///      advantage of /transactions/encode_submission. When using this
    ///      endpoint, make sure you trust the node you're talking to, as it is
    ///      possible they could manipulate your request.
    ///   2. Sign the encoded transaction and use it to create a TransactionSignature.
    ///   3. Submit the request. Make sure to use the "application/json" Content-Type.
    ///
    /// To submit a transaction as BCS, you must submit a SignedTransaction
    /// encoded as BCS. See SignedTransaction in types/src/transaction/mod.rs.
    // TODO: Point to examples of both of these flows, in multiple languages.
    #[oai(
        path = "/transactions",
        method = "post",
        operation_id = "submit_transaction",
        tag = "ApiTags::Transactions"
    )]
    async fn submit_transaction(
        &self,
        accept_type: AcceptType,
        data: SubmitTransactionPost,
    ) -> SubmitTransactionResult<PendingTransaction> {
        fail_point_poem("endpoint_submit_transaction")?;
        let ledger_info = self.context.get_latest_ledger_info()?;
        let signed_transaction = self.get_signed_transaction(&ledger_info, data)?;
        self.create(&accept_type, ledger_info, signed_transaction)
            .await
    }

    /// Simulate transaction
    ///
    /// Simulate submitting a transaction. To use this, you must:
    /// - Create a SignedTransaction with a zero-padded signature.
    /// - Submit a SubmitTransactionRequest containing a UserTransactionRequest containing that signature.
    ///
    /// To use this endpoint with BCS, you must submit a SignedTransaction
    /// encoded as BCS. See SignedTransaction in types/src/transaction/mod.rs.
    #[oai(
        path = "/transactions/simulate",
        method = "post",
        operation_id = "simulate_transaction",
        tag = "ApiTags::Transactions"
    )]
    async fn simulate_transaction(
        &self,
        accept_type: AcceptType,
        data: SubmitTransactionPost,
    ) -> SimulateTransactionResult<Vec<UserTransaction>> {
        fail_point_poem("endpoint_simulate_transaction")?;
        let ledger_info = self.context.get_latest_ledger_info()?;
        let signed_transaction = self.get_signed_transaction(&ledger_info, data)?;
        self.simulate(&accept_type, ledger_info, signed_transaction)
            .await
    }

    /// Encode submission
    ///
    /// This endpoint accepts an EncodeSubmissionRequest, which internally is a
    /// UserTransactionRequestInner (and optionally secondary signers) encoded
    /// as JSON, validates the request format, and then returns that request
    /// encoded in BCS. The client can then use this to create a transaction
    /// signature to be used in a SubmitTransactionRequest, which it then
    /// passes to the /transactions POST endpoint.
    ///
    /// To be clear, this endpoint makes it possible to submit transaction
    /// requests to the API from languages that do not have library support for
    /// BCS. If you are using an SDK that has BCS support, such as the official
    /// Rust, TypeScript, or Python SDKs, you do not need to use this endpoint.
    ///
    /// To sign a message using the response from this endpoint:
    /// - Decode the hex encoded string in the response to bytes.
    /// - Sign the bytes to create the signature.
    /// - Use that as the signature field in something like Ed25519Signature, which you then use to build a TransactionSignature.
    //
    // TODO: Link an example of how to do this. Use externalDoc.
    #[oai(
        path = "/transactions/encode_submission",
        method = "post",
        operation_id = "encode_submission",
        tag = "ApiTags::Transactions"
    )]
    async fn encode_submission(
        &self,
        accept_type: AcceptType,
        data: Json<EncodeSubmissionRequest>,
        // TODO: Use a new request type that can't return 507 but still returns all the other necessary errors.
    ) -> BasicResult<HexEncodedBytes> {
        fail_point_poem("endpoint_encode_submission")?;
        self.get_signing_message(&accept_type, data.0)
    }
}

impl TransactionsApi {
    fn list(&self, accept_type: &AcceptType, page: Page) -> BasicResultWith404<Vec<Transaction>> {
        let latest_ledger_info = self.context.get_latest_ledger_info()?;
        let ledger_version = latest_ledger_info.version();

        let limit = page.limit(&latest_ledger_info)?;
        // TODO: https://github.com/aptos-labs/aptos-core/issues/2286
        let start_version = page.compute_start(limit, ledger_version, &latest_ledger_info)?;
        let data = self
            .context
            .get_transactions(start_version, limit, ledger_version)
            .context("Failed to read raw transactions from storage")
            .map_err(|err| {
                BasicErrorWith404::internal_with_code(
                    err,
                    AptosErrorCode::InvalidBcsInStorageError,
                    &latest_ledger_info,
                )
            })?;

        match accept_type {
            AcceptType::Json => {
                let timestamp = self
                    .context
                    .get_block_timestamp(&latest_ledger_info, start_version)?;
                BasicResponse::try_from_json((
                    self.context.render_transactions_sequential(
                        &latest_ledger_info,
                        data,
                        timestamp,
                    )?,
                    &latest_ledger_info,
                    BasicResponseStatus::Ok,
                ))
            }
            AcceptType::Bcs => {
                BasicResponse::try_from_bcs((data, &latest_ledger_info, BasicResponseStatus::Ok))
            }
        }
    }

    async fn get_transaction_by_hash_inner(
        &self,
        accept_type: &AcceptType,
        hash: HashValue,
    ) -> BasicResultWith404<Transaction> {
        let ledger_info = self.context.get_latest_ledger_info()?;
        let txn_data = self
            .get_by_hash(hash.into(), &ledger_info)
            .await
            .context(format!("Failed to get transaction by hash {}", hash))
            .map_err(|err| {
                BasicErrorWith404::internal_with_code(
                    err,
                    AptosErrorCode::ReadFromStorageError,
                    &ledger_info,
                )
            })?
            .context(format!("Failed to find transaction with hash: {}", hash))
            .map_err(|_| transaction_not_found_by_hash(hash, &ledger_info))?;

        self.get_transaction_inner(accept_type, txn_data, &ledger_info)
            .await
    }

    async fn get_transaction_by_version_inner(
        &self,
        accept_type: &AcceptType,
        version: U64,
    ) -> BasicResultWith404<Transaction> {
        let ledger_info = self.context.get_latest_ledger_info()?;
        let txn_data = self
            .get_by_version(version.0, &ledger_info)
            .context(format!("Failed to get transaction by version {}", version))
            .map_err(|err| {
                BasicErrorWith404::internal_with_code(
                    err,
                    AptosErrorCode::ReadFromStorageError,
                    &ledger_info,
                )
            })?
            .context(format!(
                "Failed to find transaction at version: {}",
                version
            ))
            .map_err(|_| transaction_not_found_by_version(version.0, &ledger_info))?;

        self.get_transaction_inner(accept_type, txn_data, &ledger_info)
            .await
    }

    async fn get_transaction_inner(
        &self,
        accept_type: &AcceptType,
        transaction_data: TransactionData,
        ledger_info: &LedgerInfo,
    ) -> BasicResultWith404<Transaction> {
        match accept_type {
            AcceptType::Json => {
                let resolver = self.context.move_resolver_poem(ledger_info)?;
                let transaction = match transaction_data {
                    TransactionData::OnChain(txn) => {
                        let timestamp =
                            self.context.get_block_timestamp(ledger_info, txn.version)?;
                        resolver
                            .as_converter(self.context.db.clone())
                            .try_into_onchain_transaction(timestamp, txn)
                            .context("Failed to convert on chain transaction to Transaction")
                            .map_err(|err| {
                                BasicErrorWith404::internal_with_code(
                                    err,
                                    AptosErrorCode::InvalidBcsInStorageError,
                                    ledger_info,
                                )
                            })?
                    }
                    TransactionData::Pending(txn) => resolver
                        .as_converter(self.context.db.clone())
                        .try_into_pending_transaction(*txn)
                        .context("Failed to convert on pending transaction to Transaction")
                        .map_err(|err| {
                            BasicErrorWith404::internal_with_code(
                                err,
                                AptosErrorCode::InvalidBcsInStorageError,
                                ledger_info,
                            )
                        })?,
                };

                BasicResponse::try_from_json((transaction, ledger_info, BasicResponseStatus::Ok))
            }
            AcceptType::Bcs => BasicResponse::try_from_bcs((
                transaction_data,
                ledger_info,
                BasicResponseStatus::Ok,
            )),
        }
    }

    fn get_by_version(
        &self,
        version: u64,
        ledger_info: &LedgerInfo,
    ) -> anyhow::Result<Option<TransactionData>> {
        if version > ledger_info.version() {
            return Ok(None);
        }
        Ok(Some(
            self.context
                .get_transaction_by_version(version, ledger_info.version())?
                .into(),
        ))
    }

    // This function looks for the transaction by hash in database and then mempool,
    // because the period a transaction stay in the mempool is likely short.
    // Although the mempool get transation is async, but looking up txn in database is a sync call,
    // thus we keep it simple and call them in sequence.
    async fn get_by_hash(
        &self,
        hash: aptos_crypto::HashValue,
        ledger_info: &LedgerInfo,
    ) -> anyhow::Result<Option<TransactionData>> {
        let from_db = self
            .context
            .get_transaction_by_hash(hash, ledger_info.version())?;
        Ok(match from_db {
            None => self
                .context
                .get_pending_transaction_by_hash(hash)
                .await?
                .map(|t| t.into()),
            _ => from_db.map(|t| t.into()),
        })
    }

    fn list_by_account(
        &self,
        accept_type: &AcceptType,
        page: Page,
        address: Address,
    ) -> BasicResultWith404<Vec<Transaction>> {
        let latest_ledger_info = self.context.get_latest_ledger_info()?;
        // TODO: Return more specific errors from within this function.
        let data = self.context.get_account_transactions(
            address.into(),
            page.start(0, u64::MAX, &latest_ledger_info)?,
            page.limit(&latest_ledger_info)?,
            latest_ledger_info.version(),
            &latest_ledger_info,
        )?;
        match accept_type {
            AcceptType::Json => BasicResponse::try_from_json((
                self.context
                    .render_transactions_non_sequential(&latest_ledger_info, data)?,
                &latest_ledger_info,
                BasicResponseStatus::Ok,
            )),
            AcceptType::Bcs => {
                BasicResponse::try_from_bcs((data, &latest_ledger_info, BasicResponseStatus::Ok))
            }
        }
    }

    fn get_signed_transaction(
        &self,
        ledger_info: &LedgerInfo,
        data: SubmitTransactionPost,
    ) -> Result<SignedTransaction, SubmitTransactionError> {
        match data {
            SubmitTransactionPost::Bcs(data) => {
                let signed_transaction = bcs::from_bytes(&data.0)
                    .context("Failed to deserialize input into SignedTransaction")
                    .map_err(|err| {
                        SubmitTransactionError::bad_request_with_code(
                            err,
                            AptosErrorCode::InvalidInput,
                            ledger_info,
                        )
                    })?;
                Ok(signed_transaction)
            }
            SubmitTransactionPost::Json(data) => self
                .context
                .move_resolver_poem(ledger_info)?
                .as_converter(self.context.db.clone())
                .try_into_signed_transaction_poem(data.0, self.context.chain_id())
                .context("Failed to create SignedTransaction from SubmitTransactionRequest")
                .map_err(|err| {
                    SubmitTransactionError::bad_request_with_code(
                        err,
                        AptosErrorCode::InvalidInput,
                        ledger_info,
                    )
                }),
        }
    }

    async fn create(
        &self,
        accept_type: &AcceptType,
        ledger_info: LedgerInfo,
        txn: SignedTransaction,
    ) -> SubmitTransactionResult<PendingTransaction> {
        let (mempool_status, vm_status_opt) = self
            .context
            .submit_transaction(txn.clone())
            .await
            .context("Mempool failed to initially evaluate submitted transaction")
            .map_err(|err| {
                SubmitTransactionError::internal_with_code(
                    err,
                    AptosErrorCode::TransactionSubmissionFailed,
                    &ledger_info,
                )
            })?;
        match mempool_status.code {
            MempoolStatusCode::Accepted => match accept_type {
                AcceptType::Json => {
                    let resolver = self.context.move_resolver_poem(&ledger_info)?;
                    let pending_txn = resolver
                            .as_converter(self.context.db.clone())
                            .try_into_pending_transaction_poem(txn)
                            .context("Failed to build PendingTransaction from mempool response, even though it said the request was accepted")
                            .map_err(|err| SubmitTransactionError::internal_with_code(
                                err,
                                AptosErrorCode::InternalError,
                                &ledger_info,
                            ))?;
                    SubmitTransactionResponse::try_from_json((
                        pending_txn,
                        &ledger_info,
                        SubmitTransactionResponseStatus::Accepted,
                    ))
                }
                AcceptType::Bcs => SubmitTransactionResponse::try_from_bcs((
                    (),
                    &ledger_info,
                    SubmitTransactionResponseStatus::Accepted,
                )),
            },
            MempoolStatusCode::MempoolIsFull => {
                Err(SubmitTransactionError::insufficient_storage_with_code(
                    &mempool_status.message,
                    AptosErrorCode::MempoolIsFull,
                    &ledger_info,
                ))
            }
            MempoolStatusCode::VmError => {
                if let Some(status) = vm_status_opt {
                    Err(SubmitTransactionError::bad_request_with_vm_status(
                        format!(
                            "Invalid transaction: Type: {:?} Code: {:?}",
                            status.status_type(),
                            status
                        ),
                        AptosErrorCode::InvalidSubmittedTransaction,
                        status,
                        &ledger_info,
                    ))
                } else {
                    Err(SubmitTransactionError::bad_request_with_vm_status(
                        "Invalid transaction: unknown",
                        AptosErrorCode::InvalidSubmittedTransaction,
                        StatusCode::UNKNOWN_STATUS,
                        &ledger_info,
                    ))
                }
            }
            MempoolStatusCode::InvalidSeqNumber => {
                Err(SubmitTransactionError::bad_request_with_code(
                    mempool_status.message,
                    AptosErrorCode::SequenceNumberTooOld,
                    &ledger_info,
                ))
            }
            MempoolStatusCode::InvalidUpdate => Err(SubmitTransactionError::bad_request_with_code(
                mempool_status.message,
                AptosErrorCode::InvalidTransactionUpdate,
                &ledger_info,
            )),
            MempoolStatusCode::TooManyTransactions => {
                Err(SubmitTransactionError::insufficient_storage_with_code(
                    &mempool_status.message,
                    AptosErrorCode::MempoolIsFullForAccount,
                    &ledger_info,
                ))
            }
            MempoolStatusCode::UnknownStatus => Err(SubmitTransactionError::internal_with_code(
                format!("Transaction was rejected with status {}", mempool_status,),
                AptosErrorCode::InternalError,
                &ledger_info,
            )),
        }
    }

    // TODO: This returns a Vec<Transaction>, but is it possible for a single
    // transaction request to result in multiple executed transactions?
    // TODO: This function leverages a lot of types from aptos_types, use the
    // local API types and just return those directly, instead of converting
    // from these types in render_transactions.
    pub async fn simulate(
        &self,
        accept_type: &AcceptType,
        ledger_info: LedgerInfo,
        txn: SignedTransaction,
    ) -> SimulateTransactionResult<Vec<UserTransaction>> {
        if txn.signature_is_valid() {
            return Err(SubmitTransactionError::bad_request_with_code(
                "Simulated transactions must have a non-valid signature",
                AptosErrorCode::InvalidInput,
                &ledger_info,
            ));
        }
        let move_resolver = self.context.move_resolver_poem(&ledger_info)?;
        let (status, output_ext) = AptosVM::simulate_signed_transaction(&txn, &move_resolver);
        let version = ledger_info.version();

        // Apply deltas.
        // TODO: while `into_transaction_output_with_status()` should never fail
        // to apply deltas, we should propagate errors properly. Fix this when
        // VM error handling is fixed.
        let output = output_ext.into_transaction_output(&move_resolver);
        debug_assert!(
            matches!(output, Ok(_)),
            "converting into transaction output failed"
        );
        let output = output.unwrap();

        let exe_status = match status.into() {
            TransactionStatus::Keep(exec_status) => exec_status,
            _ => ExecutionStatus::MiscellaneousError(None),
        };

        let zero_hash = aptos_crypto::HashValue::zero();
        let info = aptos_types::transaction::TransactionInfo::new(
            zero_hash,
            zero_hash,
            zero_hash,
            None,
            output.gas_used(),
            exe_status,
        );
        let simulated_txn = TransactionOnChainData {
            version,
            transaction: aptos_types::transaction::Transaction::UserTransaction(txn),
            info,
            events: output.events().to_vec(),
            accumulator_root_hash: aptos_crypto::HashValue::default(),
            changes: output.write_set().clone(),
        };

        match accept_type {
            AcceptType::Json => {
                let transactions = self
                    .context
                    .render_transactions_non_sequential(&ledger_info, vec![simulated_txn])?;

                // Users can only make requests to simulate UserTransactions, so unpack
                // the Vec<Transaction> into Vec<UserTransaction>.
                let mut user_transactions = Vec::new();
                for transaction in transactions.into_iter() {
                    match transaction {
                        Transaction::UserTransaction(user_txn) => user_transactions.push(*user_txn),
                        _ => {
                            return Err(SubmitTransactionError::internal_with_code(
                                "Simulation transaction resulted in a non-UserTransaction",
                                AptosErrorCode::InternalError,
                                &ledger_info,
                            ))
                        }
                    }
                }
                BasicResponse::try_from_json((
                    user_transactions,
                    &ledger_info,
                    BasicResponseStatus::Ok,
                ))
            }
            AcceptType::Bcs => {
                BasicResponse::try_from_bcs((simulated_txn, &ledger_info, BasicResponseStatus::Ok))
            }
        }
    }

    pub fn get_signing_message(
        &self,
        accept_type: &AcceptType,
        request: EncodeSubmissionRequest,
    ) -> BasicResult<HexEncodedBytes> {
        // We don't want to encourage people to use this API if they can sign the request directly
        if accept_type == &AcceptType::Bcs {
            return Err(BasicError::bad_request_with_code_no_info(
                "BCS is not supported for encode submission",
                AptosErrorCode::BcsNotSupported,
            ));
        }

        let ledger_info = self.context.get_latest_ledger_info()?;
        let resolver = self.context.move_resolver_poem(&ledger_info)?;
        let raw_txn: RawTransaction = resolver
            .as_converter(self.context.db.clone())
            .try_into_raw_transaction_poem(request.transaction, self.context.chain_id())
            .context("The given transaction is invalid")
            .map_err(|err| {
                BasicError::bad_request_with_code(err, AptosErrorCode::InvalidInput, &ledger_info)
            })?;

        let raw_message = match request.secondary_signers {
            Some(secondary_signer_addresses) => {
                signing_message(&RawTransactionWithData::new_multi_agent(
                    raw_txn,
                    secondary_signer_addresses
                        .into_iter()
                        .map(|v| v.into())
                        .collect(),
                ))
            }
            None => raw_txn.signing_message(),
        };

        BasicResponse::try_from_json((
            HexEncodedBytes::from(raw_message),
            &ledger_info,
            BasicResponseStatus::Ok,
        ))
    }
}
