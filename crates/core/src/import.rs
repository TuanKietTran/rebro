//! Import the forge credentials this computer already has, so nobody has to sign in again:
//! gh, glab, tea, git's credential helpers (Git Credential Manager, macOS Keychain, libsecret, cache),
//! `~/.git-credentials` and the usual environment variables.
//!
//! Every token found is verified against its forge before it is kept; the token itself is never
//! printed, logged or serialized (`Found` has no Serialize, `Outcome` holds no secret).

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::model::{normalize_host, Forge};
use crate::store::Store;

/// One credential found on this machine. Holds the secret: never print or serialize it.
#[derive(Clone)]
pub struct Found {
    /// Where it came from, e.g. "gh", "glab", "tea", "git (manager)", "~/.git-credentials", "env GH_TOKEN".
    pub source: String,
    pub forge: Forge,
    pub host: String,
    pub token: String,
    /// Something the user should know about this token (e.g. "GitLab OAuth from glab: expires in 2 h").
    pub note: Option<String>,
}

impl std::fmt::Debug for Found {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Found").field("source", &self.source).field("host", &self.host).field("token", &"<redacted>").finish()
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Result_ {
    /// Stored as a new account.
    Added,
    /// The account existed but had no working token on this machine; now it has.
    Updated,
    /// rebro already has this exact token, or a working one for the same account.
    AlreadyThere,
    /// Another source had a better token for the same account.
    Duplicate,
    /// The forge rejected it (revoked, expired) or could not be reached.
    Failed,
}

/// What happened to one found credential. Safe to print and serialize.
#[derive(Clone, Debug, Serialize)]
pub struct Outcome {
    pub source: String,
    pub host: String,
    pub forge: Forge,
    /// host/user once verified
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub result: Result_,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

// ---------- discovery ----------

/// Everything this machine has, deduplicated by (host, token). No network.
pub fn discover(extra_hosts: &[String]) -> Vec<Found> {
    let mut all = vec![];
    all.extend(from_env());
    all.extend(from_gh());
    all.extend(from_glab());
    all.extend(from_tea());
    all.extend(from_credentials_files());
    let mut hosts: BTreeSet<String> = Forge::ALL.iter().map(|f| f.default_host().to_string()).collect();
    hosts.extend(extra_hosts.iter().map(|h| normalize_host(h)));
    hosts.extend(all.iter().map(|f| f.host.clone()));
    all.extend(from_git_helpers(&hosts));
    let mut seen = BTreeSet::new();
    all.retain(|f| !f.token.is_empty() && seen.insert((f.host.clone(), f.token.clone())));
    all
}

fn found(source: impl Into<String>, forge: Forge, host: &str, token: &str) -> Found {
    Found { source: source.into(), forge, host: normalize_host(host), token: token.trim().to_string(), note: None }
}

fn from_env() -> Vec<Found> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let mut v = vec![];
    let gh_host = var("GH_HOST").unwrap_or_else(|| "github.com".into());
    for k in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Some(t) = var(k) {
            v.push(found(format!("env {k}"), Forge::GitHub, "github.com", &t));
        }
    }
    for k in ["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"] {
        if let (Some(t), true) = (var(k), gh_host != "github.com") {
            v.push(found(format!("env {k}"), Forge::GitHub, &gh_host, &t));
        }
    }
    let gl_host = var("GITLAB_HOST").or_else(|| var("GL_HOST")).unwrap_or_else(|| "gitlab.com".into());
    for k in ["GITLAB_TOKEN", "GLAB_TOKEN"] {
        if let Some(t) = var(k) {
            v.push(found(format!("env {k}"), Forge::GitLab, &gl_host, &t));
        }
    }
    if let Some(t) = var("CODEBERG_TOKEN") {
        v.push(found("env CODEBERG_TOKEN", Forge::Forgejo, "codeberg.org", &t));
    }
    v
}

