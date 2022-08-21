// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

mod aptos_debug_natives;
mod manifest;
pub mod package_hooks;
pub use package_hooks::*;
pub mod stored_package;
mod transactional_tests_runner;

pub use stored_package::*;

use crate::common::types::MoveManifestAccountWrapper;
use crate::common::types::{ProfileOptions, RestOptions};
use crate::common::utils::{
    create_dir_if_not_exist, dir_default_to_current, prompt_yes_with_override, write_to_file,
};
use crate::move_tool::manifest::{
    Dependency, ManifestNamedAddress, MovePackageManifest, PackageInfo,
};
use crate::{
    common::{
        types::{
            load_account_arg, CliError, CliTypedResult, MovePackageDir, PromptOptions,
            TransactionOptions, TransactionSummary,
        },
        utils::check_if_file_exists,
    },
    CliCommand, CliResult,
};
use aptos_gas::NativeGasParameters;
use aptos_module_verifier::module_init::verify_module_init_function;
use aptos_rest_client::aptos_api_types::MoveType;
use aptos_transactional_test_harness::run_aptos_test;
use aptos_types::account_address::AccountAddress;
use aptos_types::transaction::{EntryFunction, ModuleBundle, TransactionPayload};
use async_trait::async_trait;
use clap::{ArgEnum, Parser, Subcommand};
use framework::natives::code::UpgradePolicy;
use framework::{BuildOptions, BuiltPackage};
use itertools::Itertools;
use move_deps::move_cli::base::test::UnitTestResult;
use move_deps::move_command_line_common::env::MOVE_HOME;
use move_deps::{
    move_cli,
    move_core_types::{
        identifier::Identifier,
        language_storage::{ModuleId, TypeTag},
    },
    move_package::{source_package::layout::SourcePackageLayout, BuildConfig},
    move_prover,
    move_unit_test::UnitTestingConfig,
};
use std::fmt::{Display, Formatter};
use std::{
    collections::BTreeMap,
    convert::TryFrom,
    path::{Path, PathBuf},
    str::FromStr,
};
use tokio::task;
use transactional_tests_runner::TransactionalTestOpts;

/// Tool for Move related operations
///
/// This tool lets you compile, test, and publish Move code, in addition
/// to run any other tools that help run, verify, or provide information
/// about this code.
#[derive(Subcommand)]
pub enum MoveTool {
    Compile(CompilePackage),
    Init(InitPackage),
    Publish(PublishPackage),
    Download(DownloadPackage),
    List(ListPackage),
    Clean(CleanPackage),
    Run(RunFunction),
    Test(TestPackage),
    Prove(ProvePackage),
    TransactionalTest(TransactionalTestOpts),
}

impl MoveTool {
    pub async fn execute(self) -> CliResult {
        match self {
            MoveTool::Compile(tool) => tool.execute_serialized().await,
            MoveTool::Init(tool) => tool.execute_serialized_success().await,
            MoveTool::Publish(tool) => tool.execute_serialized().await,
            MoveTool::Download(tool) => tool.execute_serialized().await,
            MoveTool::List(tool) => tool.execute_serialized().await,
            MoveTool::Clean(tool) => tool.execute_serialized().await,
            MoveTool::Run(tool) => tool.execute_serialized().await,
            MoveTool::Test(tool) => tool.execute_serialized().await,
            MoveTool::Prove(tool) => tool.execute_serialized().await,
            MoveTool::TransactionalTest(tool) => tool.execute_serialized_success().await,
        }
    }
}

/// Creates a new Move package at the given location
#[derive(Parser)]
pub struct InitPackage {
    /// Name of the new move package
    #[clap(long)]
    pub(crate) name: String,

    /// Path to create the new move package
    #[clap(long, parse(from_os_str))]
    pub(crate) package_dir: Option<PathBuf>,

    /// Named addresses for the move binary
    ///
    /// Example: alice=0x1234,bob=0x5678
    ///
    /// Note: This will fail if there are duplicates in the Move.toml file remove those first.
    #[clap(long, parse(try_from_str = crate::common::utils::parse_map), default_value = "")]
    pub(crate) named_addresses: BTreeMap<String, MoveManifestAccountWrapper>,

    #[clap(flatten)]
    pub(crate) prompt_options: PromptOptions,
}

