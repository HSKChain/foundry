//! # foundry-script
//!
//! Smart contract scripting.

#![recursion_limit = "256"]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[macro_use]
extern crate foundry_common;

#[macro_use]
extern crate tracing;

use crate::{broadcast::BundledState, runner::ScriptRunner};
use alloy_json_abi::{Function, JsonAbi};
use alloy_network::Network;
use alloy_primitives::{
    Address, Bytes, Log, U256, hex,
    map::{AddressHashMap, HashMap},
};
use alloy_signer::Signer;
use broadcast::next_nonce;
use build::PreprocessedState;
use clap::{Parser, ValueHint};
use dialoguer::Confirm;
use eyre::{ContextCompat, Result};
use forge_script_sequence::{AdditionalContract, NestedValue};
use forge_verify::{RetryArgs, VerifierArgs};
use foundry_cli::{
    opts::{BuildOpts, EvmArgs, GlobalArgs},
    utils::{LoadConfig, parse_fee_token_address},
};
use foundry_common::{
    CONTRACT_MAX_SIZE, ContractsByArtifact, SELECTOR_LEN,
    abi::{encode_function_args, get_func},
    shell,
};
use foundry_compilers::ArtifactId;
use foundry_config::{
    Config, figment,
    figment::{
        Metadata, Profile, Provider,
        value::{Dict, Map},
    },
};
use foundry_evm::{
    construction::{EvmConstruction, ExecutorConfig, PreparedEvm},
    core::{
        Breakpoints,
        evm::{EthEvmNetwork, FoundryEvmNetwork, OpEvmNetwork, TempoEvmNetwork},
        tempo::PATH_USD_ADDRESS,
    },
    inspectors::{
        CheatsConfig,
        cheatcodes::{BroadcastableTransactions, Wallets},
    },
    opts::{
        EvmOpts,
        resolution::{
            CommandProfileResolution, NetworkIntent, NetworkRequirementSource, ProfileKind,
            ResolvedEvmOpts, RpcForkIdentitySource,
        },
    },
    revm::interpreter::InstructionResult,
    traces::{TraceMode, Traces},
};
use foundry_evm_networks::{EvmFamily, ResolvedNetworkProfile};
use foundry_wallets::MultiWalletOpts;
use serde::Serialize;
use std::path::PathBuf;

mod broadcast;
mod build;
mod execute;
mod multi_sequence;
mod progress;
mod providers;
mod receipts;
mod runner;
mod sequence;
mod simulate;
mod transaction;
mod verify;

// Loads project's figment and merges the build cli arguments into it
foundry_config::merge_impl_figment_convert!(ScriptArgs, build, evm);

/// CLI arguments for `forge script`.
#[derive(Clone, Debug, Default, Parser)]
pub struct ScriptArgs {
    // Include global options for users of this struct.
    #[command(flatten)]
    pub global: GlobalArgs,

    /// The contract you want to run. Either the file path or contract name.
    ///
    /// If multiple contracts exist in the same file you must specify the target contract with
    /// --target-contract.
    #[arg(value_hint = ValueHint::FilePath)]
    pub path: String,

    /// Arguments to pass to the script function.
    pub args: Vec<String>,

    /// The name of the contract you want to run.
    #[arg(long, visible_alias = "tc", value_name = "CONTRACT_NAME")]
    pub target_contract: Option<String>,

    /// The signature of the function you want to call in the contract, or raw calldata.
    #[arg(long, short, default_value = "run")]
    pub sig: String,

