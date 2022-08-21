// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use aptos::test::CliTestFramework;
use aptos_config::{keys::ConfigKey, utils::get_available_port};
use aptos_crypto::ed25519::Ed25519PrivateKey;
use aptos_faucet::FaucetArgs;
use aptos_genesis::builder::{InitConfigFn, InitGenesisConfigFn};
use aptos_logger::info;
use aptos_types::{account_config::aptos_test_root_address, chain_id::ChainId};
use forge::Node;
use forge::{Factory, LocalFactory, LocalSwarm};
use framework::ReleaseBundle;
use once_cell::sync::Lazy;
use rand::rngs::OsRng;
use std::{num::NonZeroUsize, path::PathBuf, sync::Arc};
use tokio::task::JoinHandle;

pub struct SwarmBuilder {
    local: bool,
    num_validators: NonZeroUsize,
    genesis_framework: Option<ReleaseBundle>,
    init_config: Option<InitConfigFn>,
    init_genesis_config: Option<InitGenesisConfigFn>,
}

impl SwarmBuilder {
    pub fn new(local: bool, num_validators: usize) -> Self {
        Self {
            local,
            num_validators: NonZeroUsize::new(num_validators).unwrap(),
            genesis_framework: None,
            init_config: None,
            init_genesis_config: None,
        }
    }

    pub fn new_local(num_validators: usize) -> Self {
        Self::new(true, num_validators)
    }

    pub fn with_aptos(mut self) -> Self {
        self.genesis_framework = Some(cached_packages::head_release_bundle().clone());
        self
    }

    pub fn with_init_config(mut self, init_config: InitConfigFn) -> Self {
        self.init_config = Some(init_config);
        self
    }

    pub fn with_init_genesis_config(mut self, init_genesis_config: InitGenesisConfigFn) -> Self {
        self.init_genesis_config = Some(init_genesis_config);
        self
    }

    // Gas is not enabled with this setup, it's enabled via forge instance.
    pub async fn build(self) -> LocalSwarm {
        ::aptos_logger::Logger::new().init();
        info!("Preparing to finish compiling");
        // TODO change to return Swarm trait
        // Add support for forge
        assert!(self.local);
        static FACTORY: Lazy<LocalFactory> = Lazy::new(|| LocalFactory::from_workspace().unwrap());

        let version = FACTORY.versions().max().unwrap();

        info!("Node finished compiling");

        let init_genesis_config = self.init_genesis_config;

        FACTORY
            .new_swarm_with_version(
                OsRng,
                self.num_validators,
                &version,
                self.genesis_framework,
                self.init_config,
                Some(Arc::new(move |genesis_config| {
                    if let Some(init_genesis_config) = &init_genesis_config {
                        (init_genesis_config)(genesis_config);
                    }
                })),
            )
            .await
            .unwrap()
    }

    pub async fn build_with_cli(
        self,
        num_cli_accounts: usize,
    ) -> (LocalSwarm, CliTestFramework, JoinHandle<()>) {
        let swarm = self.build().await;
        let chain_id = swarm.chain_id();
        let validator = swarm.validators().next().unwrap();
        let root_key = swarm.root_key();
        let faucet_port = get_available_port();
        let faucet = launch_faucet(
            validator.rest_api_endpoint(),
            root_key,
            chain_id,
            faucet_port,
        );
        let faucet_endpoint: reqwest::Url =
            format!("http://localhost:{}", faucet_port).parse().unwrap();
        // Connect the operator tool to the node's JSON RPC API
        let tool = CliTestFramework::new(
            validator.rest_api_endpoint(),
            faucet_endpoint,
            num_cli_accounts,
        )
        .await;
        println!(
            "Created CLI with {} accounts for LocalSwarm",
            num_cli_accounts
        );
        (swarm, tool, faucet)
    }
}

// Gas is not enabled with this setup, it's enabled via forge instance.
pub async fn new_local_swarm_with_aptos(num_validators: usize) -> LocalSwarm {
    SwarmBuilder::new_local(num_validators)
        .with_aptos()
        .build()
        .await
}

#[tokio::test]
async fn test_prevent_starting_nodes_twice() {
    // Create a validator swarm of 1 validator node
    let mut swarm = new_local_swarm_with_aptos(1).await;

    assert!(swarm.launch().await.is_err());
    let validator = swarm.validators_mut().next().unwrap();
    assert!(validator.start().is_err());
    validator.stop();
    assert!(validator.start().is_ok());
    assert!(validator.start().is_err());
}

pub fn launch_faucet(
    endpoint: reqwest::Url,
    mint_key: Ed25519PrivateKey,
    chain_id: ChainId,
    port: u16,
) -> JoinHandle<()> {
    let faucet = FaucetArgs {
        address: "127.0.0.1".to_string(),
        port,
        server_url: endpoint,
        mint_key_file_path: PathBuf::new(),
        mint_key: Some(ConfigKey::new(mint_key)),
        mint_account_address: Some(aptos_test_root_address()),
        chain_id,
        maximum_amount: None,
        do_not_delegate: true,
    };
    tokio::spawn(faucet.run())
}
