mod auth;
mod config;
mod dropbox;
mod filer;
mod logging;
mod table;
mod utils;

use anyhow::Result;
use clap::Parser;

use crate::auth::resolve_access_token;
use crate::config::load_runtime_config;
use crate::filer::DropboxFiler;

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Upload parse-ia attachments to Dropbox using ai-enrich classification"
)]
struct Cli {
    /// Path to the YAML configuration file. Defaults to ./config.yml
    config: Option<String>,
    /// Restrict processing to a single configured account name
    #[arg(long)]
    account: Option<String>,
    /// Override the Dropbox root folder declared in the YAML
    #[arg(long)]
    root: Option<String>,
    /// Recompute outputs even if the input hash already matches
    #[arg(long)]
    force: bool,
    /// Only process the first N parsed email folders
    #[arg(long)]
    limit: Option<usize>,
    /// Build Dropbox destinations without calling the API
    #[arg(long)]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    logging::init_logging();

    let cli = Cli::parse();
    let runtime = load_runtime_config(cli.config.as_deref(), cli.root.as_deref())?;
    let access_token = resolve_access_token(&runtime.config).await?;
    let runner = DropboxFiler {
        config: runtime.config,
        dropbox_root_folder: runtime.dropbox_root_folder,
        access_token,
        force: cli.force,
        limit: cli.limit,
        account_filter: cli.account,
        dry_run: cli.dry_run,
    };
    runner.run().await
}
