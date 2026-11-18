//! Point git, gh and glab at the tokens rebro holds, so none of them needs its own login.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::model::{Account, Forge};
use crate::store::{config_dir, get_secret, Store};

#[derive(Debug, Clone)]
pub struct Step {
    pub what: String,
    pub ok: bool,
    pub detail: String,
}

fn step(what: impl Into<String>, r: anyhow::Result<String>) -> Step {
    match r {
        Ok(detail) => Step { what: what.into(), ok: true, detail },
        Err(e) => Step { what: what.into(), ok: false, detail: e.to_string() },
    }
}

fn run(cmd: &str, args: &[&str], stdin: Option<&str>) -> anyhow::Result<String> {
    let mut c = Command::new(cmd);
    c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flashes from the GUI
    }
    let mut child = c.spawn()?;
    if let Some(input) = stdin {
        if let Some(mut pipe) = child.stdin.take() {
            pipe.write_all(input.as_bytes())?;
        }
    }
    let out = child.wait_with_output()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string() + &String::from_utf8_lossy(&out.stderr).trim().to_string();
    anyhow::ensure!(out.status.success(), "{cmd} {}: {}", args.first().unwrap_or(&""), text);
    Ok(text)
}

fn have(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// git config value that runs this executable as the credential helper.
pub fn helper_value(exe: &Path) -> String {
    // git runs helpers starting with "!" through its bundled sh: forward slashes, single quotes.
    format!("!'{}' git-credential", exe.display().to_string().replace('\\', "/"))
}

/// Make rebro the only https credential helper for `host`:
/// an empty entry resets inherited helpers (Git Credential Manager, gh), then ours.
pub fn git_helper(host: &str, exe: &Path) -> anyhow::Result<String> {
    let key = format!("credential.https://{host}.helper");
    let current = run("git", &["config", "--global", "--get-all", &key], None).unwrap_or_default();
    let ours = helper_value(exe);
    // (run() trims, so the leading empty reset entry isn't visible here; rewriting is idempotent)
    let already = current.trim() == ours;
    if !current.is_empty() && !already {
        let backup = config_dir().join("git-helper-backup.txt");
        std::fs::create_dir_all(config_dir())?;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&backup)?;
        writeln!(f, "[{}] {key}:\n{current}\n", crate::model::now_utc())?;
    }
    let _ = run("git", &["config", "--global", "--unset-all", &key], None);
    run("git", &["config", "--global", "--add", &key, ""], None)?;
    run("git", &["config", "--global", "--add", &key, &ours], None)?;
    // pass the repo path so several accounts on one host can be told apart
    run("git", &["config", "--global", &format!("credential.https://{host}.useHttpPath"), "true"], None)?;
    Ok(if already { "already set".into() } else { "set".into() })
}

/// Store the token in gh (its own keyring entry), so `gh` works without `gh auth login`.
pub fn gh(acct: &Account) -> anyhow::Result<String> {
    // GitHub OAuth/PAT tokens don't expire on a timer, so a copy in gh stays valid
    let token = get_secret(&acct.key())?;
    run("gh", &["auth", "login", "--hostname", &acct.host, "--git-protocol", "https", "--with-token"], Some(&token))?;
    // gh may have re-added itself as git helper for this host; rebro stays in charge
    Ok(format!("gh now uses {}", acct.username))
}

pub fn glab(acct: &Account) -> anyhow::Result<String> {
    let token = get_secret(&acct.key())?;
    run("glab", &["auth", "login", "--hostname", &acct.host, "--stdin"], Some(&token))?;
    Ok(format!("glab now uses {}", acct.username))
}

/// Everything: git helper for every host, gh/glab for each host's default account.
pub fn sync_all(store: &Store, exe: &Path) -> Vec<Step> {
    let mut steps = vec![];
    let mut hosts: Vec<&str> = store.accounts.iter().map(|a| a.host.as_str()).collect();
    hosts.sort();
    hosts.dedup();
    for host in &hosts {
        steps.push(step(format!("git → {host}"), git_helper(host, exe)));
    }
    // REBRO_SKIP_TOOLS=1: wire git only (tests must not touch the real gh/glab logins)
    let tools = std::env::var("REBRO_SKIP_TOOLS").map(|v| v != "1").unwrap_or(true);
    let (has_gh, has_glab) = (tools && have("gh"), tools && have("glab"));
    for acct in store.accounts.iter().filter(|a| a.default) {
        match acct.forge {
            Forge::GitHub if has_gh => steps.push(step(format!("gh → {}", acct.key()), gh(acct))),
            // a GitLab sign-in expires every 2 hours; a copy in glab would go stale, so only PATs go there
            Forge::GitLab if has_glab && !acct.is_oauth() => steps.push(step(format!("glab → {}", acct.key()), glab(acct))),
            _ => {}
        }
    }
    // gh login can re-register gh as git helper: re-assert ours last
    if has_gh {
        for host in hosts.iter().filter(|h| store.accounts.iter().any(|a| &a.host == *h && a.forge == Forge::GitHub)) {
            let _ = git_helper(host, exe);
        }
    }
    steps
}

/// Turn rebro off for `host`: remove its helper entries so git falls back to the system helper.
pub fn unset_git_helper(host: &str) -> anyhow::Result<String> {
    let _ = run("git", &["config", "--global", "--unset-all", &format!("credential.https://{host}.helper")], None);
    let _ = run("git", &["config", "--global", "--unset-all", &format!("credential.https://{host}.useHttpPath")], None);
    Ok("removed".into())
}

/// What the "Git & tools" page shows.
#[derive(Debug, Clone, Default)]
pub struct ToolsStatus {
    /// (host, rebro is git's helper for it)
    pub helper: Vec<(String, bool)>,
    /// gh installed, and the account it is logged in as (None = not logged in)
    pub gh: Option<Option<String>>,
    pub glab_installed: bool,
}

pub fn tools_status(store: &Store, exe: &Path) -> ToolsStatus {
    let ours = helper_value(exe);
    let mut hosts: Vec<&str> = store.accounts.iter().map(|a| a.host.as_str()).collect();
    hosts.sort();
    hosts.dedup();
    let helper = hosts
        .iter()
        .map(|h| {
            let v = run("git", &["config", "--global", "--get-all", &format!("credential.https://{h}.helper")], None).unwrap_or_default();
            (h.to_string(), v.lines().last().map(str::trim) == Some(ours.as_str()))
        })
        .collect();
    let gh = have("gh").then(|| {
        let out = Command::new("gh").args(["api", "user", "--jq", ".login"]).stdin(Stdio::null()).output().ok();
        out.filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty())
    });
    ToolsStatus { helper, gh, glab_installed: have("glab") }
}
