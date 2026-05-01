#![deny(unsafe_code)]

//! Shell resolution for local subprocess execution.
//!
//! - Unix: prefer `bash`, fall back to `sh`.
//! - Windows: prefer `pwsh` (PowerShell 7 — has `&&`/`||`), fall back to
//!   `powershell.exe` (5.1, no chain operators — manifests must use
//!   `; if ($?) { ... }` style).

use std::path::PathBuf;

use crate::error::LocalError;

/// A shell binary plus the static argv prefix to pass before the user script.
#[derive(Debug, Clone)]
pub struct ResolvedShell {
    pub binary: PathBuf,
    /// Static flags inserted before the script string. The script itself is
    /// appended as the final argument by the caller.
    pub leading_args: Vec<String>,
    /// User-friendly identifier ("bash", "pwsh", …) used in events and errors.
    pub kind: ShellKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Bash,
    Sh,
    Pwsh,
    PowerShell,
}

impl ShellKind {
    pub fn label(self) -> &'static str {
        match self {
            ShellKind::Bash => "bash",
            ShellKind::Sh => "sh",
            ShellKind::Pwsh => "pwsh",
            ShellKind::PowerShell => "powershell",
        }
    }
}

#[cfg(unix)]
const CANDIDATES: &[(&str, ShellKind, &[&str])] = &[
    ("bash", ShellKind::Bash, &["-c"]),
    ("sh", ShellKind::Sh, &["-c"]),
];

#[cfg(windows)]
const CANDIDATES: &[(&str, ShellKind, &[&str])] = &[
    (
        "pwsh",
        ShellKind::Pwsh,
        &["-NoProfile", "-NonInteractive", "-Command"],
    ),
    (
        "powershell",
        ShellKind::PowerShell,
        &["-NoProfile", "-NonInteractive", "-Command"],
    ),
];

pub fn resolve_shell() -> Result<ResolvedShell, LocalError> {
    for (name, kind, leading) in CANDIDATES {
        if let Ok(p) = which::which(name) {
            return Ok(ResolvedShell {
                binary: p,
                leading_args: leading.iter().map(|s| s.to_string()).collect(),
                kind: *kind,
            });
        }
    }
    let names: Vec<&str> = CANDIDATES.iter().map(|(n, _, _)| *n).collect();
    Err(LocalError::NoShell(names.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_a_shell_on_this_host() {
        let s = resolve_shell().expect("host must have at least one supported shell");
        assert!(!s.leading_args.is_empty());
    }
}