#[async_trait]
impl CliCommand<()> for InitPackage {
    fn command_name(&self) -> &'static str {
        "InitPackage"
    }

    async fn execute(self) -> CliTypedResult<()> {
        let package_dir = dir_default_to_current(self.package_dir.clone())?;
        let addresses = self
            .named_addresses
            .into_iter()
            .map(|(key, value)| (key, value.account_address.into()))
            .collect();

        init_move_dir(
            package_dir.as_path(),
            &self.name,
            "devnet",
            addresses,
            self.prompt_options,
        )
    }
}

pub fn init_move_dir(
    package_dir: &Path,
    name: &str,
    rev: &str,
    addresses: BTreeMap<String, ManifestNamedAddress>,
    prompt_options: PromptOptions,
) -> CliTypedResult<()> {
    let move_toml = package_dir.join(SourcePackageLayout::Manifest.path());
    check_if_file_exists(move_toml.as_path(), prompt_options)?;
    create_dir_if_not_exist(
        package_dir
            .join(SourcePackageLayout::Sources.path())
            .as_path(),
    )?;

    let mut dependencies = BTreeMap::new();
    dependencies.insert(
        "AptosFramework".to_string(),
        Dependency {
            local: None,
            git: Some("https://github.com/aptos-labs/aptos-core.git".to_string()),
            rev: Some(rev.to_string()),
            subdir: Some("aptos-move/framework/aptos-framework".to_string()),
            aptos: None,
            address: None,
        },
    );
    let manifest = MovePackageManifest {
        package: PackageInfo {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            author: None,
        },
        addresses,
        dependencies,
    };

    write_to_file(
        move_toml.as_path(),
        SourcePackageLayout::Manifest.location_str(),
        toml::to_string_pretty(&manifest)
            .map_err(|err| CliError::UnexpectedError(err.to_string()))?
            .as_bytes(),
    )
}

/// Compiles a package and returns the [`ModuleId`]s
#[derive(Parser)]
pub struct CompilePackage {
    #[clap(flatten)]
    pub(crate) move_options: MovePackageDir,
}

#[async_trait]
impl CliCommand<Vec<String>> for CompilePackage {
    fn command_name(&self) -> &'static str {
        "CompilePackage"
    }

    async fn execute(self) -> CliTypedResult<Vec<String>> {
        let build_options = BuildOptions {
            with_srcs: false,
            with_abis: true,
            with_source_maps: true,
            with_error_map: true,
            install_dir: self.move_options.output_dir.clone(),
            named_addresses: self.move_options.named_addresses(),
        };
        let pack = BuiltPackage::build(self.move_options.get_package_path()?, build_options)
            .map_err(|e| CliError::MoveCompilationError(format!("{:#}", e)))?;
        let mut ids = Vec::new();
        for module in pack.modules() {
            verify_module_init_function(module)
                .map_err(|e| CliError::MoveCompilationError(e.to_string()))?;
            ids.push(module.self_id().to_string());
        }
        Ok(ids)
    }
}

/// Runs Move unit tests for a package
///
/// This will run Move unit tests against a package with debug mode
/// turned on.  Note, that move code warnings currently block tests from running.
#[derive(Parser)]
pub struct TestPackage {
    /// A filter string to determine which unit tests to run
    #[clap(long)]
    pub filter: Option<String>,

    #[clap(flatten)]
    pub(crate) move_options: MovePackageDir,
}

#[async_trait]
impl CliCommand<&'static str> for TestPackage {
    fn command_name(&self) -> &'static str {
        "TestPackage"
    }

    async fn execute(self) -> CliTypedResult<&'static str> {
        let config = BuildConfig {
            additional_named_addresses: self.move_options.named_addresses(),
            test_mode: true,
            install_dir: self.move_options.output_dir.clone(),
            ..Default::default()
        };
        let result = move_cli::base::test::run_move_unit_tests(
            self.move_options.get_package_path()?.as_path(),
            config,
            UnitTestingConfig {
                filter: self.filter,
                ..UnitTestingConfig::default_with_bound(Some(100_000))
            },
            // TODO(Gas): we may want to switch to non-zero costs in the future
            aptos_debug_natives::aptos_debug_natives(NativeGasParameters::zeros()),
            false,
            &mut std::io::stdout(),
        )
        .map_err(|err| CliError::UnexpectedError(err.to_string()))?;

        match result {
            UnitTestResult::Success => Ok("Success"),
            UnitTestResult::Failure => Err(CliError::MoveTestError),
        }
    }
}

