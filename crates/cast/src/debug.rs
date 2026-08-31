use std::str::FromStr;

use alloy_chains::Chain;
use alloy_primitives::{Address, Bytes, map::HashMap};
use foundry_cli::utils::{TraceResult, print_traces};
use foundry_common::{ContractsByArtifact, compile::ProjectCompiler, shell};
use foundry_config::Config;
use foundry_debugger::Debugger;
use foundry_evm::{
    construction::DecoderConfig,
    core::evm::FoundryEvmNetwork,
    executors::TracingExecutor,
    traces::{
        CallTraceDecoder, DebugTraceIdentifier, Traces,
        debug::ContractSources,
        decode_trace_arena,
        identifier::{SignaturesIdentifier, TraceIdentifiers},
    },
};

async fn decode_debugger_traces(traces: &mut Traces, decoder: &CallTraceDecoder) {
    for (_, trace) in traces {
        decode_trace_arena(trace, decoder).await;
    }
}

/// labels the traces, conditionally prints them or opens the debugger
#[expect(clippy::too_many_arguments)]
pub(crate) async fn handle_traces<FEN: FoundryEvmNetwork>(
    mut result: TraceResult,
    executor: &TracingExecutor<FEN>,
    config: &Config,
    contracts_bytecode: &HashMap<Address, Bytes>,
    labels: Vec<String>,
    with_local_artifacts: bool,
    debug: bool,
    decode_internal: bool,
    disable_label: bool,
    trace_depth: Option<usize>,
) -> eyre::Result<()> {
    let chain = Chain::from_id(executor.chain_id());
    let (known_contracts, mut sources) = if with_local_artifacts {
        let _ = sh_println!("Compiling project to generate artifacts");
        let project = config.project()?;
        let compiler = ProjectCompiler::new();
        let output = compiler.compile(&project)?;
        (
            Some(ContractsByArtifact::new(
                output.artifact_ids().map(|(id, artifact)| (id, artifact.clone().into())),
            )),
            ContractSources::from_project_output(&output, project.root(), None)?,
        )
    } else {
        (None, ContractSources::default())
    };

    let labels = labels.iter().filter_map(|label_str| {
        let mut iter = label_str.split(':');

        if let Some(addr) = iter.next()
            && let (Ok(address), Some(label)) = (Address::from_str(addr), iter.next())
        {
            return Some((address, label.to_string()));
        }
        None
    });
    let config_labels = config.labels.clone().into_iter();

    let mut decoder_config = DecoderConfig::default()
        .labels(labels.chain(config_labels))
        .signature_identifier(SignaturesIdentifier::from_config(config)?)
        .label_disabled(disable_label);
    let mut identifier = TraceIdentifiers::new().with_external(config, Some(chain))?;
    if let Some(contracts) = &known_contracts {
        decoder_config = decoder_config.known_contracts(contracts.clone());
        identifier = identifier.with_local_and_bytecodes(contracts, contracts_bytecode);
    }

    let mut decoder = executor.trace_decoder(decoder_config);

    for (_, trace) in result.traces.as_deref_mut().unwrap_or_default() {
        decoder.identify(trace, &mut identifier);
    }

    if decode_internal || debug {
        if let Some(ref etherscan_identifier) = identifier.external {
            sources.merge(etherscan_identifier.get_compiled_contracts().await?);
        }

        if debug {
            if let Some(traces) = result.traces.as_mut() {
                decode_debugger_traces(traces, &decoder).await;
            }
            let mut debugger = Debugger::builder()
                .traces(result.traces.expect("missing traces"))
                .decoder(&decoder)
                .sources(sources)
                .build();
            debugger.try_run_tui()?;
            return Ok(());
        }

        decoder.debug_identifier = Some(DebugTraceIdentifier::new(sources));
    }

    print_traces(
        &mut result,
        &decoder,
        shell::verbosity() > 0,
        shell::verbosity() > 4,
        trace_depth,
    )
    .await?;

    Ok(())
}

#[cfg(all(test, feature = "hashkey"))]
mod tests {
    use super::*;
    use alloy_dyn_abi::{DynSolValue, JsonAbiExt};
    use alloy_json_abi::Function;
    use alloy_primitives::U256;
    use foundry_evm::{
        construction::EvmConstruction,
        core::evm::OpEvmNetwork,
        opts::{
            EvmOpts,
            resolution::{CommandProfileResolution, NetworkIntent},
        },
        traces::{
            CallTrace, CallTraceArena, CallTraceNode, SparsedTraceArena, TraceKind, TraceMode,
        },
    };
    use foundry_evm_networks::NetworkConfigs;

    #[tokio::test(flavor = "multi_thread")]
    async fn hashkey_debugger_projection_decodes_h20_calls() {
        let mut token = [0u8; 20];
        token[0] = 0xb2;
        token[11..].fill(0x11);
        let token = Address::from(token);
        let recipient = Address::repeat_byte(0x22);
        let mint = Function::parse("mint(address to,uint256 amount)").unwrap();
        let mut arena = CallTraceArena::default();
        arena.nodes_mut()[0] = CallTraceNode {
            trace: CallTrace {
                address: token,
                data: mint
                    .abi_encode_input(&[
                        DynSolValue::Address(recipient),
                        DynSolValue::Uint(U256::from(42), 256),
                    ])
                    .unwrap()
                    .into(),
                success: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut traces =
            vec![(TraceKind::Execution, SparsedTraceArena { arena, ignored: Default::default() })];
        let mut evm_opts = EvmOpts::default();
        evm_opts.env.chain_id = Some(177);
        evm_opts.networks = NetworkConfigs::with_hashkey();
        let resolved = CommandProfileResolution::new()
            .resolve_evm_opts(evm_opts, NetworkIntent::new())
            .unwrap();
        let prepared =
            EvmConstruction::prepare::<OpEvmNetwork>(&resolved, &Config::default()).await.unwrap();
        let executor =
            TracingExecutor::new(prepared, None, TraceMode::Call, Address::ZERO, None).unwrap();
        let decoder = executor.trace_decoder(DecoderConfig::default());

        decode_debugger_traces(&mut traces, &decoder).await;

        let decoded = traces[0].1.nodes()[0].trace.decoded.as_deref().unwrap();
        assert_eq!(decoded.label.as_deref(), Some("H20Asset"));
        let call = decoded.call_data.as_ref().unwrap();
        assert_eq!(call.signature, "mint(address,uint256)");
        assert_eq!(call.args, [recipient.to_string(), "42".to_string()]);
    }
}
