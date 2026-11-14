use std::{env, ffi::OsString, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    pub store_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Self {
        let home = dirs::home_dir();
        Self {
            store_dir: store_dir_from(
                env::var_os("PI_ACP_STORE_DIR"),
                env::var_os("XDG_DATA_HOME"),
                home,
            ),
        }
    }
}

/// The store directory: `$PI_ACP_STORE_DIR`, else
/// `$XDG_DATA_HOME/pi-acp` when `XDG_DATA_HOME` is an absolute path,
/// else `~/.local/share/pi-acp`, else `<temp dir>/pi-acp`
/// when there is no home directory.
fn store_dir_from(
    store: Option<OsString>,
    xdg: Option<OsString>,
    home: Option<PathBuf>,
) -> PathBuf {
    if let Some(store) = store {
        return PathBuf::from(store);
    }
    let base = match (xdg.map(PathBuf::from), home) {
        (Some(xdg), _) if xdg.is_absolute() => xdg,
        (_, Some(home)) => home.join(".local/share"),
        (_, None) => env::temp_dir(),
    };
    base.join("pi-acp")
}