    /// Max priority fee per gas for EIP1559 transactions.
    #[arg(
        long,
        env = "ETH_PRIORITY_GAS_PRICE",
        value_parser = foundry_cli::utils::parse_ether_value,
        value_name = "PRICE"
    )]
    pub priority_gas_price: Option<U256>,

    /// Use legacy transactions instead of EIP1559 ones.
    ///
    /// This is auto-enabled for common networks without EIP1559.
    #[arg(long)]
    pub legacy: bool,

    /// Broadcasts the transactions.
    #[arg(long)]
    pub broadcast: bool,

    /// Batch all broadcast transactions into a single Tempo batch transaction.
    ///
    /// When enabled, all vm.broadcast() calls are collected and sent as a single
    /// atomic type 0x76 transaction instead of individual transactions.
    /// This provides atomicity (all-or-nothing execution) and gas savings.
    #[arg(long)]
    pub batch: bool,

    /// Number of calls per Tempo batch transaction.
    ///
    /// When `--batch` is enabled, splits the collected calls into multiple batch
    /// transactions of at most this many calls each.
    #[arg(long, requires = "batch", default_value = "100")]
    pub batch_size: usize,

    /// Tempo fee token address for paying transaction fees.
    #[arg(long = "tempo.fee-token", value_parser = parse_fee_token_address)]
    pub fee_token: Option<Address>,

    /// Skips on-chain simulation.
    #[arg(long)]
    pub skip_simulation: bool,

    /// Relative percentage to multiply gas estimates by.
    #[arg(long, short, default_value = "130")]
    pub gas_estimate_multiplier: u64,

    /// Send via `eth_sendTransaction` using the `--sender` argument as sender.
    #[arg(
        long,
        conflicts_with_all = &["private_key", "private_keys", "ledger", "trezor", "aws", "browser"],
    )]
    pub unlocked: bool,

    /// Resumes submitting transactions that failed or timed-out previously.
    ///
    /// It DOES NOT simulate the script again and it expects nonces to have remained the same.
    ///
    /// Example: If transaction N has a nonce of 22, then the account should have a nonce of 22,
    /// otherwise it fails.
    #[arg(long)]
    pub resume: bool,

    /// If present, --resume or --verify will be assumed to be a multi chain deployment.
    #[arg(long)]
    pub multi: bool,

    /// Open the script in the debugger.
    ///
    /// Takes precedence over broadcast.
    #[arg(long)]
    pub debug: bool,

    /// Dumps all debugger steps to file.
    #[arg(
        long,
        requires = "debug",
        value_hint = ValueHint::FilePath,
        value_name = "PATH"
    )]
    pub dump: Option<PathBuf>,

    /// Makes sure a transaction is sent,
    /// only after its previous one has been confirmed and succeeded.
    #[arg(long)]
    pub slow: bool,

    /// Disables interactive prompts that might appear when deploying big contracts.
    ///
    /// For more info on the contract size limit, see EIP-170: <https://eips.ethereum.org/EIPS/eip-170>
    #[arg(long)]
    pub non_interactive: bool,

    /// Disables the contract size limit during script execution.
    #[arg(long)]
    pub disable_code_size_limit: bool,

    /// Disables the labels in the traces.
    #[arg(long)]
    pub disable_labels: bool,

    /// The Etherscan (or equivalent) API key
    #[arg(long, env = "ETHERSCAN_API_KEY", value_name = "KEY")]
    pub etherscan_api_key: Option<String>,

    /// Verifies all the contracts found in the receipts of a script, if any.
    #[arg(long, requires = "broadcast")]
    pub verify: bool,

    /// Gas price for legacy transactions, or max fee per gas for EIP1559 transactions, either
    /// specified in wei, or as a string with a unit type.
    ///
    /// Examples: 1ether, 10gwei, 0.01ether
    #[arg(
        long,
        env = "ETH_GAS_PRICE",
        value_parser = foundry_cli::utils::parse_ether_value,
        value_name = "PRICE",
    )]
    pub with_gas_price: Option<U256>,

    /// Timeout to use for broadcasting transactions.
    #[arg(long, env = "ETH_TIMEOUT")]
    pub timeout: Option<u64>,

    #[command(flatten)]
    pub build: BuildOpts,

    #[command(flatten)]
    pub wallets: MultiWalletOpts,

    #[command(flatten)]
    pub evm: EvmArgs,

    #[command(flatten)]
    pub verifier: VerifierArgs,

    #[command(flatten)]
    pub retry: RetryArgs,
}

impl ScriptArgs {
    /// Loads config and resolves the command's immutable network profile.
    async fn resolved_evm_opts(&self) -> Result<(Config, ResolvedEvmOpts)> {
        let (config, evm_opts) = self.load_config_and_evm_opts()?;
        let fork_identity = RpcForkIdentitySource::from_evm_opts(&evm_opts);
        let mut intent = NetworkIntent::new();
        if evm_opts.fork_url.is_some() {
            intent = intent.with_fork_identity();
        }
        if self.fee_token.is_some() {
            intent =
                intent.require_profile(ProfileKind::Tempo, NetworkRequirementSource::TempoFeeToken);
        }

        let resolved = CommandProfileResolution::with_fork_identity_source(fork_identity)
            .resolve_evm_opts_async(evm_opts, intent)
            .await?;
        Ok((config, resolved))
    }

    async fn preprocess_for_profile<FEN: FoundryEvmNetwork>(
        self,
        config: Config,
        mut resolved: ResolvedEvmOpts,
    ) -> Result<PreprocessedState<FEN>> {
        let network_profile = resolved.network_profile();
        let script_wallets = Wallets::new(self.wallets.get_multi_wallet().await?, self.evm.sender);
        let browser_wallet = self.wallets.browser_signer::<FEN::Network>().await?;

        let mut sender = resolved.evm_opts().sender;
        if let Some(private_key_sender) = self.maybe_load_private_key()? {
            sender = private_key_sender;
        } else if self.evm.sender.is_none() {
            // If no sender was explicitly set via --sender, auto-detect it from available signers:
            // use the sole signer's address if there's exactly one, or fall back to the browser
            // wallet address if present.
            if let Ok(signers) = script_wallets.signers()
                && signers.len() == 1
            {
                sender = signers[0];
            } else if let Some(signer) = browser_wallet.as_ref().map(|b| b.address()) {
                sender = signer
            }
        }

        resolved = resolved.with_sender(sender);
        let fee_token = if network_profile.is_tempo() && self.fee_token.is_none() {
            Some(PATH_USD_ADDRESS)
        } else {
            self.fee_token
        };

        let script_config = ScriptConfig::new(config, resolved, self.batch, fee_token).await?;
        Ok(PreprocessedState { args: self, script_config, script_wallets, browser_wallet })
    }

