// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use aptos_types::vm_status::StatusCode;
use poem_openapi::{Enum, Object};
use serde::Deserialize;

/// This is the generic struct we use for all API errors, it contains a string
/// message and an Aptos API specific error code.
#[derive(Debug, Deserialize, Object)]
pub struct AptosError {
    /// A message describing the error
    pub message: String,
    /// A code providing more granular error information beyond the HTTP status code
    pub error_code: AptosErrorCode,
    /// A code providing VM error details when submitting transactions to the VM
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vm_error_code: Option<u64>,
}

impl AptosError {
    pub fn new_with_error_code<ErrorType: std::fmt::Display>(
        error: ErrorType,
        error_code: AptosErrorCode,
    ) -> AptosError {
        Self {
            message: error.to_string(),
            error_code,
            vm_error_code: None,
        }
    }

    pub fn new_with_vm_status<ErrorType: std::fmt::Display>(
        error: ErrorType,
        error_code: AptosErrorCode,
        vm_error_code: StatusCode,
    ) -> AptosError {
        Self {
            message: error.to_string(),
            error_code,
            vm_error_code: Some(vm_error_code as u64),
        }
    }
}

/// These codes provide more granular error information beyond just the HTTP
/// status code of the response.
#[derive(Debug, Deserialize, Enum)]
#[oai(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum AptosErrorCode {
    /// Account not found at the requested version
    AccountNotFound = 101,
    /// Resource not found at the requested version
    ResourceNotFound = 102,
    /// Module not found at the requested version
    ModuleNotFound = 103,
    /// Struct field not found at the requested version
    StructFieldNotFound = 104,
    /// Ledger version not found at the requested version
    ///
    /// Usually means that the version is ahead of the latest version
    VersionNotFound = 105,
    /// Transaction not found at the requested version or with the requested hash
    TransactionNotFound = 106,
    /// Table item not found at the requested version
    TableItemNotFound = 107,
    /// Block not found at the requested version or height
    ///
    /// Usually means the block is fully or partially pruned or the height / version is ahead
    /// of the latest version
    BlockNotFound = 108,

    /// Ledger version is pruned
    VersionPruned = 200,
    /// Block is fully or partially pruned
    BlockPruned = 201,

    /// The API's inputs were invalid
    InvalidInput = 300,
    /// The start param given for paging is invalid.
    InvalidStartParam = 301,
    /// The limit param given for paging is invalid.
    InvalidLimitParam = 302,
    /// The event key for events is invalid.
    InvalidEventKey = 303,

    /// The transaction submitted was invalid.
    InvalidSubmittedTransaction = 400,
    /// The transaction was an invalid update to an already submitted transaction.
    InvalidTransactionUpdate = 401,
    /// The sequence number for the transaction is behind the latest sequence number.
    SequenceNumberTooOld = 402,
    /// The submitted transaction failed VM checks.
    VmError = 403,
    /// The transaction failed to submit.
    TransactionSubmissionFailed = 404,

    /// Health check failed.
    HealthCheckFailed = 500,
    /// The mempool is full, no new transactions can be submitted.
    MempoolIsFull = 501,
    /// The mempool is full for this account's quota, no new transactions can be submitted.
    MempoolIsFullForAccount = 502,

    /// Internal server error
    InternalError = 600,
    /// The API failed to read from storage for this request, not because of a
    /// bad request, but because of some internal error.
    ReadFromStorageError = 601,
    /// The data we read from the DB was not valid BCS.
    InvalidBcsInStorageError = 602,
    /// We were unexpectedly unable to convert a Rust type to BCS.
    BcsSerializationError = 603,
    /// Error from the web framework
    WebFrameworkError = 604,

    /// BCS format is not supported on this API.
    BcsNotSupported = 701,
}
