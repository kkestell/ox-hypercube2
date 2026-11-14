use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::PathBuf,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub cwd: PathBuf,
    pub pi_session_id: String,
    pub title: Option<String>,
}

/// ACP identity and metadata shared across bridge processes. Live Pi handles
/// belong to the bridge and are never persisted.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn get(&self, id: &str) -> io::Result<Option<Session>> {
        self.transaction(|entries| (entries.get(id).cloned(), false))
    }

    pub fn create(&self, id: String, session: &Session) -> io::Result<()> {
        self.transaction(|entries| {
            entries.insert(id, session.clone());
            ((), true)
        })
    }

    pub fn update(&self, id: &str, update: impl FnOnce(&mut Session)) -> io::Result<()> {
        self.transaction(|entries| {
            let session = entries
                .get_mut(id)
                .expect("open sessions have registry entries");
            update(session);
            ((), true)
        })
    }

    fn transaction<T>(
        &self,
        operation: impl FnOnce(&mut HashMap<String, Session>) -> (T, bool),
    ) -> io::Result<T> {
        let _lock = self.lock()?;
        let path = self.dir.join("acp-sessions.json");
        let mut entries = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(error) => return Err(error),
        };
        let (result, changed) = operation(&mut entries);
        if changed {
            let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
            serde_json::to_writer_pretty(&mut file, &entries)?;
            file.flush()?;
            file.persist(path).map_err(|error| error.error)?;
        }
        Ok(result)
    }

    fn lock(&self) -> io::Result<File> {
        fs::create_dir_all(&self.dir)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join("acp-sessions.lock"))?;
        file.lock()?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_retain_other_sessions_across_stores() {
        let dir = tempfile::tempdir().unwrap();
        let first = Store::new(dir.path().to_owned());
        let second = Store::new(dir.path().to_owned());
        let session = Session {
            cwd: dir.path().to_owned(),
            pi_session_id: "native".into(),
            title: None,
        };
        first.create("one".into(), &session).unwrap();
        second.create("two".into(), &session).unwrap();
        first
            .update("one", |s| s.title = Some("title".into()))
            .unwrap();
        assert_eq!(
            second.get("one").unwrap().unwrap().title.as_deref(),
            Some("title")
        );
        assert_eq!(first.get("two").unwrap().unwrap().pi_session_id, "native");
    }
}
