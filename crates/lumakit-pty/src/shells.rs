//! Shell discovery (04 §6.6). Windows: pwsh, powershell, cmd, Git Bash
//! (from HKLM `SOFTWARE\GitForWindows`), WSL distros (`wsl.exe -l -q`).
//! Non-Windows: `/bin/sh`, `/bin/bash`, `/bin/zsh` when present, in `$SHELL`
//! order.
//!
//! The catalog is a snapshot: callers build one at startup and refresh when
//! user settings change. Discovery never panics; a failure to probe one
//! shell is logged and that shell is omitted.

use std::path::PathBuf;
use std::process::Command;

use tracing::warn;

/// A single discovered shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    /// User-facing label (`pwsh`, `Git Bash`, `Ubuntu-22.04`).
    pub id: String,
    /// Absolute path or special launcher.
    pub program: PathBuf,
    /// Extra arguments the launcher needs (WSL distros: `-d <name>`).
    pub args: Vec<String>,
    /// Working directory label for the UI (not used to spawn).
    pub kind: ShellKind,
}

/// Rough classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// PowerShell (new or old).
    PowerShell,
    /// Classic `cmd.exe`.
    Cmd,
    /// Git for Windows Bash.
    GitBash,
    /// WSL distribution.
    Wsl,
    /// POSIX shell (`sh`/`bash`/`zsh`).
    Posix,
}

/// Ordered snapshot of discovered shells.
#[derive(Debug, Default, Clone)]
pub struct ShellCatalog {
    /// Shells in display order.
    pub shells: Vec<Shell>,
}

/// Discover shells on the current host.
pub fn discover() -> ShellCatalog {
    let mut shells = Vec::new();
    #[cfg(windows)]
    {
        discover_windows(&mut shells);
    }
    #[cfg(unix)]
    {
        discover_unix(&mut shells);
    }
    ShellCatalog { shells }
}

#[cfg(windows)]
fn discover_windows(out: &mut Vec<Shell>) {
    // pwsh > powershell > cmd, each only if their binary is on `PATH` (via
    // `where.exe`).
    for (name, kind) in [
        ("pwsh", ShellKind::PowerShell),
        ("powershell", ShellKind::PowerShell),
        ("cmd", ShellKind::Cmd),
    ] {
        if let Some(path) = which_on_windows(name) {
            out.push(Shell {
                id: name.to_owned(),
                program: path,
                args: Vec::new(),
                kind,
            });
        }
    }

    if let Some(bash) = git_bash_from_registry() {
        out.push(Shell {
            id: "Git Bash".to_owned(),
            program: bash,
            args: Vec::new(),
            kind: ShellKind::GitBash,
        });
    }

    for name in list_wsl_distributions() {
        out.push(Shell {
            id: format!("WSL: {name}"),
            program: PathBuf::from("wsl.exe"),
            args: vec!["-d".to_owned(), name],
            kind: ShellKind::Wsl,
        });
    }
}

#[cfg(windows)]
fn which_on_windows(name: &str) -> Option<PathBuf> {
    let output = Command::new("where").arg(name).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().next().map(|line| PathBuf::from(line.trim()))
}

#[cfg(windows)]
fn git_bash_from_registry() -> Option<PathBuf> {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm
        .open_subkey("SOFTWARE\\GitForWindows")
        .map_err(|error| warn!(%error, "git bash registry read failed"))
        .ok()?;
    let install: String = key
        .get_value("InstallPath")
        .map_err(|error| warn!(%error, "git bash InstallPath missing"))
        .ok()?;
    let path = PathBuf::from(install).join("bin").join("bash.exe");
    path.exists().then_some(path)
}

#[cfg(windows)]
fn list_wsl_distributions() -> Vec<String> {
    let output = Command::new("wsl.exe").args(["-l", "-q"]).output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    // `wsl.exe -l -q` prints UTF-16LE with a BOM.
    let bytes = &output.stdout;
    let text = utf16le_to_string(bytes);
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(windows)]
fn utf16le_to_string(bytes: &[u8]) -> String {
    let start = if bytes.starts_with(&[0xff, 0xfe]) {
        2
    } else {
        0
    };
    let units: Vec<u16> = bytes[start..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(unix)]
fn discover_unix(out: &mut Vec<Shell>) {
    use std::env;
    use std::path::Path;

    if let Ok(shell) = env::var("SHELL") {
        let path = PathBuf::from(&shell);
        if path.exists() {
            out.push(Shell {
                id: path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("shell")
                    .to_owned(),
                program: path,
                args: Vec::new(),
                kind: ShellKind::Posix,
            });
        }
    }
    for candidate in ["/bin/zsh", "/bin/bash", "/bin/sh"] {
        if Path::new(candidate).exists()
            && !out
                .iter()
                .any(|shell| shell.program.as_os_str() == candidate)
        {
            out.push(Shell {
                id: candidate.rsplit('/').next().unwrap_or(candidate).to_owned(),
                program: PathBuf::from(candidate),
                args: Vec::new(),
                kind: ShellKind::Posix,
            });
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn utf16_bom_decodes_distro_name() {
        // Simulated `wsl.exe -l -q` output for one distribution.
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(
            "Ubuntu-22.04\r\n"
                .encode_utf16()
                .flat_map(|u| u.to_le_bytes()),
        );
        let text = utf16le_to_string(&bytes);
        assert!(text.contains("Ubuntu-22.04"));
    }

    #[test]
    fn discover_does_not_panic() {
        let _catalog = discover();
    }
}
