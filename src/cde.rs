//! This CDE's identity: its environment id from `ONA_ID` and its name from `gitpod`.

use std::process::{Command, Stdio};

/// This CDE's environment id, from `ONA_ID`; `None` outside a CDE.
pub fn id() -> Option<String> {
    std::env::var("ONA_ID").ok().filter(|id| !id.is_empty())
}

/// The name of the CDE with environment id `id`, from `gitpod environment get`; `None` on
/// any failure.
pub fn name(id: &str) -> Option<String> {
    let out = Command::new("gitpod")
        .args([
            "environment",
            "get",
            id,
            "--field",
            "name",
            "--timeout",
            "3s",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let name = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !name.is_empty()).then_some(name)
}

/// This CDE's name; `None` outside a CDE or if `gitpod` cannot say.
pub fn this_name() -> Option<String> {
    name(&id()?)
}