/// Run a tool with a time limit; stdout on success. Never inherits the console (no prompts).
fn tool(cmd: &str, args: &[&str], stdin: Option<&str>, envs: &[(&str, &str)], unset: &[&str]) -> Option<String> {
    let mut c = Command::new(cmd);
    c.args(args).stdout(Stdio::piped()).stderr(Stdio::null()).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    for (k, v) in envs {
        c.env(k, v);
    }
    for k in unset {
        c.env_remove(k);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = c.spawn().ok()?;
    if let Some(input) = stdin {
        let _ = child.stdin.take()?.write_all(input.as_bytes());
    }
    // a helper that decides to wait for a human must not hang the import
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                return status.success().then_some(out);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn from_gh() -> Vec<Found> {
    let mut v = vec![];
    // gh ≥ 2.6x: every account on every host
    if let Some(json) = tool("gh", &["auth", "status", "--json", "hosts"], None, &[("GH_PROMPT_DISABLED", "1")], &["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"]) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json) {
            if let Some(hosts) = val["hosts"].as_object() {
                for (host, accts) in hosts {
                    for a in accts.as_array().into_iter().flatten() {
                        let Some(login) = a["login"].as_str() else { continue };
                        if let Some(t) = gh_token(host, Some(login)) {
                            v.push(found(format!("gh ({login})"), Forge::GitHub, host, &t));
                        }
                    }
                }
                return v;
            }
        }
    }
    // older gh: the active account per known host
    if let Some(t) = gh_token("github.com", None) {
        v.push(found("gh", Forge::GitHub, "github.com", &t));
    }
    v
}

