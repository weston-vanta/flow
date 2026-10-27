use std::path::{Path, PathBuf};
use std::{env, fs, io};

const STATE_ENV: &str = "FLOW_STATE";

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("state directory not found: {} (expected a symlink to /efs-home/flow/state)", .0.display())]
    Missing(PathBuf),
}

#[derive(Debug)]
pub struct StateDir {
    root: PathBuf,
}

impl StateDir {
    /// `$FLOW_STATE` if set, else `<repo_root>/state`. A dangling symlink counts as missing.
    pub fn resolve(repo_root: &Path) -> Result<StateDir, PathError> {
        let root = env::var_os(STATE_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| repo_root.join("state"));
        if root.is_dir() {
            Ok(StateDir { root })
        } else {
            Err(PathError::Missing(root))
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// This state directory with symlinks resolved: its physical path on the shared volume,
    /// which is the same on every host.
    pub fn canonical(&self) -> io::Result<StateDir> {
        Ok(StateDir {
            root: fs::canonicalize(&self.root)?,
        })
    }
}
