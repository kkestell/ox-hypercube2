use std::path::{Component, Path, PathBuf};

pub fn cwd(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("cwd must be an absolute path: {}", path.display()));
    }
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    if !resolved.is_dir() {
        return Err(format!("cwd is not a directory: {}", resolved.display()));
    }
    Ok(resolved)
}
