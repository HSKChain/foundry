use crate::{
    opts::{Chisel, ChiselSubcommand},
    prelude::{ChiselCommand, ChiselDispatcher, SolidityHelper},
};
use clap::Parser;
use eyre::{Context, Result};
use foundry_cli::utils::{self, LoadConfig};
use foundry_common::fs;
use foundry_evm::{
    core::evm::{EthEvmNetwork, FoundryEvmNetwork, OpEvmNetwork, TempoEvmNetwork},
    opts::resolution::{
        CommandProfileResolution, NetworkIntent, ResolvedEvmOpts, RpcForkIdentitySource,
    },
};
use foundry_evm_networks::{EvmFamily, NetworkConfigs, ResolvedNetworkProfile};
use rustyline::{Editor, config::Configurer, error::ReadlineError};
use std::{ops::ControlFlow, path::PathBuf};
use yansi::Paint;

/// Run the `chisel` command line interface.
pub fn run() -> Result<()> {
    setup()?;

    foundry_cli::opts::GlobalArgs::check_markdown_help::<Chisel>();

    let args = Chisel::parse();
    args.global.init()?;
    args.global.tokio_runtime().block_on(run_command(args))
}

/// Setup the global logger and other utilities.
pub fn setup() -> Result<()> {
    utils::common_setup();
    utils::subscriber();

    Ok(())
}

macro_rules! try_cf {
    ($e:expr) => {
        match $e {
            ControlFlow::Continue(()) => {}
            ControlFlow::Break(()) => return Ok(()),
        }
    };
}

/// Builds the canonical network intent from the configured chain identity.
///
/// The chain selection is an identity-bearing hint; it never mutates the EVM network
/// configuration directly.
const fn chisel_network_intent(chain: Option<foundry_config::Chain>) -> NetworkIntent {
    let mut intent = NetworkIntent::new();
    if let Some(chain) = chain {
        intent = intent.with_chain_hint(chain.id());
    }
    intent
}

/// Maps a resolved profile back to the canonical network configuration persisted with sessions.
///
/// The persisted configuration is the identity used by the session-load network guard, so it
/// must reflect the resolved profile rather than the raw pre-resolution options.
fn resolved_network_configs(profile: ResolvedNetworkProfile) -> NetworkConfigs {
    if profile.is_tempo() {
        NetworkConfigs::with_tempo()
    } else if profile.is_celo() {
        NetworkConfigs::with_celo()
    } else {
        #[cfg(feature = "hashkey")]
        if profile.is_hashkey() {
            return NetworkConfigs::with_hashkey();
        }
        if profile.is_optimism() {
            NetworkConfigs::with_optimism()
        } else {
            NetworkConfigs::default()
        }
    }
}

/// Run the subcommand.
pub async fn run_command(args: Chisel) -> Result<()> {
    // Load configuration
    let (mut config, evm_opts) = args.load_config_and_evm_opts()?;
    let configured_networks = evm_opts.networks;

    // Resolve the command network profile once; the opaque carrier is preserved through
    // session creation, load, and rebuild without repeating chain or fork inference.
    let fork_identity = RpcForkIdentitySource::from_evm_opts(&evm_opts);
    let resolved = CommandProfileResolution::with_fork_identity_source(fork_identity)
        .resolve_evm_opts_async(evm_opts, chisel_network_intent(config.chain))
        .await?;
    // Persist the resolved network identity so session save/load can detect mismatches;
    // an explicit user selection is preserved verbatim.
    config.networks = if configured_networks.has_explicit_selection() {
        configured_networks
    } else {
        resolved_network_configs(resolved.network_profile())
    };

    match resolved.network_profile().evm_family() {
        EvmFamily::Ethereum => {
            run_command_with_network::<EthEvmNetwork>(args, config, resolved).await
        }
        EvmFamily::Optimism => {
            run_command_with_network::<OpEvmNetwork>(args, config, resolved).await
        }
        EvmFamily::Tempo => {
            run_command_with_network::<TempoEvmNetwork>(args, config, resolved).await
        }
    }
}

