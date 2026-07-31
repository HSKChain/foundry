use std::str::FromStr;

use alloy_chains::Chain;
use alloy_primitives::{Address, Bytes, map::HashMap};
use foundry_cli::utils::{TraceResult, print_traces};
use foundry_common::{ContractsByArtifact, compile::ProjectCompiler, shell};
use foundry_config::Config;
use foundry_debugger::Debugger;
use foundry_evm::traces::{
    CallTraceDecoder, CallTraceDecoderBuilder, DebugTraceIdentifier, Traces,
    debug::ContractSources,
    decode_trace_arena,
    identifier::{SignaturesIdentifier, TraceIdentifiers},
};
use foundry_evm_networks::{NetworkExecutionContext, ResolvedNetworkProfile};

async fn decode_debugger_traces(traces: &mut Traces, decoder: &CallTraceDecoder) {
    for (_, trace) in traces {
        decode_trace_arena(trace, decoder).await;
    }
}

/// labels the traces, conditionally prints them or opens the debugger
#[expect(clippy::too_many_arguments)]
pub(crate) async fn handle_traces(
    mut result: TraceResult,
    config: &Config,
    chain: Chain,
    contracts_bytecode: &HashMap<Address, Bytes>,
    labels: Vec<String>,
    with_local_artifacts: bool,
    debug: bool,
    decode_internal: bool,
    disable_label: bool,
    trace_depth: Option<usize>,
    network_profile: ResolvedNetworkProfile,
    network_context: NetworkExecutionContext,
) -> eyre::Result<()> {
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

    let mut builder = CallTraceDecoderBuilder::new()
        .with_labels(labels.chain(config_labels))
        .with_signature_identifier(SignaturesIdentifier::from_config(config)?)
        .with_label_disabled(disable_label)
        .with_chain_id(Some(chain.id()))
        .with_network_profile(network_profile, network_context);
    let mut identifier = TraceIdentifiers::new().with_external(config, Some(chain))?;
    if let Some(contracts) = &known_contracts {
        builder = builder.with_known_contracts(contracts);
        identifier = identifier.with_local_and_bytecodes(contracts, contracts_bytecode);
    }

    let mut decoder = builder.build();

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
    use foundry_evm::traces::{
        CallTrace, CallTraceArena, CallTraceNode, SparsedTraceArena, TraceKind,
    };
    use foundry_evm_networks::NetworkConfigs;

    #[tokio::test]
    async fn hashkey_debugger_projection_decodes_b20_calls() {
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
        let decoder = CallTraceDecoderBuilder::new()
            .with_network_profile(
                NetworkConfigs::with_hashkey().resolve(),
                NetworkExecutionContext::new(177, 0),
            )
            .build();

        decode_debugger_traces(&mut traces, &decoder).await;

        let decoded = traces[0].1.nodes()[0].trace.decoded.as_deref().unwrap();
        assert_eq!(decoded.label.as_deref(), Some("B20Asset"));
        let call = decoded.call_data.as_ref().unwrap();
        assert_eq!(call.signature, "mint(address,uint256)");
        assert_eq!(call.args, [recipient.to_string(), "42".to_string()]);
    }
}