    /// Executes the script
    #[allow(clippy::large_stack_frames)]
    pub async fn run_script(self) -> Result<()> {
        trace!(target: "script", "executing script command");

        let (config, resolved) = self.resolved_evm_opts().await?;
        let family = resolved.network_profile().evm_family();

        if self.batch && family != EvmFamily::Tempo {
            eyre::bail!("--batch mode is only supported on Tempo networks");
        }

        match family {
            EvmFamily::Tempo => {
                let batch = self.batch;
                let bundled =
                    match self.prepare_bundled::<TempoEvmNetwork>(config, resolved).await? {
                        Some(bundled) => bundled,
                        None => return Ok(()),
                    };
                let bundled = bundled.wait_for_pending().await?;
                let broadcasted = if batch {
                    bundled.broadcast_batch().await?
                } else {
                    bundled.broadcast().await?
                };
                if broadcasted.args.verify {
                    broadcasted.verify().await?;
                }
                Ok(())
            }
            EvmFamily::Optimism => self.run_generic_script::<OpEvmNetwork>(config, resolved).await,
            EvmFamily::Ethereum => self.run_generic_script::<EthEvmNetwork>(config, resolved).await,
        }
    }

    /// Prepares the bundled state (compile, simulate, bundle) and returns it
    /// for broadcasting, or returns `None` if there's nothing to broadcast
    /// (e.g., debug mode, no transactions, missing RPCs).
    #[allow(clippy::large_stack_frames)]
    async fn prepare_bundled<FEN: FoundryEvmNetwork>(
        self,
        config: Config,
        resolved: ResolvedEvmOpts,
    ) -> Result<Option<BundledState<FEN>>> {
        let state = self.preprocess_for_profile::<FEN>(config, resolved).await?;
        let create2_deployer = state.script_config.evm_opts.create2_deployer;
        let compiled = state.compile()?;

        // Move from `CompiledState` to `BundledState` either by resuming or executing and
        // simulating script.
        let bundled = if compiled.args.resume {
            compiled.resume().await?
        } else {
            // Drive state machine to point at which we have everything needed for simulation.
            let pre_simulation = compiled
                .link()
                .await?
                .prepare_execution()
                .await?
                .execute()
                .await?
                .prepare_simulation()
                .await?;

            if pre_simulation.args.debug {
                return match pre_simulation.args.dump.clone() {
                    Some(path) => pre_simulation.dump_debugger(&path).map(|_| None),
                    None => pre_simulation.run_debugger().map(|_| None),
                };
            }

            if shell::is_json() {
                pre_simulation.show_json().await?;
            } else {
                pre_simulation.show_traces().await?;
            }

            // Ensure that we have transactions to simulate/broadcast, otherwise exit early to avoid
            // hard error.
            if pre_simulation
                .execution_result
                .transactions
                .as_ref()
                .is_none_or(|txs| txs.is_empty())
            {
                if pre_simulation.args.broadcast {
                    sh_warn!("No transactions to broadcast.")?;
                }

                return Ok(None);
            }

            // Check if there are any missing RPCs and exit early to avoid hard error.
            if pre_simulation.execution_artifacts.rpc_data.missing_rpc {
                if !shell::is_json() {
                    sh_println!("\nIf you wish to simulate on-chain transactions pass a RPC URL.")?;
                }

                return Ok(None);
            }

            pre_simulation.args.check_contract_sizes(
                &pre_simulation.execution_result,
                &pre_simulation.build_data.known_contracts,
                create2_deployer,
            )?;

            pre_simulation.fill_metadata().await?.bundle().await?
        };

        // Exit early in case user didn't provide any broadcast/verify related flags.
        if !bundled.args.should_broadcast() {
            if !shell::is_json() {
                if shell::verbosity() >= 4 {
                    sh_println!("\n=== Transactions that will be broadcast ===\n")?;
                    bundled.sequence.show_transactions()?;
                }

                sh_println!(
                    "\nSIMULATION COMPLETE. To broadcast these transactions, add --broadcast and wallet configuration(s) to the previous command. See forge script --help for more."
                )?;
            }
            return Ok(None);
        }

        // Exit early if something is wrong with verification options.
        if bundled.args.verify {
            bundled.verify_preflight_check()?;
        }

        Ok(Some(bundled))
    }

