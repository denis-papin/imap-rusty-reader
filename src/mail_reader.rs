use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_imap::Session;
use chrono::{DateTime, Utc};
use filetime::FileTime;
use futures_util::TryStreamExt;
use log::{debug, error, info};
use mailparse::{MailHeaderMap, ParsedMail};
use tokio::net::TcpStream;
use tokio_native_tls::{TlsConnector, TlsStream, native_tls};

use crate::config::{Account, Config};
use crate::index_store::IndexStore;
use crate::utils::{
    ContactInfo, EmailSep, extract_contact_info, normalize_display_name, sanitize_filename,
};

const MAX_MESSAGE_TO_READ: usize = 10_000;

#[derive(Debug, Clone, Copy)]
enum Direction {
    In,
    Out,
}

pub enum ImapSession {
    Tls(Session<TlsStream<TcpStream>>),
    Plain(Session<TcpStream>),
}

impl ImapSession {
    /// Closes the IMAP session gracefully regardless of whether the underlying\n    /// transport is TLS or plaintext.
    pub async fn logout(&mut self) {
        match self {
            Self::Tls(session) => {
                let _ = session.logout().await;
            }
            Self::Plain(session) => {
                let _ = session.logout().await;
            }
        }
    }
}

pub struct MailReader {
    base_folder: PathBuf,
    sub_folder: String,
    hostname: String,
    login: String,
    password: String,
    port: u16,
    ssl_enabled: bool,
    group: bool,
    recover: bool,
    imap_in_folder: Vec<String>,
    imap_out_folder: Vec<String>,
    in_recovery: HashSet<String>,
    contact_map: HashMap<String, String>,
}

impl MailReader {
    /// Builds a reader instance for one configured account and seeds its recovery\n    /// state from the global Message-ID index.
    pub fn new(config: &Config, account: &Account, index_store: &IndexStore) -> Self {
        Self {
            base_folder: PathBuf::from(&config.email_folder),
            sub_folder: account.name.clone(),
            hostname: account.server.clone(),
            login: account.login.clone(),
            password: account.password.clone(),
            port: account.port,
            ssl_enabled: account.ssl_enabled,
            group: account.group,
            recover: account.recover,
            imap_in_folder: account.imap_in_folder.clone(),
            imap_out_folder: account.imap_out_folder.clone(),
            in_recovery: index_store.snapshot(),
            contact_map: HashMap::new(),
        }
    }

    /// Opens and authenticates an IMAP session using either TLS or plaintext,\n    /// mirroring the `sslEnabled` behavior of the Java version.
    pub async fn open_imap_session(&self) -> Result<ImapSession> {
        let tcp = TcpStream::connect((self.hostname.as_str(), self.port))
            .await
            .with_context(|| format!("unable to connect to {}:{}", self.hostname, self.port))?;

        if self.ssl_enabled {
            let tls = native_tls::TlsConnector::builder()
                .build()
                .context("unable to build TLS connector")?;
            let tls = TlsConnector::from(tls);
            let tls_stream = tls
                .connect(self.hostname.as_str(), tcp)
                .await
                .with_context(|| format!("unable to negotiate TLS with {}", self.hostname))?;
            let mut client = async_imap::Client::new(tls_stream);
            client
                .read_response()
                .await?
                .context("unexpected end of stream, expected greeting")?;
            let session = client
                .login(&self.login, &self.password)
                .await
                .map_err(|(err, _client)| err)
                .with_context(|| format!("unable to login to {}", self.login))?;
            Ok(ImapSession::Tls(session))
        } else {
            let mut client = async_imap::Client::new(tcp);
            client
                .read_response()
                .await?
                .context("unexpected end of stream, expected greeting")?;
            let session = client
                .login(&self.login, &self.password)
                .await
                .map_err(|(err, _client)| err)
                .with_context(|| format!("unable to login to {}", self.login))?;
            Ok(ImapSession::Plain(session))
        }
    }

    /// Ensures that the configured root folder used to store exported emails\n    /// already exists before any message is written.
    pub fn create_base_folder_if_needed(&self) -> Result<()> {
        fs::create_dir_all(&self.base_folder)
            .with_context(|| format!("unable to create {}", self.base_folder.display()))?;
        Ok(())
    }

    /// Rebuilds the in-memory contact map and confirms which Message-ID values\n    /// are still backed by `.eml` files on disk.
    pub fn read_indexes(&mut self, index_store: &mut IndexStore) -> Result<()> {
        self.read_index_folder(&self.base_folder.clone(), index_store)
    }

    /// Dispatches IMAP reading through the concrete session transport while\n    /// keeping the rest of the mailbox logic transport-agnostic.
    pub async fn read_imap_emails(
        &mut self,
        session: &mut ImapSession,
        index_store: &mut IndexStore,
    ) -> Result<()> {
        match session {
            ImapSession::Tls(session) => {
                self.read_imap_emails_with_session(session, index_store)
                    .await
            }
            ImapSession::Plain(session) => {
                self.read_imap_emails_with_session(session, index_store)
                    .await
            }
        }
    }

