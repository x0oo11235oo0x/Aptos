// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use crate::{Factory, GenesisConfig, Result, Swarm, Version};
use anyhow::{bail, Context};
use aptos_genesis::builder::{InitConfigFn, InitGenesisConfigFn};
use framework::ReleaseBundle;
use rand::rngs::StdRng;
use std::time::Duration;
use std::{
    collections::HashMap,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

mod cargo;
mod node;
mod swarm;
pub use node::LocalNode;
pub use swarm::{LocalSwarm, SwarmDirectory};

#[derive(Clone, Debug)]
pub struct LocalVersion {
    bin: PathBuf,
    version: Version,
}

impl LocalVersion {
    pub fn new(bin: PathBuf, version: Version) -> Self {
        Self { bin, version }
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    pub fn version(&self) -> Version {
        self.version.clone()
    }
}

pub struct LocalFactory {
    versions: Arc<HashMap<Version, LocalVersion>>,
}

impl LocalFactory {
    pub fn new(versions: HashMap<Version, LocalVersion>) -> Self {
        Self {
            versions: Arc::new(versions),
        }
    }

    pub fn from_workspace() -> Result<Self> {
        let mut versions = HashMap::new();
        let new_version = cargo::get_aptos_node_binary_from_worktree().map(|(revision, bin)| {
            let version = Version::new(usize::max_value(), revision);
            LocalVersion { bin, version }
        })?;

        versions.insert(new_version.version.clone(), new_version);
        Ok(Self::new(versions))
    }

    pub fn from_revision(revision: &str) -> Result<Self> {
        let mut versions = HashMap::new();
        let new_version =
            cargo::get_aptos_node_binary_at_revision(revision).map(|(revision, bin)| {
                let version = Version::new(usize::max_value(), revision);
                LocalVersion { bin, version }
            })?;

        versions.insert(new_version.version.clone(), new_version);
        Ok(Self::new(versions))
    }

    pub fn with_revision_and_workspace(revision: &str) -> Result<Self> {
        let workspace = cargo::get_aptos_node_binary_from_worktree().map(|(revision, bin)| {
            let version = Version::new(usize::max_value(), revision);
            LocalVersion { bin, version }
        })?;
        let revision =
            cargo::get_aptos_node_binary_at_revision(revision).map(|(revision, bin)| {
                let version = Version::new(usize::min_value(), revision);
                LocalVersion { bin, version }
            })?;

        let mut versions = HashMap::new();
        versions.insert(workspace.version(), workspace);
        versions.insert(revision.version(), revision);
        Ok(Self::new(versions))
    }

    /// Create a LocalFactory with a aptos-node version built at the tip of upstream/main and the
    /// current workspace, suitable for compatibility testing.
    pub fn with_upstream_and_workspace() -> Result<Self> {
        let upstream_main = cargo::git_get_upstream_remote().map(|r| format!("{}/main", r))?;
        Self::with_revision_and_workspace(&upstream_main)
    }

    /// Create a LocalFactory with a aptos-node version built at merge-base of upstream/main and the
    /// current workspace, suitable for compatibility testing.
    pub fn with_upstream_merge_base_and_workspace() -> Result<Self> {
        let upstream_main = cargo::git_get_upstream_remote().map(|r| format!("{}/main", r))?;
        let merge_base = cargo::git_merge_base(upstream_main)?;
        Self::with_revision_and_workspace(&merge_base)
    }

    pub async fn new_swarm<R>(
        &self,
        rng: R,
        number_of_validators: NonZeroUsize,
    ) -> Result<LocalSwarm>
    where
        R: ::rand::RngCore + ::rand::CryptoRng,
    {
        let version = self.versions.keys().max().unwrap();
        self.new_swarm_with_version(rng, number_of_validators, version, None, None, None)
            .await
    }

    pub async fn new_swarm_with_version<R>(
        &self,
        rng: R,
        number_of_validators: NonZeroUsize,
        version: &Version,
        genesis_framework: Option<ReleaseBundle>,
        init_config: Option<InitConfigFn>,
        init_genesis_config: Option<InitGenesisConfigFn>,
    ) -> Result<LocalSwarm>
    where
        R: ::rand::RngCore + ::rand::CryptoRng,
    {
        println!("Preparing a new swarm");
        let mut swarm = LocalSwarm::build(
            rng,
            number_of_validators,
            self.versions.clone(),
            Some(version.clone()),
            init_config,
            init_genesis_config,
            None,
            genesis_framework,
        )?;

        swarm
            .launch()
            .await
            .with_context(|| format!("Swarm logs can be found here: {}", swarm.logs_location()))?;

        Ok(swarm)
    }
}

#[async_trait::async_trait]
impl Factory for LocalFactory {
    fn versions<'a>(&'a self) -> Box<dyn Iterator<Item = Version> + 'a> {
        Box::new(self.versions.keys().cloned())
    }

    async fn launch_swarm(
        &self,
        rng: &mut StdRng,
        num_validators: NonZeroUsize,
        // TODO: support fullnodes in local forge
        _num_fullnodes: usize,
        version: &Version,
        _genesis_version: &Version,
        genesis_config: Option<&GenesisConfig>,
        _cleanup_duration: Duration,
    ) -> Result<Box<dyn Swarm>> {
        let framework = match genesis_config {
            Some(config) => match config {
                GenesisConfig::Bundle(bundle) => Some(bundle.clone()),
                GenesisConfig::Path(_) => {
                    bail!("local forge backend does not support flattened dir for genesis")
                }
            },
            None => None,
        };
        let swarm = self
            .new_swarm_with_version(rng, num_validators, version, framework, None, None)
            .await?;

        Ok(Box::new(swarm))
    }
}
