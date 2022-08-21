// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use crate::common::types::{CliCommand, CliResult};
use clap::Subcommand;

pub mod create;
pub mod create_resource_account;
pub mod fund;
pub mod list;
pub mod transfer;

/// Tool for interacting with accounts
///
/// This tool is used to create accounts, get information about the
/// account's resources, and transfer resources between accounts.
#[derive(Debug, Subcommand)]
pub enum AccountTool {
    Create(create::CreateAccount),
    CreateResourceAccount(create_resource_account::CreateResourceAccount),
    FundWithFaucet(fund::FundWithFaucet),
    List(list::ListAccount),
    Transfer(transfer::TransferCoins),
}

impl AccountTool {
    pub async fn execute(self) -> CliResult {
        match self {
            AccountTool::Create(tool) => tool.execute_serialized().await,
            AccountTool::CreateResourceAccount(tool) => tool.execute_serialized().await,
            AccountTool::FundWithFaucet(tool) => tool.execute_serialized().await,
            AccountTool::List(tool) => tool.execute_serialized().await,
            AccountTool::Transfer(tool) => tool.execute_serialized().await,
        }
    }
}