fn gh_token(host: &str, user: Option<&str>) -> Option<String> {
    let mut args = vec!["auth", "token", "--hostname", host];
    if let Some(u) = user {
        args.extend(["--user", u]);
    }
    // env tokens would shadow the stored ones; we want what gh stored
    tool("gh", &args, None, &[("GH_PROMPT_DISABLED", "1")], &["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Config dirs a CLI may use: XDG-style ~/.config/<app> everywhere, plus the platform's own
/// (macOS ~/Library/Application Support/<app>, Windows %APPDATA%\<app>).
fn config_candidates(app: &str, env_override: &str) -> Vec<PathBuf> {
    let mut v = vec![];
    if let Ok(p) = std::env::var(env_override) {
        v.push(PathBuf::from(p));
    }
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        v.push(PathBuf::from(x).join(app));
    }
    if let Some(h) = dirs::home_dir() {
        v.push(h.join(".config").join(app));
    }
    if let Some(c) = dirs::config_dir() {
        v.push(c.join(app));
    }
    v.dedup();
    v
}

/// Minimal YAML reader for the two flat configs we need (glab, tea): `key: value` lines with
/// indentation, lists of maps with "- ". Returns (indent, key, value) in order; values unquoted.
fn yaml_lines(text: &str) -> Vec<(usize, bool, String, String)> {
    let mut out = vec![];
    for raw in text.lines() {
        let trimmed = raw.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut indent = raw.len() - trimmed.len();
        let (item, rest) = match trimmed.strip_prefix("- ") {
            Some(r) => {
                indent += 2;
                (true, r)
            }
            None => (false, trimmed),
        };
        let Some((k, v)) = rest.split_once(':') else { continue };
        let v = v.trim();
        let v = v.split(" #").next().unwrap_or(v).trim();
        let v = v.trim_matches('"').trim_matches('\'');
        out.push((indent, item, k.trim().trim_matches('"').to_string(), v.to_string()));
    }
    out
}

fn from_glab() -> Vec<Found> {
    let mut v = vec![];
    let have_glab = tool("glab", &["--version"], None, &[], &[]).is_some();
    for dir in config_candidates("glab-cli", "GLAB_CONFIG_DIR") {
        let Ok(text) = std::fs::read_to_string(dir.join("config.yml")) else { continue };
        // hosts:
        //     gitlab.com:
        //         token: glpat-…          (absent when glab keeps it in the keyring)
        //         is_oauth2: "true"
        let lines = yaml_lines(&text);
        let Some(start) = lines.iter().position(|(i, _, k, _)| *i == 0 && k == "hosts") else { continue };
        let mut host: Option<(String, usize)> = None;
        let mut entries: Vec<(String, Option<String>, bool)> = vec![];
        for (indent, _, k, val) in &lines[start + 1..] {
            if *indent == 0 {
                break;
            }
            match &host {
                Some((_, hi)) if indent > hi => {
                    let Some(e) = entries.last_mut() else { continue };
                    match k.as_str() {
                        "token" if !val.is_empty() => e.1 = Some(val.clone()),
                        "is_oauth2" => e.2 = val == "true",
                        _ => {}
                    }
                }
                _ => {
                    host = Some((k.clone(), *indent));
                    entries.push((k.clone(), None, false));
                }
            }
        }
        for (h, token, oauth) in entries {
            let token = token.or_else(|| {
                have_glab.then(|| tool("glab", &["config", "get", "token", "--host", &h], None, &[], &["GITLAB_TOKEN", "GLAB_TOKEN"])).flatten().map(|s| s.trim().to_string())
            });
            if let Some(t) = token.filter(|t| !t.is_empty()) {
                let mut f = found("glab", Forge::GitLab, &h, &t);
                if oauth {
                    f.note = Some("glab's OAuth sign-in: expires within 2 hours; sign in with rebro to keep it".into());
                }
                v.push(f);
            }
        }
    }
    v
}

fn from_tea() -> Vec<Found> {
    let mut v = vec![];
    for dir in config_candidates("tea", "TEA_CONFIG_DIR") {
        let Ok(text) = std::fs::read_to_string(dir.join("config.yml")) else { continue };
        // logins:
        //     - name: codeberg
        //       url: https://codeberg.org
        //       token: …
        let mut cur: (Option<String>, Option<String>) = (None, None);
        let flush = |cur: &mut (Option<String>, Option<String>), v: &mut Vec<Found>| {
            if let (Some(url), Some(token)) = (cur.0.take(), cur.1.take()) {
                let host = normalize_host(&url);
                v.push(found("tea", Forge::guess(&host), &host, &token));
            }
            *cur = (None, None);
        };
        for (_, item, k, val) in yaml_lines(&text) {
            if item {
                flush(&mut cur, &mut v);
            }
            match k.as_str() {
                "url" => cur.0 = Some(val),
                "token" if !val.is_empty() => cur.1 = Some(val),
                _ => {}
            }
        }
        flush(&mut cur, &mut v);
    }
    v
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            if let Some(n) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(n);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// git's `store` helper files: one `https://user:token@host[/path]` per line.
fn parse_credentials_file(text: &str, source: &str) -> Vec<Found> {
    text.lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("https://")?;
            let (userinfo, hostpath) = rest.rsplit_once('@')?;
            let (_, pass) = userinfo.split_once(':')?;
            let host = normalize_host(hostpath);
            Some(found(source, Forge::guess(&host), &host, &pct_decode(pass)))
        })
        .collect()
}

fn from_credentials_files() -> Vec<Found> {
    let mut paths = vec![];
    // git reads $HOME first (also Git for Windows), so do we
    if let Some(h) = std::env::var_os("HOME").map(PathBuf::from).or_else(dirs::home_dir) {
        paths.push((h.join(".git-credentials"), "~/.git-credentials"));
        paths.push((h.join(".config/git/credentials"), "~/.config/git/credentials"));
    }
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        paths.push((PathBuf::from(x).join("git/credentials"), "$XDG_CONFIG_HOME/git/credentials"));
    }
    let mut v = vec![];
    for (p, label) in paths {
        if let Ok(text) = std::fs::read_to_string(&p) {
            v.extend(parse_credentials_file(&text, label));
        }
    }
    v
}

/// The helpers git would use besides rebro and gh (both covered elsewhere) and `store` (read
/// directly): Git Credential Manager, osxkeychain, libsecret, wincred, cache, …
fn other_helpers() -> Vec<String> {
    let mut v: Vec<String> = tool("git", &["config", "--get-all", "credential.helper"], None, &[], &[])
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|h| !h.is_empty() && !h.contains("rebro") && !h.contains("auth git-credential") && !(h == "store" || h.starts_with("store ")))
        .collect();
    // macOS: Apple's git sets osxkeychain in a gitconfig that other gits don't read
    if cfg!(target_os = "macos") {
        v.push("osxkeychain".into());
    }
    let mut seen = BTreeSet::new();
    v.retain(|h| seen.insert(h.clone()));
    v
}

/// "manager", "osxkeychain", "/opt/x/git-credential-foo --a" -> "git-credential-foo", "!sh…" -> "custom helper".
fn helper_label(helper: &str) -> String {
    if helper.starts_with('!') {
        return "custom helper".into();
    }
    // the program: a quoted path (may hold spaces) or the first word
    let first = match helper.chars().next() {
        Some(q @ ('\'' | '"')) => helper[1..].split(q).next().unwrap_or(helper),
        _ => helper.split_whitespace().next().unwrap_or(helper),
    };
    first.rsplit(['/', '\\']).next().unwrap_or(first).trim_end_matches(".exe").to_string()
}

