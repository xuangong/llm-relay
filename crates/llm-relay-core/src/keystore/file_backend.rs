use super::Backend;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct FileBackend {
    pub(crate) path: PathBuf,
}

impl FileBackend {
    pub fn new(path: PathBuf) -> Self { Self { path } }
}

impl Backend for FileBackend {
    fn load(&self) -> HashMap<String, String> {
        // TODO Task 9: real AES-GCM decryption
        log::warn!("file backend stubbed; load returning empty");
        HashMap::new()
    }

    fn save(&self, _map: &HashMap<String, String>) {
        // TODO Task 9
        log::warn!("file backend stubbed; save no-op");
    }
}
