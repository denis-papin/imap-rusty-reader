use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const INDEX_FILE_NAME: &str = "message-id.v2.json";

#[derive(Debug, Serialize, Deserialize)]
struct StoredIndex {
    version: u32,
    message_ids: Vec<String>,
}

#[derive(Debug)]
pub struct IndexStore {
    path: PathBuf,
    ids: HashSet<String>,
}

impl IndexStore {
    /// Loads the JSON index file from the configured mail root, or returns an\n    /// empty in-memory store when the file does not exist yet.
    pub fn read_from_disk(base_folder: &str) -> Result<Self> {
        let path = Path::new(base_folder).join(INDEX_FILE_NAME);
        if !path.exists() {
            return Ok(Self {
                path,
                ids: HashSet::new(),
            });
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("unable to read index file {}", path.display()))?;
        let stored: StoredIndex = serde_json::from_str(&content)
            .with_context(|| format!("unable to parse index file {}", path.display()))?;

        Ok(Self {
            path,
            ids: stored.message_ids.into_iter().collect(),
        })
    }

    /// Persists the current set of known Message-ID values as a stable JSON file\n    /// in the mail root directory.
    pub fn write_to_disk(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("unable to create {}", parent.display()))?;
        }

        let mut message_ids = self.ids.iter().cloned().collect::<Vec<_>>();
        message_ids.sort();
        let stored = StoredIndex {
            version: 2,
            message_ids,
        };
        let payload = serde_json::to_string_pretty(&stored)?;
        fs::write(&self.path, payload)
            .with_context(|| format!("unable to write index file {}", self.path.display()))?;
        Ok(())
    }

    /// Returns whether the given Message-ID is already known by the local index.
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// Adds a Message-ID to the in-memory index after a successful disk or IMAP read.
    pub fn insert(&mut self, id: String) {
        self.ids.insert(id);
    }

    /// Clones the current Message-ID set so recovery logic can compare it against\n    /// what is still present on disk.
    pub fn snapshot(&self) -> HashSet<String> {
        self.ids.clone()
    }
}
