mod agents;
mod client;
mod config;
mod enrichment;
mod logging;

use anyhow::Result;
use clap::Parser;

use crate::agents::load_agents_spec;
use crate::config::load_runtime_config;
use crate::enrichment::EnrichmentRunner;

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Enrich parse-ia folders with OpenAI structured outputs"
)]
struct Cli {
    /// Path to the YAML configuration file. Defaults to ./config.yml
    config: Option<String>,
    /// Restrict processing to a single configured account name
    #[arg(long)]
    account: Option<String>,
    /// Override the AGENTS.md path declared in the YAML
    #[arg(long)]
    agents: Option<String>,
    /// Recompute outputs even if the input hash already matches
    #[arg(long)]
    force: bool,
    /// Only process the first N parsed email folders
    #[arg(long)]
    limit: Option<usize>,
    /// Build the request payloads without calling OpenAI
    #[arg(long)]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    logging::init_logging();

    let cli = Cli::parse();
    let runtime = load_runtime_config(cli.config.as_deref(), cli.agents.as_deref())?;
    let agents = load_agents_spec(&runtime.agents_path)?;
    let runner = EnrichmentRunner {
        config: runtime.config,
        agents_path: runtime.agents_path,
        force: cli.force,
        limit: cli.limit,
        account_filter: cli.account,
        dry_run: cli.dry_run,
    };
    runner.run(&agents).await?;

    Ok(())
}