#[async_trait]
impl CliCommand<()> for TransactionalTestOpts {
    fn command_name(&self) -> &'static str {
        "TransactionalTest"
    }

    async fn execute(self) -> CliTypedResult<()> {
        let root_path = self.root_path.display().to_string();

        let requirements = vec![transactional_tests_runner::Requirements::new(
            run_aptos_test,
            "tests".to_string(),
            root_path,
            self.pattern.clone(),
        )];

        transactional_tests_runner::runner(&self, &requirements)
    }
}

/// Proves the Move package
///
/// This is a tool for formal verification of a Move package using
/// the Move prover
#[derive(Parser)]
pub struct ProvePackage {
    /// A filter string to determine which files to verify
    #[clap(long)]
    pub filter: Option<String>,

    #[clap(flatten)]
    move_options: MovePackageDir,
}

#[async_trait]
impl CliCommand<&'static str> for ProvePackage {
    fn command_name(&self) -> &'static str {
        "ProvePackage"
    }

    async fn execute(self) -> CliTypedResult<&'static str> {
        let config = BuildConfig {
            additional_named_addresses: self.move_options.named_addresses(),
            test_mode: true,
            install_dir: self.move_options.output_dir.clone(),
            ..Default::default()
        };
        let result = task::spawn_blocking(move || {
            move_cli::base::prove::run_move_prover(
                config,
                self.move_options.get_package_path()?.as_path(),
                &self.filter,
                true,
                move_prover::cli::Options::default(),
            )
        })
        .await
        .map_err(|err| CliError::UnexpectedError(err.to_string()))?;

        match result {
            Ok(_) => Ok("Success"),
            Err(e) => Err(CliError::MoveProverError(format!("{:#}", e))),
        }
    }
}

/// Publishes the modules in a Move package to the Aptos blockchain
#[derive(Parser)]
pub struct PublishPackage {
    /// Whether to use the legacy publishing flow. This will be soon removed.
    #[clap(long)]
    pub(crate) legacy_flow: bool,

    /// Whether to override the check for maximal size of published data.
    #[clap(long)]
    pub(crate) override_size_check: bool,

    /// What artifacts to include in the package. This can be one of `none`, `sparse`, and
    /// `all`. `none` is the most compact form and does not allow to reconstruct a source
    /// package from chain; `sparse` is the minimal set of artifacts needed to reconstruct
    /// a source package; `all` includes all available artifacts. The choice of included
    /// artifacts heavily influences the size and therefore gas cost of publishing: `none`
    /// is the size of bytecode alone; `sparse` is roughly 2 times as much; and `all` 3-4
    /// as much.
    #[clap(long, default_value_t = IncludedArtifacts::Sparse)]
    pub(crate) included_artifacts: IncludedArtifacts,

    #[clap(flatten)]
    pub(crate) move_options: MovePackageDir,
    #[clap(flatten)]
    pub(crate) txn_options: TransactionOptions,
}

#[derive(ArgEnum, Clone, Copy, Debug)]
pub enum IncludedArtifacts {
    None,
    Sparse,
    All,
}

impl Display for IncludedArtifacts {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        use IncludedArtifacts::*;
        match self {
            None => f.write_str("none"),
            Sparse => f.write_str("sparse"),
            All => f.write_str("all"),
        }
    }
}

impl FromStr for IncludedArtifacts {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use IncludedArtifacts::*;
        match s {
            "none" => Ok(None),
            "sparse" => Ok(Sparse),
            "all" => Ok(All),
            _ => Err("unknown variant"),
        }
    }
}

impl IncludedArtifacts {
    pub(crate) fn build_options(
        self,
        named_addresses: BTreeMap<String, AccountAddress>,
    ) -> BuildOptions {
        use IncludedArtifacts::*;
        match self {
            None => BuildOptions {
                with_srcs: false,
                with_abis: false,
                with_source_maps: false,
                // Always enable error map bytecode injection
                with_error_map: true,
                named_addresses,
                install_dir: Option::None,
            },
            Sparse => BuildOptions {
                with_srcs: true,
                with_abis: false,
                with_source_maps: false,
                with_error_map: true,
                named_addresses,
                install_dir: Option::None,
            },
            All => BuildOptions {
                with_srcs: true,
                with_abis: true,
                with_source_maps: true,
                with_error_map: true,
                named_addresses,
                install_dir: Option::None,
            },
        }
    }
}

