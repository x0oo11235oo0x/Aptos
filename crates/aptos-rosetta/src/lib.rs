// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

//! Aptos Rosetta API
//!
//! [Rosetta API Spec](https://www.rosetta-api.org/docs/Reference.html)

use crate::{
    account::CoinCache,
    block::BlockCache,
    common::{handle_request, with_context},
    error::{ApiError, ApiResult},
};
use aptos_config::config::ApiConfig;
use aptos_logger::debug;
use aptos_types::account_address::AccountAddress;
use aptos_types::chain_id::ChainId;
use aptos_warp_webserver::WebServer;
use aptos_warp_webserver::{logger, Error};
use std::collections::BTreeMap;
use std::{convert::Infallible, sync::Arc};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use warp::{
    http::{HeaderValue, Method, StatusCode},
    reject::{MethodNotAllowed, PayloadTooLarge, UnsupportedMediaType},
    reply, Filter, Rejection, Reply,
};

mod account;
mod block;
mod construction;
mod network;

pub mod client;
pub mod common;
pub mod error;
pub mod types;

pub const NODE_VERSION: &str = "0.1";
pub const ROSETTA_VERSION: &str = "1.4.12";

type SequenceNumber = u64;

/// Rosetta API context for use on all APIs
#[derive(Clone, Debug)]
pub struct RosettaContext {
    /// A rest client to connect to a fullnode
    rest_client: Option<Arc<aptos_rest_client::Client>>,
    /// ChainId of the chain to connect to
    pub chain_id: ChainId,
    /// Coin cache for looking up Currency details
    pub coin_cache: Arc<CoinCache>,
    /// Block index cache
    pub block_cache: Option<Arc<BlockCache>>,
    pub accounts: Arc<Mutex<BTreeMap<AccountAddress, SequenceNumber>>>,
}

impl RosettaContext {
    fn rest_client(&self) -> ApiResult<Arc<aptos_rest_client::Client>> {
        if let Some(ref client) = self.rest_client {
            Ok(client.clone())
        } else {
            Err(ApiError::NodeIsOffline)
        }
    }

    fn block_cache(&self) -> ApiResult<Arc<BlockCache>> {
        if let Some(ref block_cache) = self.block_cache {
            Ok(block_cache.clone())
        } else {
            Err(ApiError::NodeIsOffline)
        }
    }
}

/// Creates HTTP server (warp-based) for Rosetta
pub fn bootstrap(
    chain_id: ChainId,
    api_config: ApiConfig,
    rest_client: Option<aptos_rest_client::Client>,
) -> anyhow::Result<tokio::runtime::Runtime> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .thread_name("rosetta")
        .enable_all()
        .build()
        .expect("[rosetta] failed to create runtime");

    debug!("Starting up Rosetta server with {:?}", api_config);

    runtime.spawn(bootstrap_async(chain_id, api_config, rest_client));
    Ok(runtime)
}

/// Creates HTTP server for Rosetta in an async context
pub async fn bootstrap_async(
    chain_id: ChainId,
    api_config: ApiConfig,
    rest_client: Option<aptos_rest_client::Client>,
) -> anyhow::Result<JoinHandle<()>> {
    debug!("Starting up Rosetta server with {:?}", api_config);
    let api = WebServer::from(api_config);
    let handle = tokio::spawn(async move {
        // If it's Online mode, add the block cache
        let rest_client = rest_client.map(Arc::new);
        let block_cache = rest_client
            .as_ref()
            .map(|rest_client| Arc::new(BlockCache::new(rest_client.clone())));

        let context = RosettaContext {
            rest_client: rest_client.clone(),
            chain_id,
            coin_cache: Arc::new(CoinCache::new()),
            block_cache,
            accounts: Arc::new(Mutex::new(BTreeMap::new())),
        };
        api.serve(routes(context)).await;
    });
    Ok(handle)
}