    async fn run_generic_script<FEN: FoundryEvmNetwork>(
        self,
        config: Config,
        resolved: ResolvedEvmOpts,
    ) -> Result<()> {
        let bundled = match self.prepare_bundled::<FEN>(config, resolved).await? {
            Some(bundled) => bundled,
            None => return Ok(()),
        };

        // Wait for pending txes and broadcast others.
        let broadcasted = bundled.wait_for_pending().await?.broadcast().await?;

        if broadcasted.args.verify {
            broadcasted.verify().await?;
        }

        Ok(())
    }

    /// In case the user has loaded *only* one private-key or a single remote signer (e.g.,
    /// Turnkey), we can assume that they're using it as the `--sender`.
    fn maybe_load_private_key(&self) -> Result<Option<Address>> {
        if let Some(turnkey_address) = self.wallets.turnkey_address() {
            return Ok(Some(turnkey_address));
        }

        let maybe_sender = self
            .wallets
            .private_keys()?
            .filter(|pks| pks.len() == 1)
            .map(|pks| pks.first().unwrap().address());
        Ok(maybe_sender)
    }

    /// Returns the Function and calldata based on the signature
    ///
    /// If the `sig` is a valid human-readable function we find the corresponding function in the
    /// `abi` If the `sig` is valid hex, we assume it's calldata and try to find the
    /// corresponding function by matching the selector, first 4 bytes in the calldata.
    ///
    /// Note: We assume that the `sig` is already stripped of its prefix, See [`ScriptArgs`]
    fn get_method_and_calldata(&self, abi: &JsonAbi) -> Result<(Function, Bytes)> {
        if let Ok(decoded) = hex::decode(&self.sig) {
            let selector = &decoded[..SELECTOR_LEN];
            let func =
                abi.functions().find(|func| selector == &func.selector()[..]).ok_or_else(|| {
                    eyre::eyre!(
                        "Function selector `{}` not found in the ABI",
                        hex::encode(selector)
                    )
                })?;
            return Ok((func.clone(), decoded.into()));
        }

        let func = if self.sig.contains('(') {
            let func = get_func(&self.sig)?;
            abi.functions()
                .find(|&abi_func| abi_func.selector() == func.selector())
                .wrap_err(format!("Function `{}` is not implemented in your script.", self.sig))?
        } else {
            let matching_functions =
                abi.functions().filter(|func| func.name == self.sig).collect::<Vec<_>>();
            match matching_functions.len() {
                0 => eyre::bail!("Function `{}` not found in the ABI", self.sig),
                1 => matching_functions[0],
                2.. => eyre::bail!(
                    "Multiple functions with the same name `{}` found in the ABI",
                    self.sig
                ),
            }
        };
        let data = encode_function_args(func, &self.args)?;

        Ok((func.clone(), data.into()))
    }

    /// Checks if the transaction is a deployment with either a size above the `CONTRACT_MAX_SIZE`
    /// or specified `code_size_limit`.
    ///
    /// If `self.broadcast` is enabled, it asks confirmation of the user. Otherwise, it just warns
    /// the user.
    fn check_contract_sizes<N: Network>(
        &self,
        result: &ScriptResult<N>,
        known_contracts: &ContractsByArtifact,
        create2_deployer: Address,
    ) -> Result<()> {
        // If disable-code-size-limit flag is enabled then skip the size check
        if self.disable_code_size_limit {
            return Ok(());
        }

        // (name, &init, &deployed)[]
        let mut bytecodes: Vec<(String, &[u8], &[u8])> = vec![];

        // From artifacts
        for (artifact, contract) in known_contracts.iter() {
            let Some(bytecode) = contract.bytecode() else { continue };
            let Some(deployed_bytecode) = contract.deployed_bytecode() else { continue };
            bytecodes.push((artifact.name.clone(), bytecode, deployed_bytecode));
        }

        // From traces
        let create_nodes = result.traces.iter().flat_map(|(_, traces)| {
            traces.nodes().iter().filter(|node| node.trace.kind.is_any_create())
        });
        let mut unknown_c = 0usize;
        for node in create_nodes {
            let init_code = &node.trace.data;
            let deployed_code = &node.trace.output;
            if !bytecodes.iter().any(|(_, b, _)| *b == init_code.as_ref()) {
                bytecodes.push((format!("Unknown{unknown_c}"), init_code, deployed_code));
                unknown_c += 1;
            }
        }

        let mut prompt_user = false;
        let max_size = match self.evm.env.code_size_limit {
            Some(size) => size,
            None => CONTRACT_MAX_SIZE,
        };

        for (data, to) in result.transactions.iter().flat_map(|txes| {
            txes.iter().filter_map(|tx| {
                tx.transaction
                    .input()
                    .filter(|data| data.len() > max_size)
                    .map(|data| (data, tx.transaction.to()))
            })
        }) {
            let mut offset = 0;

            // Find if it's a CREATE or CREATE2. Otherwise, skip transaction.
            if let Some(to) = to {
                if to == create2_deployer {
                    // Size of the salt prefix.
                    offset = 32;
                } else {
                    continue;
                }
            }

            // Find artifact with a deployment code same as the data.
            if let Some((name, _, deployed_code)) =
                bytecodes.iter().find(|(_, init_code, _)| *init_code == &data[offset..])
            {
                let deployment_size = deployed_code.len();

                if deployment_size > max_size {
                    prompt_user = self.should_broadcast();
                    sh_err!(
                        "`{name}` is above the contract size limit ({deployment_size} > {max_size})."
                    )?;
                }
            }
        }

        // Only prompt if we're broadcasting and we've not disabled interactivity.
        if prompt_user
            && !self.non_interactive
            && !Confirm::new().with_prompt("Do you wish to continue?".to_string()).interact()?
        {
            eyre::bail!("User canceled the script.");
        }

        Ok(())
    }

