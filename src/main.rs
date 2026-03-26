mod config;
mod index_store;
mod logging;
mod mail_reader;
mod utils;

use anyhow::Result;
use log::info;

use crate::config::load_config;
use crate::index_store::IndexStore;
use crate::mail_reader::MailReader;

/// Coordinates the full program lifecycle: config loading, index reconstruction,\n/// IMAP processing for each account, and final index persistence.
#[tokio::main]
async fn main() -> Result<()> {
    logging::init_logging();

    let config_path = std::env::args().nth(1);
    let config = load_config(config_path.as_deref())?;

    info!("🚀 Read stored indexes");
    let mut index_store = IndexStore::read_from_disk(&config.email_folder)?;
    info!("🏁 Read stored indexes");

    for account in &config.accounts {
        let mut reader = MailReader::new(&config, account, &index_store);

        info!("🚀 Open IMAP Session");
        let mut session = reader.open_imap_session().await?;
        info!("🏁 End Open IMAP Session");

        info!("🚀 Create base folder");
        reader.create_base_folder_if_needed()?;
        info!("🏁 Create base folder");

        info!("🚀 Read indexes");
        reader.read_indexes(&mut index_store)?;
        info!("🏁 Read indexes");

        info!("🚀 Process Emails, account name=[{}]", account.name);
        reader
            .read_imap_emails(&mut session, &mut index_store)
            .await?;
        info!("🏁 Process Emails, account name=[{}]", account.name);

        session.logout().await;
    }

    info!("🚀 Write indexes");
    index_store.write_to_disk()?;
    info!("🏁 End Write indexes");

    Ok(())
}