fn from_git_helpers(hosts: &BTreeSet<String>) -> Vec<Found> {
    let helpers = other_helpers();
    let mut v = vec![];
    for helper in &helpers {
        let label = format!("git ({})", helper_label(helper));
        for host in hosts {
            // `credential.helper=` (empty) clears every helper git collected so far, url-specific ones
            // included, so only `helper` is asked. Nothing may prompt: no terminal, no askpass, no GCM UI.
            let reset = "credential.helper=".to_string();
            let only = format!("credential.helper={helper}");
            let args = ["-c", &reset, "-c", &only, "-c", "credential.interactive=false", "-c", "core.askPass=", "credential", "fill"];
            let input = format!("protocol=https\nhost={host}\n\n");
            let out = tool(
                "git",
                &args,
                Some(&input),
                &[("GIT_TERMINAL_PROMPT", "0"), ("GCM_INTERACTIVE", "never"), ("GCM_GUI_PROMPT", "false")],
                &["GIT_ASKPASS", "SSH_ASKPASS"],
            );
            let Some(out) = out else { continue };
            let pass = out.lines().find_map(|l| l.strip_prefix("password=")).unwrap_or("");
            if !pass.is_empty() {
                v.push(found(label.clone(), Forge::guess(host), host, pass));
            }
        }
    }
    v
}

// ---------- verify + store ----------

/// How good a verified token is for keeping: fewer warnings first, then no expiry.
fn rank(h: &crate::model::Health) -> (usize, bool) {
    (h.warnings.len(), h.expires.as_deref() != Some("never"))
}

/// Verify every found credential and store the best working token per account.
/// Accounts that already work in rebro are left alone. `dry_run` verifies but stores nothing.
pub fn import(store: &mut Store, found: Vec<Found>, dry_run: bool) -> anyhow::Result<Vec<Outcome>> {
    // verify in parallel: a dozen forge round-trips one after another is slow
    let verified: Vec<(Found, anyhow::Result<crate::api::Identity>)> = std::thread::scope(|s| {
        let handles: Vec<_> = found
            .into_iter()
            .map(|f| s.spawn(move || {
                let r = crate::guarded(|| crate::api::verify(f.forge, &f.host, &f.token));
                (f, r)
            }))
            .collect();
        // guarded() already turned panics into errors, so join() can't fail; skip it if it somehow does
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });

    let mut outcomes = vec![];
    let mut ok: Vec<(Found, crate::api::Identity)> = vec![];
    for (f, r) in verified {
        match r {
            Ok(id) => ok.push((f, id)),
            Err(e) => outcomes.push(Outcome {
                source: f.source,
                host: f.host,
                forge: f.forge,
                account: None,
                result: Result_::Failed,
                detail: format!("{e:#}"),
            }),
        }
    }
    // best first, so the first one seen per account is the keeper
    ok.sort_by_key(|(_, id)| rank(&crate::model::Health::from_identity(id)));

    let mut decided: Vec<String> = vec![];
    for (f, id) in ok {
        let key = format!("{}/{}", f.host, id.username);
        let lkey = key.to_ascii_lowercase();
        let mut out = Outcome { source: f.source.clone(), host: f.host.clone(), forge: f.forge, account: Some(key.clone()), result: Result_::Added, detail: f.note.clone().unwrap_or_default() };
        if decided.contains(&lkey) {
            out.result = Result_::Duplicate;
            out.detail = "a better token for this account was found elsewhere".into();
            outcomes.push(out);
            continue;
        }
        decided.push(lkey);
        let existing = store.accounts.iter().find(|a| a.key().eq_ignore_ascii_case(&key)).cloned();
        if let Some(a) = &existing {
            let current = crate::store::get_secret(&a.key()).ok();
            if current.as_deref() == Some(f.token.as_str()) {
                out.result = Result_::AlreadyThere;
                out.detail = "same token".into();
                outcomes.push(out);
                continue;
            }
            // keep what rebro has when it still works (e.g. its own refreshable sign-in)
            let works = current.is_some() && crate::access_token(store, &a.key()).ok().is_some_and(|t| crate::api::verify(a.forge, &a.host, &t).is_ok());
            if works {
                out.result = Result_::AlreadyThere;
                out.detail = "rebro's own token still works; kept it".into();
                outcomes.push(out);
                continue;
            }
            out.result = Result_::Updated;
        }
        if !dry_run {
            let forge = existing.as_ref().map(|a| a.forge).unwrap_or(f.forge);
            crate::store_verified(store, forge, &f.host, &f.token, &id, None, None)?;
        }
        if out.detail.is_empty() {
            out.detail = id.warnings.join("; ");
        }
        outcomes.push(out);
    }
    Ok(outcomes)
}