/// Collection of all routes for the server
pub fn routes(
    context: RosettaContext,
) -> impl Filter<Extract = impl Reply, Error = Infallible> + Clone {
    account::routes(context.clone())
        .or(block::block_route(context.clone()))
        .or(construction::combine_route(context.clone()))
        .or(construction::derive_route(context.clone()))
        .or(construction::hash_route(context.clone()))
        .or(construction::metadata_route(context.clone()))
        .or(construction::parse_route(context.clone()))
        .or(construction::payloads_route(context.clone()))
        .or(construction::preprocess_route(context.clone()))
        .or(construction::submit_route(context.clone()))
        .or(network::list_route(context.clone()))
        .or(network::options_route(context.clone()))
        .or(network::status_route(context.clone()))
        .or(health_check_route(context))
        .with(
            warp::cors()
                .allow_any_origin()
                .allow_methods(vec![Method::GET, Method::POST])
                .allow_headers(vec![warp::http::header::CONTENT_TYPE]),
        )
        .with(logger())
        .recover(handle_rejection)
}

/// Handle error codes from warp
async fn handle_rejection(err: Rejection) -> Result<impl Reply, Infallible> {
    let code;
    let body;

    debug!("Failed with: {:?}", err);

    if err.is_not_found() {
        code = StatusCode::NOT_FOUND;
        body = reply::json(&Error::new(code, "Not Found".to_owned()));
    } else if let Some(cause) = err.find::<warp::cors::CorsForbidden>() {
        code = StatusCode::FORBIDDEN;
        body = reply::json(&Error::new(code, cause.to_string()));
    } else if let Some(cause) = err.find::<warp::body::BodyDeserializeError>() {
        code = StatusCode::BAD_REQUEST;
        body = reply::json(&Error::new(code, cause.to_string()));
    } else if let Some(cause) = err.find::<warp::reject::LengthRequired>() {
        code = StatusCode::LENGTH_REQUIRED;
        body = reply::json(&Error::new(code, cause.to_string()));
    } else if let Some(cause) = err.find::<PayloadTooLarge>() {
        code = StatusCode::PAYLOAD_TOO_LARGE;
        body = reply::json(&Error::new(code, cause.to_string()));
    } else if let Some(cause) = err.find::<UnsupportedMediaType>() {
        code = StatusCode::UNSUPPORTED_MEDIA_TYPE;
        body = reply::json(&Error::new(code, cause.to_string()));
    } else if let Some(cause) = err.find::<MethodNotAllowed>() {
        code = StatusCode::METHOD_NOT_ALLOWED;
        body = reply::json(&Error::new(code, cause.to_string()));
    } else {
        code = StatusCode::INTERNAL_SERVER_ERROR;
        body = reply::json(&Error::new(code, format!("unexpected error: {:?}", err)));
    }
    let mut rep = reply::with_status(body, code).into_response();
    rep.headers_mut()
        .insert("access-control-allow-origin", HeaderValue::from_static("*"));
    Ok(rep)
}

/// These parameters are directly passed onto the underlying rest server for a healthcheck
#[derive(serde::Deserialize)]
struct HealthCheckParams {
    pub duration_secs: Option<u64>,
}

/// Default amount of time the fullnode is accepted to be behind (arbitrarily it's 5 minutes)
const HEALTH_CHECK_DEFAULT_SECS: u64 = 300;

pub fn health_check_route(
    server_context: RosettaContext,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("-" / "healthy")
        .and(warp::path::end())
        .and(warp::query().map(move |params: HealthCheckParams| params))
        .and(with_context(server_context))
        .and_then(handle_request(health_check))
}

/// Calls the underlying REST health check
async fn health_check(
    params: HealthCheckParams,
    server_context: RosettaContext,
) -> ApiResult<&'static str> {
    let rest_client = server_context.rest_client()?;
    let duration_secs = params.duration_secs.unwrap_or(HEALTH_CHECK_DEFAULT_SECS);
    rest_client.health_check(duration_secs).await?;

    Ok("aptos-node:ok")
}