pub const MAX_PUBLISH_PACKAGE_SIZE: usize = 60_000;

#[async_trait]
impl CliCommand<TransactionSummary> for PublishPackage {
    fn command_name(&self) -> &'static str {
        "PublishPackage"
    }

    async fn execute(self) -> CliTypedResult<TransactionSummary> {
        let PublishPackage {
            move_options,
            txn_options,
            legacy_flow,
            override_size_check,
            included_artifacts,
        } = self;
        let package_path = move_options.get_package_path()?;
        let options = included_artifacts.build_options(move_options.named_addresses());
        let package = BuiltPackage::build(package_path, options)?;
        let compiled_units = package.extract_code();
        if legacy_flow {
            // Send the compiled module using a module bundle
            txn_options
                .submit_transaction(TransactionPayload::ModuleBundle(ModuleBundle::new(
                    compiled_units,
                )))
                .await
                .map(TransactionSummary::from)
        } else {
            // Send the compiled module and metadata using the code::publish_package_txn.
            let metadata = package.extract_metadata()?;
            let payload = cached_packages::aptos_stdlib::code_publish_package_txn(
                bcs::to_bytes(&metadata).expect("PackageMetadata has BCS"),
                compiled_units,
            );
            let size = bcs::serialized_size(&payload)?;
            if !override_size_check && size > MAX_PUBLISH_PACKAGE_SIZE {
                return Err(CliError::UnexpectedError(format!(
                    "The package is larger than {}k ({}k)! To lower the size \
                you may want to include less artifacts via `--included_artifacts`. \
                You can also override this check with `--override-size-check",
                    MAX_PUBLISH_PACKAGE_SIZE / 1000,
                    size / 1000
                )));
            }
            txn_options
                .submit_transaction(payload)
                .await
                .map(TransactionSummary::from)
        }
    }
}

/// Downloads a package and stores it in a directory named after the package
///
/// This lets you retrieve packages directly from the blockchain for inspection
/// and use as a local dependency in testing.
#[derive(Parser)]
pub struct DownloadPackage {
    /// Address of the account containing the package
    #[clap(long, parse(try_from_str=crate::common::types::load_account_arg))]
    pub(crate) account: AccountAddress,

    /// Name of the package
    #[clap(long)]
    pub package: String,

    /// Directory to store downloaded package. Defaults to the current directory.
    #[clap(long, parse(from_os_str))]
    pub output_dir: Option<PathBuf>,

    #[clap(flatten)]
    pub(crate) rest_options: RestOptions,
    #[clap(flatten)]
    pub(crate) profile_options: ProfileOptions,
}

#[async_trait]
impl CliCommand<&'static str> for DownloadPackage {
    fn command_name(&self) -> &'static str {
        "DownloadPackage"
    }

    async fn execute(self) -> CliTypedResult<&'static str> {
        let url = self.rest_options.url(&self.profile_options.profile)?;
        let registry = CachedPackageRegistry::create(url, self.account).await?;
        let output_dir = dir_default_to_current(self.output_dir)?;

        let package = registry
            .get_package(self.package)
            .await
            .map_err(|s| CliError::CommandArgumentError(s.to_string()))?;
        if package.upgrade_policy() == UpgradePolicy::arbitrary() {
            return Err(CliError::CommandArgumentError(
                "A package with upgrade policy `arbitrary` cannot be downloaded \
                since it is not safe to depend on such packages."
                    .to_owned(),
            ));
        }
        let package_path = output_dir.join(package.name());
        package
            .save_package_to_disk(package_path.as_path(), true)
            .map_err(|e| CliError::UnexpectedError(format!("Failed to save package: {}", e)))?;
        println!(
            "Saved package with {} module(s) to `{}`",
            package.module_names().len(),
            package_path.display()
        );
        Ok("Download succeeded")
    }
}

/// Lists information about packages and modules on-chain
#[derive(Parser)]
pub struct ListPackage {
    /// Address of the account for which to list packages.
    #[clap(long, parse(try_from_str=crate::common::types::load_account_arg))]
    pub(crate) account: AccountAddress,

    /// Type of resources to query
    #[clap(long, default_value_t = ListQuery::Packages)]
    query: ListQuery,