/// Discover and import in one go (what the UIs call).
pub fn import_all(store: &mut Store, dry_run: bool) -> anyhow::Result<Vec<Outcome>> {
    let extra: Vec<String> = store.accounts.iter().map(|a| a.host.clone()).collect();
    import(store, discover(&extra), dry_run)
}

/// One-line summary for a status bar.
pub fn summary(outcomes: &[Outcome]) -> String {
    let n = |r: Result_| outcomes.iter().filter(|o| o.result == r).count();
    let (added, updated, there, failed) = (n(Result_::Added), n(Result_::Updated), n(Result_::AlreadyThere), n(Result_::Failed));
    if outcomes.is_empty() {
        return "no credentials found on this computer".into();
    }
    let mut parts = vec![];
    if added > 0 {
        parts.push(format!("{added} added"));
    }
    if updated > 0 {
        parts.push(format!("{updated} updated"));
    }
    if there > 0 {
        parts.push(format!("{there} already in rebro"));
    }
    if failed > 0 {
        parts.push(format!("{failed} rejected by the forge"));
    }
    if parts.is_empty() {
        parts.push("only duplicates found".into());
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_file_lines() {
        let f = parse_credentials_file(
            "https://me:ghp_abc@github.com\nhttps://oauth2:glpat%2Dx@gitlab.example.com/group\n# junk\nhttp://x:y@insecure.org\n",
            "~/.git-credentials",
        );
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].host.as_str(), f[0].forge, f[0].token.as_str()), ("github.com", Forge::GitHub, "ghp_abc"));
        assert_eq!((f[1].host.as_str(), f[1].forge, f[1].token.as_str()), ("gitlab.example.com", Forge::GitLab, "glpat-x"));
    }

    #[test]
    fn yaml_reader_handles_glab_and_tea_shapes() {
        let glab = "git_protocol: ssh\nhosts:\n    gitlab.com:\n        token: glpat-1\n        is_oauth2: \"true\"\n    git.acme.io:\n        api_protocol: https\n";
        let l = yaml_lines(glab);
        assert!(l.contains(&(4, false, "gitlab.com".into(), "".into())));
        assert!(l.contains(&(8, false, "token".into(), "glpat-1".into())));
        let tea = "logins:\n    - name: cb\n      url: https://codeberg.org\n      token: abc\n    - name: g\n      url: https://gitea.com\n      token: def\n";
        let l = yaml_lines(tea);
        assert_eq!(l.iter().filter(|x| x.1).count(), 2);
    }

    #[test]
    fn helper_labels() {
        assert_eq!(helper_label("manager"), "manager");
        assert_eq!(helper_label("'C:/Program Files/Git/mingw64/bin/git-credential-manager.exe' --x"), "git-credential-manager");
        assert_eq!(helper_label("!f() { echo; }; f"), "custom helper");
    }

    #[test]
    fn found_debug_hides_the_token() {
        let f = found("x", Forge::GitHub, "github.com", "ghp_secret");
        assert!(!format!("{f:?}").contains("ghp_secret"));
    }

    #[test]
    fn summary_reads_well() {
        let o = |r| Outcome { source: "gh".into(), host: "github.com".into(), forge: Forge::GitHub, account: None, result: r, detail: String::new() };
        assert_eq!(summary(&[o(Result_::Added), o(Result_::AlreadyThere), o(Result_::Failed)]), "1 added, 1 already in rebro, 1 rejected by the forge");
        assert_eq!(summary(&[]), "no credentials found on this computer");
    }
}