async fn run_command_with_network<FEN: FoundryEvmNetwork>(
    args: Chisel,
    config: foundry_config::Config,
    resolved: ResolvedEvmOpts,
) -> Result<()> {
    // Create a new cli dispatcher
    let mut dispatcher = ChiselDispatcher::<FEN>::new(crate::source::SessionSourceConfig {
        // Enable traces if any level of verbosity was passed
        traces: config.verbosity > 0,
        foundry_config: config,
        no_vm: args.no_vm,
        evm_opts: resolved.evm_opts().clone(),
        network_profile: resolved.network_profile(),
        resolved_evm_opts: Some(resolved),
        state: None,
        calldata: None,
        ir_minimum: args.ir_minimum,
    })?;

    // Execute prelude Solidity source files
    evaluate_prelude(&mut dispatcher, args.prelude).await?;

    if let Some(cmd) = args.cmd {
        try_cf!(handle_cli_command(&mut dispatcher, cmd).await?);
        return Ok(());
    }

    let mut rl = Editor::<SolidityHelper, _>::new()?;
    rl.set_helper(Some(dispatcher.helper.clone()));
    rl.set_auto_add_history(true);
    if let Some(path) = chisel_history_file() {
        let _ = rl.load_history(&path);
    }

    sh_println!("Welcome to Chisel! Type `{}` to show available commands.", "!help".green())?;

    // REPL loop.
    let mut interrupt = false;
    loop {
        match rl.readline(&dispatcher.get_prompt()) {
            Ok(line) => {
                debug!("dispatching next line: {line}");
                // Clear interrupt flag.
                interrupt = false;

                // Dispatch and match results.
                let r = dispatcher.dispatch(&line).await;
                dispatcher.helper.set_errored(r.is_err());
                match r {
                    Ok(ControlFlow::Continue(())) => {}
                    Ok(ControlFlow::Break(())) => break,
                    Err(e) => {
                        sh_err!("{}", foundry_common::errors::display_chain(&e))?;
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                if interrupt {
                    break;
                }
                sh_println!("(To exit, press Ctrl+C again)")?;
                interrupt = true;
            }
            Err(ReadlineError::Eof) => break,
            Err(err) => {
                sh_err!("{err}")?;
                break;
            }
        }
    }

    if let Some(path) = chisel_history_file() {
        let _ = rl.save_history(&path);
    }

    Ok(())
}

/// Evaluate multiple Solidity source files contained within a
/// Chisel prelude directory.
async fn evaluate_prelude(
    dispatcher: &mut ChiselDispatcher<impl FoundryEvmNetwork>,
    maybe_prelude: Option<PathBuf>,
) -> Result<()> {
    let Some(prelude_dir) = maybe_prelude else { return Ok(()) };
    if prelude_dir.is_file() {
        sh_println!("{} {}", "Loading prelude source file:".yellow(), prelude_dir.display())?;
        try_cf!(load_prelude_file(dispatcher, prelude_dir).await?);
        sh_println!("{}\n", "Prelude source file loaded successfully!".green())?;
    } else {
        let prelude_sources = fs::files_with_ext(&prelude_dir, "sol");
        let mut print_success_msg = false;
        for source_file in prelude_sources {
            print_success_msg = true;
            sh_println!("{} {}", "Loading prelude source file:".yellow(), source_file.display())?;
            try_cf!(load_prelude_file(dispatcher, source_file).await?);
        }

        if print_success_msg {
            sh_println!("{}\n", "All prelude source files loaded successfully!".green())?;
        }
    }
    Ok(())
}

/// Loads a single Solidity file into the prelude.
async fn load_prelude_file(
    dispatcher: &mut ChiselDispatcher<impl FoundryEvmNetwork>,
    file: PathBuf,
) -> Result<ControlFlow<()>> {
    let prelude = fs::read_to_string(file)
        .wrap_err("Could not load source file. Are you sure this path is correct?")?;
    dispatcher.dispatch(&prelude).await
}

async fn handle_cli_command(
    d: &mut ChiselDispatcher<impl FoundryEvmNetwork>,
    cmd: ChiselSubcommand,
) -> Result<ControlFlow<()>> {
    match cmd {
        ChiselSubcommand::List => d.dispatch_command(ChiselCommand::ListSessions).await,
        ChiselSubcommand::Load { id } => d.dispatch_command(ChiselCommand::Load { id }).await,
        ChiselSubcommand::View { id } => {
            let ControlFlow::Continue(()) = d.dispatch_command(ChiselCommand::Load { id }).await?
            else {
                return Ok(ControlFlow::Break(()));
            };
            d.dispatch_command(ChiselCommand::Source).await
        }
        ChiselSubcommand::ClearCache => d.dispatch_command(ChiselCommand::ClearCache).await,
        ChiselSubcommand::Eval { command } => d.dispatch(&command).await,
    }
}

fn chisel_history_file() -> Option<PathBuf> {
    foundry_config::Config::foundry_dir().map(|p| p.join(".chisel_history"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use foundry_evm::opts::{
        EvmOpts,
        resolution::{ForkIdentity, InMemoryForkIdentitySource},
    };

    #[test]
    fn resolved_network_configs_preserve_session_identity() {
        assert_eq!(
            resolved_network_configs(NetworkConfigs::default().resolve()),
            NetworkConfigs::default()
        );
        assert_eq!(
            resolved_network_configs(NetworkConfigs::with_tempo().resolve()),
            NetworkConfigs::with_tempo()
        );
        assert_eq!(
            resolved_network_configs(NetworkConfigs::with_celo().resolve()),
            NetworkConfigs::with_celo()
        );
        assert_eq!(
            resolved_network_configs(NetworkConfigs::with_optimism().resolve()),
            NetworkConfigs::with_optimism()
        );
        #[cfg(feature = "hashkey")]
        assert_eq!(
            resolved_network_configs(NetworkConfigs::with_hashkey().resolve()),
            NetworkConfigs::with_hashkey()
        );
    }

    #[test]
    fn configured_tempo_chain_selects_tempo_without_fork_lookup() {
        let mut resolution = CommandProfileResolution::with_fork_identity_source(
            InMemoryForkIdentitySource::unavailable("must not be called"),
        );

        let resolved = resolution
            .resolve_evm_opts(
                EvmOpts::default(),
                chisel_network_intent(Some(foundry_config::Chain::from_id(42431))),
            )
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "tempo");
    }

    #[test]
    fn configured_ethereum_chain_selects_ethereum_without_fork_lookup() {
        let mut resolution = CommandProfileResolution::with_fork_identity_source(
            InMemoryForkIdentitySource::unavailable("must not be called"),
        );

        let resolved = resolution
            .resolve_evm_opts(
                EvmOpts::default(),
                chisel_network_intent(Some(foundry_config::Chain::from_id(1))),
            )
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "ethereum");
    }

    #[test]
    fn no_configured_chain_resolves_ethereum_without_fork_lookup() {
        let mut resolution = CommandProfileResolution::with_fork_identity_source(
            InMemoryForkIdentitySource::unavailable("must not be called"),
        );

        let resolved =
            resolution.resolve_evm_opts(EvmOpts::default(), chisel_network_intent(None)).unwrap();

        assert_eq!(resolved.network_profile().name(), "ethereum");
    }

    #[test]
    fn known_configured_chain_skips_fork_identity_even_with_fork_url() {
        let evm_opts =
            EvmOpts { fork_url: Some("http://localhost:8545".to_string()), ..Default::default() };
        let mut resolution = CommandProfileResolution::with_fork_identity_source(
            InMemoryForkIdentitySource::unavailable("must not be called"),
        );

        let resolved = resolution
            .resolve_evm_opts(
                evm_opts,
                chisel_network_intent(Some(foundry_config::Chain::from_id(42431))),
            )
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "tempo");
    }

    #[test]
    fn unknown_configured_chain_with_fork_url_uses_fork_identity() {
        let evm_opts =
            EvmOpts { fork_url: Some("http://localhost:8545".to_string()), ..Default::default() };
        let mut resolution = CommandProfileResolution::with_fork_identity_source(
            InMemoryForkIdentitySource::new(ForkIdentity::new(42431)),
        );

        let resolved = resolution
            .resolve_evm_opts(
                evm_opts,
                chisel_network_intent(Some(foundry_config::Chain::from_id(999_999))),
            )
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "tempo");
    }

    #[test]
    fn unknown_configured_chain_without_fork_resolves_ethereum() {
        let mut resolution = CommandProfileResolution::with_fork_identity_source(
            InMemoryForkIdentitySource::unavailable("must not be called"),
        );

        let resolved = resolution
            .resolve_evm_opts(
                EvmOpts::default(),
                chisel_network_intent(Some(foundry_config::Chain::from_id(999_999))),
            )
            .unwrap();

        assert_eq!(resolved.network_profile().name(), "ethereum");
    }

    #[test]
    fn verify_cli() {
        Chisel::command().debug_assert();
    }
}