    #[clap(flatten)]
    rest_options: RestOptions,
    #[clap(flatten)]
    pub(crate) profile_options: ProfileOptions,
}

#[derive(ArgEnum, Clone, Copy, Debug)]
pub enum ListQuery {
    Packages,
}

impl Display for ListQuery {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ListQuery::Packages => "packages",
        })
    }
}

impl FromStr for ListQuery {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "packages" => Ok(ListQuery::Packages),
            _ => Err("Invalid query. Valid values are modules, packages"),
        }
    }
}

#[async_trait]
impl CliCommand<&'static str> for ListPackage {
    fn command_name(&self) -> &'static str {
        "ListPackage"
    }

    async fn execute(self) -> CliTypedResult<&'static str> {
        let url = self.rest_options.url(&self.profile_options.profile)?;
        let registry = CachedPackageRegistry::create(url, self.account).await?;
        match self.query {
            ListQuery::Packages => {
                for name in registry.package_names() {
                    let data = registry.get_package(name).await?;
                    println!("package {}", data.name());
                    println!("  upgrade_policy: {}", data.upgrade_policy());
                    println!("  upgrade_number: {}", data.upgrade_number());
                    println!("  modules: {}", data.module_names().into_iter().join(", "));
                    println!(
                        "  build_info:\n    {}",
                        data.build_info().replace('\n', "\n    ")
                    );
                }
            }
        }
        Ok("list succeeded")
    }
}

/// Cleans derived artifacts of a package.
#[derive(Parser)]
pub struct CleanPackage {
    #[clap(flatten)]
    pub(crate) move_options: MovePackageDir,
    #[clap(flatten)]
    pub(crate) prompt_options: PromptOptions,
}

#[async_trait]
impl CliCommand<&'static str> for CleanPackage {
    fn command_name(&self) -> &'static str {
        "Clean"
    }
    async fn execute(self) -> CliTypedResult<&'static str> {
        let path = self.move_options.get_package_path()?;
        let build_dir = path.join("build");
        std::fs::remove_dir_all(build_dir)
            .map_err(|e| CliError::IO("Removing Move build dir".to_string(), e))?;

        let move_dir = &*MOVE_HOME;
        if prompt_yes_with_override(
            &format!(
                "Do you also want to delete the local package download cache at `{}`?",
                move_dir
            ),
            self.prompt_options,
        )
        .is_ok()
        {
            std::fs::remove_dir_all(PathBuf::from(move_dir))
                .map_err(|e| CliError::IO("Removing Move cache dir".to_string(), e))?;
        }
        Ok("succeeded")
    }
}

/// Run a Move function
#[derive(Parser)]
pub struct RunFunction {
    /// Function name as `<ADDRESS>::<MODULE_ID>::<FUNCTION_NAME>`
    ///
    /// Example: `0x842ed41fad9640a2ad08fdd7d3e4f7f505319aac7d67e1c0dd6a7cce8732c7e3::message::set_message`
    #[clap(long)]
    pub(crate) function_id: MemberId,

    /// Arguments combined with their type separated by spaces.
    ///
    /// Supported types [u8, u64, u128, bool, hex, string, address]
    ///
    /// Example: `address:0x1 bool:true u8:0`
    #[clap(long, multiple_values = true)]
    pub(crate) args: Vec<ArgWithType>,

    /// TypeTag arguments separated by spaces.
    ///
    /// Example: `u8 u64 u128 bool address vector signer`
    #[clap(long, multiple_values = true)]
    pub(crate) type_args: Vec<MoveType>,

    #[clap(flatten)]
    pub(crate) txn_options: TransactionOptions,
}

#[async_trait]
impl CliCommand<TransactionSummary> for RunFunction {
    fn command_name(&self) -> &'static str {
        "RunFunction"
    }

    async fn execute(self) -> CliTypedResult<TransactionSummary> {
        let args: Vec<Vec<u8>> = self
            .args
            .into_iter()
            .map(|arg_with_type| arg_with_type.arg)
            .collect();
        let mut type_args: Vec<TypeTag> = Vec::new();

        // These TypeArgs are used for generics
        for type_arg in self.type_args.into_iter() {
            let type_tag = TypeTag::try_from(type_arg)
                .map_err(|err| CliError::UnableToParse("--type-args", err.to_string()))?;
            type_args.push(type_tag)
        }

        self.txn_options
            .submit_transaction(TransactionPayload::EntryFunction(EntryFunction::new(
                self.function_id.module_id,
                self.function_id.member_id,
                type_args,
                args,
            )))
            .await
            .map(TransactionSummary::from)
    }
}