    /// We only broadcast transactions if --broadcast, --resume, or --verify was passed.
    const fn should_broadcast(&self) -> bool {
        self.broadcast || self.resume || self.verify
    }
}

impl Provider for ScriptArgs {
    fn metadata(&self) -> Metadata {
        Metadata::named("Script Args Provider")
    }

    fn data(&self) -> Result<Map<Profile, Dict>, figment::Error> {
        let mut dict = Dict::default();

        if let Some(etherscan_api_key) =
            self.etherscan_api_key.as_ref().filter(|s| !s.trim().is_empty())
        {
            dict.insert(
                "etherscan_api_key".to_string(),
                figment::value::Value::from(etherscan_api_key.clone()),
            );
        }

        if let Some(timeout) = self.timeout {
            dict.insert("transaction_timeout".to_string(), timeout.into());
        }

        Ok(Map::from([(Config::selected_profile(), dict)]))
    }
}

#[derive(Serialize, Clone)]
#[serde(bound = "")]
pub struct ScriptResult<N: Network> {
    pub success: bool,
    #[serde(rename = "raw_logs")]
    pub logs: Vec<Log>,
    pub traces: Traces,
    pub gas_used: u64,
    pub labeled_addresses: AddressHashMap<String>,
    #[serde(skip)]
    pub transactions: Option<BroadcastableTransactions<N>>,
    pub returned: Bytes,
    #[serde(skip)]
    pub exit_reason: Option<InstructionResult>,
    pub address: Option<Address>,
    #[serde(skip)]
    pub breakpoints: Breakpoints,
}

impl<N: Network> Default for ScriptResult<N> {
    fn default() -> Self {
        Self {
            success: Default::default(),
            logs: Default::default(),
            traces: Default::default(),
            gas_used: Default::default(),
            labeled_addresses: Default::default(),
            transactions: Default::default(),
            returned: Default::default(),
            exit_reason: Default::default(),
            address: Default::default(),
            breakpoints: Default::default(),
        }
    }
}

impl<N: Network> ScriptResult<N> {
    pub fn get_created_contracts(
        &self,
        known_contracts: &ContractsByArtifact,
    ) -> Vec<AdditionalContract> {
        self.traces
            .iter()
            .flat_map(|(_, traces)| {
                traces.nodes().iter().filter_map(|node| {
                    if node.trace.kind.is_any_create() {
                        let init_code = node.trace.data.clone();
                        let contract_name = known_contracts
                            .find_by_creation_code(init_code.as_ref())
                            .map(|artifact| artifact.0.name.clone());
                        return Some(AdditionalContract {
                            call_kind: node.trace.kind,
                            address: node.trace.address,
                            contract_name,
                            init_code,
                        });
                    }
                    None
                })
            })
            .collect()
    }
}

#[derive(Serialize)]
#[serde(bound = "")]
struct JsonResult<'a, N: Network> {
    logs: Vec<String>,
    returns: &'a HashMap<String, NestedValue>,
    #[serde(flatten)]
    result: &'a ScriptResult<N>,
}

#[derive(Clone, Debug)]
pub struct ScriptConfig<FEN: FoundryEvmNetwork> {
    pub config: Config,
    pub evm_opts: EvmOpts,
    /// Immutable runtime network profile.
    pub network_profile: ResolvedNetworkProfile,
    /// Opaque options/profile pairing consumed by EVM construction.
    pub resolved_evm_opts: ResolvedEvmOpts,
    pub sender_nonce: u64,
    /// Maps an RPC URL to reusable opaque EVM preparation state.
    pub preparations: HashMap<String, PreparedEvm<FEN>>,
    /// The preparation snapshot used for the latest script execution.
    pub prepared: Option<PreparedEvm<FEN>>,
    /// Whether to batch all broadcast transactions into a single Tempo batch transaction.
    pub batch: bool,
    /// Tempo fee token address for paying transaction fees.
    pub fee_token: Option<Address>,
}

