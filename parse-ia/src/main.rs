mod config;
mod logging;
mod parser;
mod pdf_text;
mod utils;

use anyhow::Result;

use crate::config::load_config;
use crate::parser::BackupParser;

fn main() -> Result<()> {
    logging::init_logging();

    let config_path = std::env::args().nth(1);
    let config = load_config(config_path.as_deref())?;

    for account in &config.accounts {
        let parser =
            BackupParser::new(&config.email_folder, &config.parse_ia_folder, &account.name);
        parser.parse_account_backup()?;
    }

    Ok(())
}