#[derive(Clone, Debug)]
pub(crate) enum FunctionArgType {
    Address,
    Bool,
    Hex,
    String,
    U8,
    U64,
    U128,
}

impl FunctionArgType {
    fn parse_arg(&self, arg: &str) -> CliTypedResult<Vec<u8>> {
        match self {
            FunctionArgType::Address => bcs::to_bytes(
                &load_account_arg(arg)
                    .map_err(|err| CliError::UnableToParse("address", err.to_string()))?,
            ),
            FunctionArgType::Bool => bcs::to_bytes(
                &bool::from_str(arg)
                    .map_err(|err| CliError::UnableToParse("bool", err.to_string()))?,
            ),
            FunctionArgType::Hex => bcs::to_bytes(
                &hex::decode(arg).map_err(|err| CliError::UnableToParse("hex", err.to_string()))?,
            ),
            FunctionArgType::String => bcs::to_bytes(arg),
            FunctionArgType::U8 => bcs::to_bytes(
                &u8::from_str(arg).map_err(|err| CliError::UnableToParse("u8", err.to_string()))?,
            ),
            FunctionArgType::U64 => bcs::to_bytes(
                &u64::from_str(arg)
                    .map_err(|err| CliError::UnableToParse("u64", err.to_string()))?,
            ),
            FunctionArgType::U128 => bcs::to_bytes(
                &u128::from_str(arg)
                    .map_err(|err| CliError::UnableToParse("u128", err.to_string()))?,
            ),
        }
        .map_err(|err| CliError::BCS("arg", err))
    }
}

impl FromStr for FunctionArgType {
    type Err = CliError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "address" => Ok(FunctionArgType::Address),
            "bool" => Ok(FunctionArgType::Bool),
            "hex" => Ok(FunctionArgType::Hex),
            "string" => Ok(FunctionArgType::String),
            "u8" => Ok(FunctionArgType::U8),
            "u64" => Ok(FunctionArgType::U64),
            "u128" => Ok(FunctionArgType::U128),
            str => Err(CliError::CommandArgumentError(format!("Invalid arg type '{}'.  Must be one of: ['address','bool','hex','string','u8','u64','u128']", str))),
        }
    }
}

/// A parseable arg with a type separated by a colon
pub struct ArgWithType {
    pub(crate) _ty: FunctionArgType,
    pub(crate) arg: Vec<u8>,
}

impl FromStr for ArgWithType {
    type Err = CliError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<_> = s.split(':').collect();
        if parts.len() != 2 {
            return Err(CliError::CommandArgumentError(
                "Arguments must be pairs of <type>:<arg> e.g. bool:true".to_string(),
            ));
        }

        let ty = FunctionArgType::from_str(parts.first().unwrap())?;
        let arg = parts.last().unwrap();
        let arg = ty.parse_arg(arg)?;

        Ok(ArgWithType { _ty: ty, arg })
    }
}

/// Identifier of a module member (function or struct).
#[derive(Debug, Clone)]
pub struct MemberId {
    pub module_id: ModuleId,
    pub member_id: Identifier,
}

fn parse_member_id(function_id: &str) -> CliTypedResult<MemberId> {
    let ids: Vec<&str> = function_id.split_terminator("::").collect();
    if ids.len() != 3 {
        return Err(CliError::CommandArgumentError(
            "FunctionId is not well formed.  Must be of the form <address>::<module>::<function>"
                .to_string(),
        ));
    }
    let address = load_account_arg(ids.get(0).unwrap())?;
    let module = Identifier::from_str(ids.get(1).unwrap())
        .map_err(|err| CliError::UnableToParse("Module Name", err.to_string()))?;
    let member_id = Identifier::from_str(ids.get(2).unwrap())
        .map_err(|err| CliError::UnableToParse("Member Name", err.to_string()))?;
    let module_id = ModuleId::new(address, module);
    Ok(MemberId {
        module_id,
        member_id,
    })
}

impl FromStr for MemberId {
    type Err = CliError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_member_id(s)
    }
}