    /// Lists folders for diagnostic logging and processes all configured input\n    /// and output IMAP folders for one account.
    async fn read_imap_emails_with_session<T>(
        &mut self,
        session: &mut Session<T>,
        index_store: &mut IndexStore,
    ) -> Result<()>
    where
        T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
    {
        let folders = match session.list(None, Some("*")).await {
            Ok(folders) => folders.try_collect::<Vec<_>>().await?,
            Err(_) => Vec::new(),
        };
        for folder in &folders {
            let full_name = folder.name().to_string();
            if let Ok(mailbox) = session.examine(&full_name).await {
                info!(
                    "😎 >> Number of message read : [{}] [{}]",
                    mailbox.exists, full_name
                );
            }
        }

        for imap_folder in self.imap_out_folder.clone() {
            self.read_imap_single_folder(session, &imap_folder, Direction::Out, index_store)
                .await?;
        }
        for imap_folder in self.imap_in_folder.clone() {
            self.read_imap_single_folder(session, &imap_folder, Direction::In, index_store)
                .await?;
        }
        Ok(())
    }

    /// Reads one IMAP folder, filters already indexed messages, and persists the\n    /// newly discovered ones to disk.
    async fn read_imap_single_folder<T>(
        &mut self,
        session: &mut Session<T>,
        imap_folder: &str,
        direction: Direction,
        index_store: &mut IndexStore,
    ) -> Result<()>
    where
        T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
    {
        info!("🚀 Read imap single folder : [{}]", imap_folder);

        let mailbox = match session.examine(imap_folder).await {
            Ok(mailbox) => mailbox,
            Err(_) => {
                debug!("💣 Imap Folder open error : {}", imap_folder);
                info!("🏁 Read imap single folder : [{}]", imap_folder);
                return Ok(());
            }
        };

        let message_count = mailbox.exists as usize;
        info!("😎 Number of message read : [{}]", message_count);
        if message_count == 0 {
            info!("🏁 Read imap single folder : [{}]", imap_folder);
            return Ok(());
        }

        let upper_bound = message_count.min(MAX_MESSAGE_TO_READ);
        let sequence = format!("1:{}", upper_bound);
        let fetches = session
            .fetch(sequence, "RFC822")
            .await
            .with_context(|| format!("unable to fetch messages from {}", imap_folder))?;
        let messages = fetches.try_collect::<Vec<_>>().await?;

        for (idx, fetch) in messages.iter().enumerate() {
            let Some(body) = fetch.body() else {
                continue;
            };

            let parsed = match mailparse::parse_mail(body) {
                Ok(parsed) => parsed,
                Err(err) => {
                    error!("{}", err);
                    continue;
                }
            };

            let mut subject = parsed
                .headers
                .get_first_value("Subject")
                .unwrap_or_default()
                .trim()
                .to_string();
            debug!("🐞 Message number : [{}], subject : [{}]", idx, subject);

            let id = parsed
                .headers
                .get_first_value("Message-ID")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());

            let Some(id) = id else {
                continue;
            };

            if !index_store.contains(&id) || (self.recover && self.in_recovery.contains(&id)) {
                let header_value = match direction {
                    Direction::In => {
                        subject = format!("🔴 {}", subject);
                        parsed.headers.get_first_value("From").unwrap_or_default()
                    }
                    Direction::Out => {
                        subject = format!("🔵 {}", subject);
                        parsed.headers.get_first_value("To").unwrap_or_default()
                    }
                };

                let contact_info = extract_contact_info(&header_value, EmailSep::Brackets);
                let terminal_folder = if self.group {
                    self.build_terminal_folder(contact_info)
                } else {
                    imap_folder.to_string()
                };

                info!("📥 Download message content, subject : [{}]", subject);
                info!("😎 New message saved, subject : [{}]", subject);
                if let Err(err) = self.process_save_to_file(
                    body,
                    &parsed,
                    &subject,
                    &terminal_folder,
                    &id,
                    index_store,
                ) {
                    error!("{}", err);
                }
            }
        }