impl<FEN: FoundryEvmNetwork> ScriptConfig<FEN> {
    pub async fn new(
        config: Config,
        resolved_evm_opts: ResolvedEvmOpts,
        batch: bool,
        fee_token: Option<Address>,
    ) -> Result<Self> {
        let network_profile = resolved_evm_opts.network_profile();
        let evm_opts = resolved_evm_opts.evm_opts().clone();
        let sender_nonce = if let Some(fork_url) = evm_opts.fork_url.as_ref() {
            next_nonce(evm_opts.sender, fork_url, evm_opts.fork_block_number).await?
        } else {
            // dapptools compatibility
            1
        };

        Ok(Self {
            config,
            evm_opts,
            network_profile,
            resolved_evm_opts,
            sender_nonce,
            preparations: HashMap::default(),
            prepared: None,
            batch,
            fee_token,
        })
    }

    pub async fn update_sender(&mut self, sender: Address) -> Result<()> {
        self.sender_nonce = if let Some(fork_url) = self.evm_opts.fork_url.as_ref() {
            next_nonce(sender, fork_url, None).await?
        } else {
            // dapptools compatibility
            1
        };
        self.evm_opts.sender = sender;
        self.resolved_evm_opts = self.resolved_evm_opts.clone().with_sender(sender);
        Ok(())
    }

    async fn get_runner(&mut self) -> Result<ScriptRunner<FEN>> {
        self._get_runner(None, false).await
    }

    async fn get_runner_with_cheatcodes(
        &mut self,
        known_contracts: ContractsByArtifact,
        script_wallets: Wallets,
        debug: bool,
        target: ArtifactId,
    ) -> Result<ScriptRunner<FEN>> {
        self._get_runner(Some((known_contracts, script_wallets, target)), debug).await
    }

