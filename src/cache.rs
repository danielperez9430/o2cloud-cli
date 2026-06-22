use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::config::config_dir;
use crate::error::O2Error;

/// Local cache mapping media IDs to file metadata for display in `ls`.
#[derive(Debug, serde::Serialize, serde::Deserialize, Default)]
pub struct FileCache {
    pub files: HashMap<u64, CachedFile>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CachedFile {
    pub name: String,
    pub size: u64,
    pub contenttype: String,
    pub date: String,
    #[serde(default)]
    pub folder_id: u64,
}

fn cache_file() -> PathBuf {
    config_dir().join("file_cache.json")
}

impl FileCache {
    pub fn load() -> Self {
        let path = cache_file();
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) -> Result<(), O2Error> {
        let dir = config_dir();
        fs::create_dir_all(&dir).map_err(|e| {
            O2Error::Config(format!("Failed to create config dir: {}", e))
        })?;
        let json = serde_json::to_string_pretty(self).unwrap();
        fs::write(cache_file(), json).map_err(|e| {
            O2Error::Config(format!("Failed to write file cache: {}", e))
        })?;
        Ok(())
    }

    pub fn insert(
        &mut self, id: u64, name: String, size: u64,
        contenttype: String, date: String, folder_id: u64,
    ) -> Result<(), O2Error> {
        self.files.insert(
            id,
            CachedFile { name, size, contenttype, date, folder_id },
        );
        self.save()
    }
}