        info!("🏁 Read imap single folder : [{}]", imap_folder);
        Ok(())
    }

    /// Builds the last-level local folder name used when mails are grouped by\n    /// contact, falling back to the email address when needed.
    fn build_terminal_folder(&mut self, contact_info: ContactInfo) -> String {
        let casual = self.resolve_casual_name(&contact_info);
        if contact_info.email.trim().is_empty() {
            return sanitize_filename(&casual);
        }
        if casual.is_empty() {
            return sanitize_filename(&contact_info.email);
        }
        sanitize_filename(&format!("{} ({})", casual, contact_info.email))
    }

    /// Chooses the best display name for a contact by combining the current\n    /// message data with any non-empty name already learned from disk.
    fn resolve_casual_name(&mut self, contact_info: &ContactInfo) -> String {
        if let Some(existing) = self.contact_map.get(&contact_info.email) {
            if !existing.trim().is_empty() {
                return existing.clone();
            }
        }

        let casual = normalize_display_name(&contact_info.casual);
        if !contact_info.email.is_empty() && !casual.is_empty() {
            self.contact_map
                .insert(contact_info.email.clone(), casual.clone());
        }
        casual
    }

    /// Creates the target folder, chooses a collision-free `.eml` name, writes the\n    /// raw message bytes, updates the Message-ID index, and restores file time.
    fn process_save_to_file(
        &mut self,
        body: &[u8],
        parsed: &ParsedMail<'_>,
        subject: &str,
        terminal_folder: &str,
        message_id: &str,
        index_store: &mut IndexStore,
    ) -> Result<()> {
        let target_folder = self.build_email_folder(terminal_folder);
        fs::create_dir_all(&target_folder)
            .with_context(|| format!("unable to create {}", target_folder.display()))?;

        let safe_subject = sanitize_filename(subject);
        let mut path_name = target_folder.join(format!("{}.eml", safe_subject));
        let mut count = 1;
        while path_name.exists() {
            path_name = target_folder.join(format!("{}_{}.eml", safe_subject, count));
            count += 1;
        }

        info!("🔥 Save message in pathName=[{}]", path_name.display());
        debug!("🐞 Message ID : {}", message_id);
        fs::write(&path_name, body)
            .with_context(|| format!("unable to write {}", path_name.display()))?;

        if !index_store.contains(message_id) {
            index_store.insert(message_id.to_string());
        }

        if let Some(file_time) = extract_received_time(parsed)? {
            filetime::set_file_mtime(&path_name, file_time)
                .with_context(|| format!("unable to set mtime on {}", path_name.display()))?;
        }

        Ok(())
    }

    /// Computes the full local target path for one logical terminal folder under\n    /// the configured account sub-folder.
    fn build_email_folder(&self, terminal_folder: &str) -> PathBuf {
        self.base_folder
            .join(&self.sub_folder)
            .join(terminal_folder)
    }

    /// Recursively walks the local mail tree to rebuild the contact map and\n    /// recover Message-ID values from existing `.eml` files.
    fn read_index_folder(&mut self, folder: &Path, index_store: &mut IndexStore) -> Result<()> {
        for entry in
            fs::read_dir(folder).with_context(|| format!("unable to read {}", folder.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if self.group {
                    self.index_folder(&path);
                }
                self.read_index_folder(&path, index_store)?;
            } else {
                self.index_email(&path, index_store)?;
            }
        }
        Ok(())
    }

    /// Learns a contact display name from an existing grouped folder on disk when\n    /// the folder name embeds an email address.
    fn index_folder(&mut self, folder: &Path) {
        let Some(name) = folder.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if name.contains('@') {
            info!("😎 Indexing Folder : [{}]", name);
            let info = extract_contact_info(name, EmailSep::Parenthesis);
            if !info.email.is_empty() {
                self.contact_map.insert(info.email, info.casual);
            }
        }
    }

    /// Parses one local `.eml` file to recover its `Message-ID` and mark it as\n    /// still present on disk for recovery logic.
    fn index_email(&mut self, eml_file: &Path, index_store: &mut IndexStore) -> Result<()> {
        let Some(name) = eml_file.file_name().and_then(|name| name.to_str()) else {
            return Ok(());
        };
        info!("😎 Indexing Email : [{}]", name);
        if !name.ends_with(".eml") {
            return Ok(());
        }

        let data =
            fs::read(eml_file).with_context(|| format!("unable to read {}", eml_file.display()))?;
        let parsed = mailparse::parse_mail(&data)
            .with_context(|| format!("unable to parse {}", eml_file.display()))?;
        if let Some(id) = parsed.headers.get_first_value("Message-ID") {
            let id = id.trim().to_string();
            if !id.is_empty() {
                index_store.insert(id.clone());
                self.in_recovery.remove(&id);
            }
        }
        Ok(())
    }
}

/// Extracts the message `Date` header and turns it into a filesystem mtime so\n/// exported `.eml` files keep a meaningful timestamp.
fn extract_received_time(parsed: &ParsedMail<'_>) -> Result<Option<FileTime>> {
    let Some(date_header) = parsed.headers.get_first_value("Date") else {
        return Ok(None);
    };

    let timestamp = match mailparse::dateparse(&date_header) {
        Ok(timestamp) => timestamp,
        Err(_) => return Ok(None),
    };

    let datetime = DateTime::<Utc>::from_timestamp(timestamp, 0);
    Ok(datetime.map(|dt| FileTime::from_unix_time(dt.timestamp(), 0)))
}