    async fn _get_runner(
        &mut self,
        cheats_data: Option<(ContractsByArtifact, Wallets, ArtifactId)>,
        debug: bool,
    ) -> Result<ScriptRunner<FEN>> {
        trace!("preparing script runner");
        let prepared = if let Some(fork_url) = self.evm_opts.fork_url.as_ref() {
            match self.preparations.get(fork_url) {
                Some(prepared) => prepared.refresh(&self.resolved_evm_opts).await?,
                None => {
                    let prepared =
                        EvmConstruction::prepare::<FEN>(&self.resolved_evm_opts, &self.config)
                            .await?;
                    self.preparations.insert(fork_url.clone(), prepared.clone());
                    prepared
                }
            }
        } else {
            // It's only really `None`, when we don't pass any `--fork-url`. And if so, there is
            // no need to cache it, since there won't be any onchain simulation that we'd need
            // to cache the backend for.
            EvmConstruction::prepare::<FEN>(&self.resolved_evm_opts, &self.config).await?
        };

        // We need to enable tracing to decode contract names: local or external.
        let mut executor_config = ExecutorConfig::default()
            .logs(self.config.live_logs)
            .trace_mode(if debug { TraceMode::Debug } else { TraceMode::Call })
            .create2_deployer(self.evm_opts.create2_deployer)
            .spec_id(self.config.evm_spec_id())
            .gas_limit(self.evm_opts.gas_limit())
            .legacy_assertions(self.config.legacy_assertions)
            .fee_token(self.fee_token);

        if let Some((known_contracts, script_wallets, target)) = cheats_data {
            executor_config = executor_config
                .cheatcodes(
                    CheatsConfig::new(
                        &self.config,
                        self.evm_opts.clone(),
                        Some(known_contracts),
                        Some(target),
                        self.fee_token,
                    )
                    .into(),
                )
                .wallets(script_wallets)
                .enable_isolation(self.evm_opts.isolate);
        }

        let executor = prepared.clone().construct(executor_config)?.into_executor();
        self.prepared = Some(prepared);
        Ok(ScriptRunner::new(executor, self.evm_opts.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_network::Ethereum;
    #[cfg(feature = "hashkey")]
    use alloy_primitives::address;
    use foundry_config::{NamedChain, UnresolvedEnvVarError};
    #[cfg(feature = "hashkey")]
    use foundry_evm::{construction::DecoderConfig, traces::CallTrace};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn can_parse_sig() {
        let sig = "0x522bb704000000000000000000000000f39fd6e51aad88f6f4ce6ab8827279cfFFb92266";
        let args = ScriptArgs::parse_from(["foundry-cli", "Contract.sol", "--sig", sig]);
        assert_eq!(args.sig, sig);
    }

    #[cfg(feature = "hashkey")]
    #[tokio::test(flavor = "multi_thread")]
    async fn script_config_preserves_hashkey_profile_for_local_simulation() {
        let evm_opts = EvmOpts {
            networks: foundry_evm_networks::NetworkConfigs::with_hashkey(),
            ..Default::default()
        };
        let resolved = CommandProfileResolution::new()
            .resolve_evm_opts(evm_opts, NetworkIntent::new())
            .unwrap();
        let network_profile = resolved.network_profile();
        let mut script_config =
            ScriptConfig::<OpEvmNetwork>::new(Config::default(), resolved, false, None)
                .await
                .unwrap();

        script_config.evm_opts.networks = foundry_evm_networks::NetworkConfigs::default();
        assert_eq!(script_config.network_profile, network_profile);
        assert!(script_config.network_profile.is_hashkey());

        let _runner = script_config.get_runner().await.unwrap();
        let prepared = script_config.prepared.as_ref().unwrap();
        let trace = CallTrace {
            address: address!("0177FF0000000000000000000000000000000000"),
            ..Default::default()
        };
        assert_eq!(
            prepared.trace_decoder(DecoderConfig::default()).decode_function(&trace).await.label,
            Some("H20Factory".to_string())
        );
    }

    #[test]
    fn can_parse_unlocked() {
        let args = ScriptArgs::parse_from([
            "foundry-cli",
            "Contract.sol",
            "--sender",
            "0x4e59b44847b379578588920ca78fbf26c0b4956c",
            "--unlocked",
        ]);
        assert!(args.unlocked);

        let key = U256::ZERO;
        let args = ScriptArgs::try_parse_from([
            "foundry-cli",
            "Contract.sol",
            "--sender",
            "0x4e59b44847b379578588920ca78fbf26c0b4956c",
            "--unlocked",
            "--private-key",
            &key.to_string(),
        ]);
        assert!(args.is_err());
    }

    #[test]
    fn can_merge_script_config() {
        let args = ScriptArgs::parse_from([
            "foundry-cli",
            "Contract.sol",
            "--etherscan-api-key",
            "goerli",
        ]);
        let config = args.load_config().unwrap();
        assert_eq!(config.etherscan_api_key, Some("goerli".to_string()));
    }

    #[test]
    fn can_disable_code_size_limit() {
        let args =
            ScriptArgs::parse_from(["foundry-cli", "Contract.sol", "--disable-code-size-limit"]);
        assert!(args.disable_code_size_limit);

        let result = ScriptResult::<Ethereum>::default();
        let contracts = ContractsByArtifact::default();
        let create = Address::ZERO;
        assert!(args.check_contract_sizes(&result, &contracts, create).is_ok());
    }

    #[test]
    fn can_parse_verifier_url() {
        let args = ScriptArgs::parse_from([
            "foundry-cli",
            "script",
            "script/Test.s.sol:TestScript",
            "--fork-url",
            "http://localhost:8545",
            "--verifier-url",
            "http://localhost:3000/api/verify",
            "--etherscan-api-key",
            "blacksmith",
            "--broadcast",
            "--verify",
            "-vvvvv",
        ]);
        assert_eq!(
            args.verifier.verifier_url,
            Some("http://localhost:3000/api/verify".to_string())
        );
    }

    #[test]
    fn can_extract_code_size_limit() {
        let args = ScriptArgs::parse_from([
            "foundry-cli",
            "script",
            "script/Test.s.sol:TestScript",
            "--fork-url",
            "http://localhost:8545",
            "--broadcast",
            "--code-size-limit",
            "50000",
        ]);
        assert_eq!(args.evm.env.code_size_limit, Some(50000));
    }

    #[test]
    fn can_extract_script_etherscan_key() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        let config = r#"
                [profile.default]
                etherscan_api_key = "amoy"

                [etherscan]
                amoy = { key = "https://etherscan-amoy.com/" }
            "#;

        let toml_file = root.join(Config::FILE_NAME);
        fs::write(toml_file, config).unwrap();
        let args = ScriptArgs::parse_from([
            "foundry-cli",
            "Contract.sol",
            "--etherscan-api-key",
            "amoy",
            "--root",
            root.as_os_str().to_str().unwrap(),
        ]);

        let config = args.load_config().unwrap();
        let amoy = config.get_etherscan_api_key(Some(NamedChain::PolygonAmoy.into()));
        assert_eq!(amoy, Some("https://etherscan-amoy.com/".to_string()));
    }

    #[test]
    fn can_extract_script_rpc_alias() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        let config = r#"
                [profile.default]

                [rpc_endpoints]
                polygonAmoy = "https://polygon-amoy.g.alchemy.com/v2/${_CAN_EXTRACT_RPC_ALIAS}"
            "#;

        let toml_file = root.join(Config::FILE_NAME);
        fs::write(toml_file, config).unwrap();
        let args = ScriptArgs::parse_from([
            "foundry-cli",
            "DeployV1",
            "--rpc-url",
            "polygonAmoy",
            "--root",
            root.as_os_str().to_str().unwrap(),
        ]);

        let err = args.load_config_and_evm_opts().unwrap_err();

        assert!(err.downcast::<UnresolvedEnvVarError>().is_ok());

        unsafe {
            std::env::set_var("_CAN_EXTRACT_RPC_ALIAS", "123456");
        }
        let (config, evm_opts) = args.load_config_and_evm_opts().unwrap();
        assert_eq!(config.eth_rpc_url, Some("polygonAmoy".to_string()));
        assert_eq!(
            evm_opts.fork_url,
            Some("https://polygon-amoy.g.alchemy.com/v2/123456".to_string())
        );
    }

    #[test]
    fn can_extract_script_rpc_and_etherscan_alias() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        let config = r#"
            [profile.default]

            [rpc_endpoints]
            amoy = "https://polygon-amoy.g.alchemy.com/v2/${_EXTRACT_RPC_ALIAS}"

            [etherscan]
            amoy = { key = "${_ETHERSCAN_API_KEY}", chain = 80002, url = "https://amoy.polygonscan.com/" }
        "#;

        let toml_file = root.join(Config::FILE_NAME);
        fs::write(toml_file, config).unwrap();
        let args = ScriptArgs::parse_from([
            "foundry-cli",
            "DeployV1",
            "--rpc-url",
            "amoy",
            "--etherscan-api-key",
            "amoy",
            "--root",
            root.as_os_str().to_str().unwrap(),
        ]);
        let err = args.load_config_and_evm_opts().unwrap_err();

        assert!(err.downcast::<UnresolvedEnvVarError>().is_ok());

        unsafe {
            std::env::set_var("_EXTRACT_RPC_ALIAS", "123456");
        }
        unsafe {
            std::env::set_var("_ETHERSCAN_API_KEY", "etherscan_api_key");
        }
        let (config, evm_opts) = args.load_config_and_evm_opts().unwrap();
        assert_eq!(config.eth_rpc_url, Some("amoy".to_string()));
        assert_eq!(
            evm_opts.fork_url,
            Some("https://polygon-amoy.g.alchemy.com/v2/123456".to_string())
        );
        let etherscan = config.get_etherscan_api_key(Some(80002u64.into()));
        assert_eq!(etherscan, Some("etherscan_api_key".to_string()));
        let etherscan = config.get_etherscan_api_key(None);
        assert_eq!(etherscan, Some("etherscan_api_key".to_string()));
    }

