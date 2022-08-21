// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use crate::common::{to_hex_lower, Y2K_MS};
use crate::{
    common::{
        check_network, get_block_index_from_request, get_timestamp, handle_request, with_context,
    },
    error::{ApiError, ApiResult},
    types::{Block, BlockIdentifier, BlockRequest, BlockResponse, Transaction},
    RosettaContext,
};
use aptos_logger::{debug, trace};
use aptos_rest_client::aptos_api_types::HashValue;
use std::sync::Arc;
use std::{collections::BTreeMap, sync::RwLock};
use warp::Filter;

pub fn block_route(
    server_context: RosettaContext,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("block")
        .and(warp::post())
        .and(warp::body::json())
        .and(with_context(server_context))
        .and_then(handle_request(block))
}

/// Retrieves a block (in this case a single transaction) given it's identifier.
///
/// Our implementation allows for by `index`, which is the ledger `version` or by
/// transaction `hash`.
///
/// [API Spec](https://www.rosetta-api.org/docs/BlockApi.html#block)
async fn block(request: BlockRequest, server_context: RosettaContext) -> ApiResult<BlockResponse> {
    debug!("/block");
    trace!(
        request = ?request,
        server_context = ?server_context,
        "/block",
    );

    check_network(request.network_identifier, &server_context)?;

    // Retrieve by block or by hash, both or neither is not allowed
    let block_index =
        get_block_index_from_request(&server_context, request.block_identifier).await?;

    let (parent_transaction, block) =
        get_block_by_index(server_context.block_cache()?.as_ref(), block_index).await?;

    let block = build_block(parent_transaction, block).await?;

    Ok(BlockResponse {
        block: Some(block),
        other_transactions: None,
    })
}

/// Build up the transaction, which should contain the `operations` as the change set
async fn build_block(
    parent_block_identifier: BlockIdentifier,
    block: aptos_rest_client::aptos_api_types::Block,
) -> ApiResult<Block> {
    // note: timestamps are in microseconds, so we convert to milliseconds
    let timestamp = get_timestamp(block.block_timestamp.0);
    let block_identifier = BlockIdentifier::from_block(&block);

    // Convert the transactions and build the block
    let mut transactions: Vec<Transaction> = Vec::new();
    if let Some(txns) = block.transactions {
        for txn in txns {
            transactions.push(Transaction::from_transaction(txn).await?)
        }
    }

    Ok(Block {
        block_identifier,
        parent_block_identifier,
        timestamp,
        transactions,
    })
}

/// Retrieves a block by its index
async fn get_block_by_index(
    block_cache: &BlockCache,
    block_height: u64,
) -> ApiResult<(BlockIdentifier, aptos_rest_client::aptos_api_types::Block)> {
    let block = block_cache.get_block_by_height(block_height, true).await?;

    // For the genesis block, we populate parent_block_identifier with the
    // same genesis block. Refer to
    // https://www.rosetta-api.org/docs/common_mistakes.html#malformed-genesis-block
    if block_height == 0 {
        Ok((BlockIdentifier::from_block(&block), block))
    } else {
        // Retrieve the previous block's identifier
        let prev_block = block_cache
            .get_block_by_height(block_height - 1, false)
            .await?;
        let prev_block_id = BlockIdentifier::from_block(&prev_block);

        // Retrieve the current block
        Ok((prev_block_id, block))
    }
}

#[derive(Clone, Debug)]
pub struct BlockInfo {
    /// Block identifier (block hash & block height)
    pub block_id: BlockIdentifier,
    /// Milliseconds timestamp
    pub timestamp: u64,
    /// Last version in block for getting state
    pub last_version: u64,
}

impl BlockInfo {
    pub fn from_block(block: &aptos_rest_client::aptos_api_types::Block) -> BlockInfo {
        BlockInfo {
            block_id: BlockIdentifier::from_block(block),
            timestamp: get_timestamp(block.block_timestamp.0),
            last_version: block.last_version.0,
        }
    }
}

/// A cache of [`BlockInfo`] to allow us to keep track of the block boundaries
#[derive(Debug)]
pub struct BlockCache {
    blocks: RwLock<BTreeMap<u64, BlockInfo>>,
    hashes: RwLock<BTreeMap<HashValue, u64>>,
    rest_client: Arc<aptos_rest_client::Client>,
}

impl BlockCache {
    pub fn new(rest_client: Arc<aptos_rest_client::Client>) -> Self {
        let mut blocks = BTreeMap::new();
        let mut hashes = BTreeMap::new();

        let genesis_hash = HashValue::zero();
        let block_info = BlockInfo {
            block_id: BlockIdentifier {
                index: 0,
                hash: to_hex_lower(&genesis_hash),
            },
            timestamp: Y2K_MS,
            last_version: 0,
        };
        // Genesis is always index 0
        blocks.insert(0, block_info);
        hashes.insert(genesis_hash, 0);

        // Insert the genesis block
        BlockCache {
            blocks: RwLock::new(blocks),
            hashes: RwLock::new(hashes),
            rest_client,
        }
    }

    pub async fn get_block_info_by_height(&self, height: u64) -> ApiResult<BlockInfo> {
        // If we cached it, get the information associated
        if let Some(info) = self.blocks.read().unwrap().get(&height) {
            return Ok(info.clone());
        }

        // Do this not in an else to allow function to be Send
        let block = self.get_block_by_height(height, false).await?;
        Ok(BlockInfo::from_block(&block))
    }

    pub async fn get_block_by_height(
        &self,
        height: u64,
        with_transactions: bool,
    ) -> ApiResult<aptos_rest_client::aptos_api_types::Block> {
        let block = self
            .rest_client
            .get_block_by_height(height, with_transactions)
            .await?
            .into_inner();
        let block_id = BlockInfo::from_block(&block);
        self.blocks
            .write()
            .unwrap()
            .insert(block.block_height.0, block_id);
        self.hashes
            .write()
            .unwrap()
            .insert(block.block_hash, block.block_height.0);

        Ok(block)
    }

    /// Retrieve the block info for the hash
    ///
    /// This is particularly bad, since there's no index on this value.  It can only be derived
    /// from the cache, otherwise it needs to fail immediately.  This cache will need to be saved
    /// somewhere for these purposes.
    ///
    /// We could use the BlockMetadata transaction's hash rather than the block hash as a hack,
    /// and that is always indexed
    ///
    /// TODO: Improve reliability
    pub fn get_block_height_by_hash(&self, hash: &HashValue) -> ApiResult<u64> {
        if let Some(height) = self.hashes.read().unwrap().get(hash) {
            Ok(*height)
        } else {
            // TODO: We can alternatively scan backwards in time to find the hash
            // If for some reason the block doesn't get found, retry with block incomplete
            Err(ApiError::BlockIncomplete)
        }
    }
}
