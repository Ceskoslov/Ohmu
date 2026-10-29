use super::CliError;
use agent_harness_core::{CancellationToken, ModelError};
use agent_harness_provider_openai::{
    OpenAiCompatibleAdapter, OpenAiConfig, RequestBudget, RequestTokenCounter, ReqwestTransport,
};
use clap::Args;
use ohmu_nausicaa::workflow::{WorkflowBudget, WorkflowKind, run_workflow};
use serde_json::{Value, json};
use std::{
    io::{Read, Seek, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Args)]
pub struct ModelArgs {
    /// User question or draft-generation instruction.
    question: String,
    #[arg(long, default_value = ".")]
    wiki: PathBuf,
    /// Complete trusted chat-completions endpoint (not an ingestion URL).
    #[arg(long)]
    endpoint: String,
    #[arg(long)]
    model: String,
    /// Trusted model-specific executable. Receives request JSON on stdin;
    /// returns {"input_tokens":N} on stdout. No shell is involved.
    #[arg(long)]
    token_counter: PathBuf,
    #[arg(long)]
    context_window: u64,
    #[arg(long, default_value_t = 4096)]
    output_tokens: u64,
    #[arg(long, default_value = "OHMU_API_KEY", conflicts_with = "no_api_key")]
    key_env: String,
    /// Explicitly use an unauthenticated local/provider endpoint.
    #[arg(long)]
    no_api_key: bool,
    /// A new JSONL event file; acceptance is saved alongside it.
    #[arg(long)]
    events: PathBuf,
}
struct ExecutableCounter {
    path: PathBuf,
}
impl RequestTokenCounter for ExecutableCounter {
    fn count_input_tokens(&self, body: &Value) -> Result<u64, ModelError> {
        let count = || -> Result<u64, Box<dyn std::error::Error>> {
            let mut input = tempfile::tempfile()?;
            input.write_all(&serde_json::to_vec(body)?)?;
            input.rewind()?;
            let mut output = tempfile::tempfile()?;
            let mut child = Command::new(&self.path)
                .stdin(input)
                .stdout(output.try_clone()?)
                .stderr(Stdio::null())
                .spawn()?;
            let started = Instant::now();
            loop {
                if let Some(status) = child.try_wait()? {
                    if !status.success() {
                        return Err("counter exited unsuccessfully".into());
                    }
                    break;
                }
                if started.elapsed() > Duration::from_secs(5) || output.metadata()?.len() > 4096 {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("counter exceeded time/output limit".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            output.rewind()?;
            let mut bytes = Vec::new();
            output.take(4097).read_to_end(&mut bytes)?;
            if bytes.len() > 4096 {
                return Err("counter output too large".into());
            }
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Count {
                input_tokens: u64,
            }
            let count: Count = serde_json::from_slice(&bytes)?;
            if count.input_tokens == 0 {
                return Err("counter returned zero".into());
            }
            Ok(count.input_tokens)
        };
        count().map_err(|_| {
            ModelError::new(
                "trusted model-specific token counter failed; request was not sent",
                false,
            )
        })
    }
}
pub fn run(args: ModelArgs, kind: WorkflowKind) -> Result<(&'static str, Value, String), CliError> {
    let execute = || -> Result<_, Box<dyn std::error::Error + Send + Sync>> {
        if !args.token_counter.is_absolute() || !args.token_counter.is_file() {
            return Err("--token-counter requires an absolute trusted executable path".into());
        }
        let wiki = Arc::new(ohmu_engine::Wiki::open(&args.wiki)?);
        let mut config = OpenAiConfig::new(&args.endpoint, &args.model);
        config.api_key = if args.no_api_key {
            None
        } else {
            Some(
                std::env::var(&args.key_env)
                    .map_err(|_| "configured API key environment variable is missing")?,
            )
        };
        config.timeout_seconds = 120;
        let provider =
            OpenAiCompatibleAdapter::new_async(config, Arc::new(ReqwestTransport::new()?))
                .with_request_budget(
                    RequestBudget {
                        context_window_tokens: args.context_window,
                        reserved_output_tokens: args.output_tokens,
                    },
                    Arc::new(ExecutableCounter {
                        path: args.token_counter,
                    }),
                )?;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(run_workflow(
            wiki,
            Arc::new(provider),
            kind,
            &args.question,
            &args.events,
            WorkflowBudget::default(),
            CancellationToken::new(),
        ))
    };
    let report = execute().map_err(|e| CliError::Model(e.to_string()))?;
    let text = serde_json::to_string_pretty(&report)?;
    // Rejected/partial results remain inspectable, but shell callers can identify
    // acceptance from this explicit field rather than TurnCompleted.
    Ok((
        if kind == WorkflowKind::Ask {
            "ask"
        } else {
            "compile"
        },
        json!(report),
        text,
    ))
}