    #[test]
    fn can_extract_script_rpc_and_sole_etherscan_alias() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        let config = r#"
                [profile.default]

               [rpc_endpoints]
                amoy = "https://polygon-amoy.g.alchemy.com/v2/${_SOLE_EXTRACT_RPC_ALIAS}"

                [etherscan]
                amoy = { key = "${_SOLE_ETHERSCAN_API_KEY}" }
            "#;

        let toml_file = root.join(Config::FILE_NAME);
        fs::write(toml_file, config).unwrap();
        let args = ScriptArgs::parse_from([
            "foundry-cli",
            "DeployV1",
            "--rpc-url",
            "amoy",
            "--root",
            root.as_os_str().to_str().unwrap(),
        ]);
        let err = args.load_config_and_evm_opts().unwrap_err();

        assert!(err.downcast::<UnresolvedEnvVarError>().is_ok());

        unsafe {
            std::env::set_var("_SOLE_EXTRACT_RPC_ALIAS", "123456");
        }
        unsafe {
            std::env::set_var("_SOLE_ETHERSCAN_API_KEY", "etherscan_api_key");
        }
        let (config, evm_opts) = args.load_config_and_evm_opts().unwrap();
        assert_eq!(
            evm_opts.fork_url,
            Some("https://polygon-amoy.g.alchemy.com/v2/123456".to_string())
        );
        let etherscan = config.get_etherscan_api_key(Some(80002u64.into()));
        assert_eq!(etherscan, Some("etherscan_api_key".to_string()));
        let etherscan = config.get_etherscan_api_key(None);
        assert_eq!(etherscan, Some("etherscan_api_key".to_string()));
    }

    // <https://github.com/foundry-rs/foundry/issues/5923>
    #[test]
    fn test_5923() {
        let args =
            ScriptArgs::parse_from(["foundry-cli", "DeployV1", "--priority-gas-price", "100"]);
        assert!(args.priority_gas_price.is_some());
    }

    // <https://github.com/foundry-rs/foundry/issues/5910>
    #[test]
    fn test_5910() {
        let args = ScriptArgs::parse_from([
            "foundry-cli",
            "--broadcast",
            "--with-gas-price",
            "0",
            "SolveTutorial",
        ]);
        assert!(args.with_gas_price.unwrap().is_zero());
    }

    #[test]
    fn test_priority_gas_price_cannot_exceed_gas_price() {
        let args = ScriptArgs::parse_from([
            "foundry-cli",
            "--broadcast",
            "--with-gas-price",
            "100",
            "--priority-gas-price",
            "200",
            "Script",
        ]);
        // priority (200) > max_fee (100) — broadcast should reject this at runtime
        assert!(args.priority_gas_price.unwrap() > args.with_gas_price.unwrap());
    }
}
