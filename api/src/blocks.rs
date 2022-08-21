// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use crate::accept_type::AcceptType;
use crate::context::Context;
use crate::failpoint::fail_point_poem;
use crate::response::{BasicResponse, BasicResponseStatus, BasicResultWith404};
use crate::ApiTags;
use aptos_api_types::{BcsBlock, Block, LedgerInfo};
use poem_openapi::param::{Path, Query};
use poem_openapi::OpenApi;
use std::sync::Arc;

pub struct BlocksApi {
    pub context: Arc<Context>,
}

#[OpenApi]
impl BlocksApi {
    /// Get blocks by height
    ///
    /// This endpoint allows you to get the transactions in a block
    /// and the corresponding block information.
    #[oai(
        path = "/blocks/by_height/:block_height",
        method = "get",
        operation_id = "get_block_by_height",
        tag = "ApiTags::Blocks"
    )]
    async fn get_block_by_height(
        &self,
        accept_type: AcceptType,
        block_height: Path<u64>,
        with_transactions: Query<Option<bool>>,
    ) -> BasicResultWith404<Block> {
        fail_point_poem("endpoint_get_block_by_height")?;
        self.get_by_height(
            accept_type,
            block_height.0,
            with_transactions.0.unwrap_or_default(),
        )
    }

    /// Get blocks by version
    ///
    /// This endpoint allows you to get the transactions in a block
    /// and the corresponding block information given a version in the block.
    #[oai(
        path = "/blocks/by_version/:version",
        method = "get",
        operation_id = "get_block_by_version",
        tag = "ApiTags::Blocks"
    )]
    async fn get_block_by_version(
        &self,
        accept_type: AcceptType,
        version: Path<u64>,
        with_transactions: Query<Option<bool>>,
    ) -> BasicResultWith404<Block> {
        fail_point_poem("endpoint_get_block_by_version")?;
        self.get_by_version(
            accept_type,
            version.0,
            with_transactions.0.unwrap_or_default(),
        )
    }
}

impl BlocksApi {
    fn get_by_height(
        &self,
        accept_type: AcceptType,
        block_height: u64,
        with_transactions: bool,
    ) -> BasicResultWith404<Block> {
        let latest_ledger_info = self.context.get_latest_ledger_info()?;
        let bcs_block = self.context.get_block_by_height(
            block_height,
            &latest_ledger_info,
            with_transactions,
        )?;

        self.render_bcs_block(&accept_type, latest_ledger_info, bcs_block)
    }

    fn get_by_version(
        &self,
        accept_type: AcceptType,
        version: u64,
        with_transactions: bool,
    ) -> BasicResultWith404<Block> {
        let latest_ledger_info = self.context.get_latest_ledger_info()?;
        let bcs_block =
            self.context
                .get_block_by_version(version, &latest_ledger_info, with_transactions)?;

        self.render_bcs_block(&accept_type, latest_ledger_info, bcs_block)
    }

    fn render_bcs_block(
        &self,
        accept_type: &AcceptType,
        latest_ledger_info: LedgerInfo,
        bcs_block: BcsBlock,
    ) -> BasicResultWith404<Block> {
        match accept_type {
            AcceptType::Json => {
                let transactions = if let Some(inner) = bcs_block.transactions {
                    Some(self.context.render_transactions_sequential(
                        &latest_ledger_info,
                        inner,
                        bcs_block.block_timestamp,
                    )?)
                } else {
                    None
                };
                let block = Block {
                    block_height: bcs_block.block_height.into(),
                    block_hash: bcs_block.block_hash.into(),
                    block_timestamp: bcs_block.block_timestamp.into(),
                    first_version: bcs_block.first_version.into(),
                    last_version: bcs_block.last_version.into(),
                    transactions,
                };
                BasicResponse::try_from_json((block, &latest_ledger_info, BasicResponseStatus::Ok))
            }
            AcceptType::Bcs => BasicResponse::try_from_bcs((
                bcs_block,
                &latest_ledger_info,
                BasicResponseStatus::Ok,
            )),
        }
    }
}
