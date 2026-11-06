//! **Running a command LINE someone else wrote**, the way their host runs it.
//!
//! A status line or a wrapped command is a string written for a shell. On Unix
//! that is `sh -c`. On Windows it is not `cmd /C` by default: Claude Code runs
//! its commands through Git Bash, and `std::process::Command` quotes every
//! argument with the MSVCRT rules, which cmd.exe does not parse — a line with
//! its own quotes (`node "d:\x\statusline.mjs"`) arrives as
//! `"node \"d:\x\statusline.mjs\""` and cmd runs nothing (#10).

use std::path::PathBuf;
use std::process::Command;

/// Git Bash, located the way Claude Code locates it: `CLAUDE_CODE_GIT_BASH_PATH`
/// first, then the `bash.exe` of the Git whose `git.exe` is on PATH, then the
/// machine-wide and per-user default install directories. `None` off Windows.
pub fn git_bash() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let exists = |p: &PathBuf| p.is_file();
    if let Some(p) = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH").map(PathBuf::from) {
        if exists(&p) {
            return Some(p);
        }
    }
    // `<root>\cmd\git.exe` or `<root>\bin\git.exe` or `<root>\mingw64\bin\git.exe`.
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if !dir.join("git.exe").is_file() {
                continue;
            }
            for root in dir.ancestors().skip(1).take(2) {
                let bash = root.join("bin").join("bash.exe");
                if exists(&bash) {
                    return Some(bash);
                }
            }
        }
    }
    let mut roots: Vec<PathBuf> = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|d| PathBuf::from(d).join("Git"))
        .collect();
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(l).join("Programs").join("Git"));
    }
    roots.push(PathBuf::from(r"C:\Program Files\Git"));
    roots
        .into_iter()
        .map(|r| r.join("bin").join("bash.exe"))
        .find(exists)
}

/// `cmd.exe` given the line VERBATIM: `/d /s /c "<line>"`, where `/s` strips
/// exactly the outer pair and leaves the line's own quotes alone (what Node's
/// `shell: true` does). `raw_arg` keeps Rust's escaping out of it.
#[cfg(windows)]
pub fn cmd_exe(line: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new("cmd.exe");
    c.raw_arg(format!("/d /s /c \"{line}\""));
    super::configure_no_window(&mut c);
    c
}

/// A POSIX shell running `line`: `sh -c` on Unix; on Windows Git Bash, else
/// cmd.exe with the line verbatim. Every caller captures the output, so no
/// console window is ever wanted.
pub fn user_shell(line: &str) -> Command {
    #[cfg(windows)]
    {
        match git_bash() {
            Some(bash) => {
                let mut c = Command::new(bash);
                c.arg("-c").arg(line);
                super::configure_no_window(&mut c);
                c
            }
            None => cmd_exe(line),
        }
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(line);
        super::configure_no_window(&mut c);
        c
    }
}

#[cfg(test)]
#[cfg(windows)]
mod tests {
    use super::*;

    fn stdout(mut c: Command) -> String {
        String::from_utf8_lossy(&c.output().unwrap().stdout).into_owned()
    }

    #[test]
    fn cmd_gets_the_line_verbatim() {
        assert_eq!(
            stdout(cmd_exe(r#"echo "a b" "c\d""#)).trim_end(),
            r#""a b" "c\d""#
        );
    }

    #[test]
    fn git_bash_keeps_a_quoted_backslash_path() {
        let bash = git_bash().expect("Git Bash is installed on the test machine");
        assert!(bash.is_file());
        assert_eq!(
            stdout(user_shell(r#"printf '%s' "D:\x\statusline.mjs""#)),
            r"D:\x\statusline.mjs"
        );
    }
}
