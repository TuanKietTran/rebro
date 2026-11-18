mod tui;

use std::io::{self, BufRead, IsTerminal};

use anyhow::Context;
use clap::{Parser, Subcommand};
use rebro_core::{model::days_until, sync, Forge, Store};

#[derive(Parser)]
#[command(name = "rebro", version, about = "One place for every forge account: GitHub, GitLab, Codeberg/Forgejo, Gitea")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sign in with your browser (OAuth). The easy way: no token to create or paste.
    Login {
        /// github | gitlab | codeberg | forgejo | gitea (guessed from --host if omitted)
        #[arg(long)]
        forge: Option<String>,
        #[arg(long)]
        host: Option<String>,
        /// Your own OAuth app's client id (self-hosted GitLab, GitHub Enterprise)
        #[arg(long)]
        client_id: Option<String>,
        #[arg(long)]
        no_sync: bool,
    },
    /// Add (or replace) an account with a token you created. Prompts for the token without echo.
    Add {
        /// github | gitlab | codeberg | forgejo | gitea (guessed from --host if omitted)
        #[arg(long)]
        forge: Option<String>,
        /// e.g. github.com, gitlab.example.com (default: the forge's public host)
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        label: Option<String>,
        /// Read the token from stdin instead of prompting
        #[arg(long)]
        token_stdin: bool,
        /// Open the forge's token page in the browser first
        #[arg(long)]
        open: bool,
        /// Don't wire git/gh/glab afterwards
        #[arg(long)]
        no_sync: bool,
    },
    /// Import the credentials this computer already has: gh, glab, tea, git's credential helpers
    /// (Git Credential Manager, macOS Keychain, …), ~/.git-credentials and GH_TOKEN/GITLAB_TOKEN.
    /// Each token is verified with its forge first; tokens are never printed.
    Import {
        /// Find and verify, but store nothing
        #[arg(long)]
        dry_run: bool,
        /// Also look for credentials for these hosts (the forges' public hosts are always tried)
        #[arg(long = "host")]
        hosts: Vec<String>,
        #[arg(long)]
        json: bool,
        /// Don't wire git/gh/glab afterwards
        #[arg(long)]
        no_sync: bool,
    },
    /// List accounts with their last check
    Ls {
        #[arg(long)]
        json: bool,
    },
    /// Verify every token against its forge (identity, scopes, expiry)
    Check {
        #[arg(long)]
        json: bool,
    },
    /// Remove an account (and its token) by host/user, user or host
    Rm { account: String },
    /// Make an account the default for its host
    Default { account: String },
    /// Route repos of these owners (users/orgs/groups) to an account; no owners = clear
    Owners { account: String, owners: Vec<String> },
    /// Make git use rebro for every host, and give gh/glab the default tokens
    Sync {
        #[arg(long)]
        json: bool,
    },
    /// Check that git, gh and the credential store are wired correctly
    Doctor,
    /// Terminal UI
    Tui,
    /// Desktop UI (GPUI)
    Gui,
    /// git credential helper protocol (configured by `rebro sync`)
    #[command(hide = true)]
    GitCredential { action: String },
}

fn main() {
    rebro_core::extend_path(); // git may run us with a bare PATH; gh/glab still need to be found
    quiet_panics();
    if let Err(e) = run() {
        eprintln!("rebro: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut store = Store::load()?;
    match cli.cmd.unwrap_or(Cmd::Tui) {
        Cmd::GitCredential { action } => {
            let stdin = io::stdin();
            rebro_core::credential::handle(&action, stdin.lock(), io::stdout().lock(), &mut store)?;
        }
        Cmd::Login { forge, host, client_id, no_sync } => {
            let forge = pick_forge(forge.as_deref(), host.as_deref())?;
            let host = host.unwrap_or_else(|| forge.default_host().to_string());
            eprintln!("Signing in to {host} ({forge})…");
            let acct = rebro_core::login(forge, &host, client_id, &show_prompt, &|| false)?;
            println!("signed in as {} ({})", acct.key(), acct.health.as_ref().and_then(|h| h.token_kind.clone()).unwrap_or_default());
            if !no_sync {
                let store = Store::load()?;
                print_steps(&sync::sync_all(&store, &std::env::current_exe()?), false);
            }
        }
        Cmd::Add { forge, host, label, token_stdin, open, no_sync } => {
            let forge = pick_forge(forge.as_deref(), host.as_deref())?;
            let host = host.unwrap_or_else(|| forge.default_host().to_string());
            let url = forge.token_url(&host);
            if open {
                let _ = crate::open_url(&url);
            }
            let token = if token_stdin || !io::stdin().is_terminal() {
                let mut s = String::new();
                io::stdin().lock().read_line(&mut s)?;
                s
            } else {
                eprintln!("Create a token at:\n  {url}");
                if forge == Forge::GitHub {
                    eprintln!("  (classic token, Expiration: No expiration, scopes repo, read:org, workflow, gist)");
                }
                rpassword::prompt_password(format!("{} token for {host} (hidden): ", forge.name()))?
            };
            let acct = rebro_core::add_account(&mut store, forge, &host, &token, label)?;
            let h = acct.health.clone().unwrap_or_default();
            println!("added {} ({}), {}, expires {}", acct.key(), acct.forge, h.token_kind.unwrap_or_default(), h.expires.unwrap_or_else(|| "?".into()));
            for w in h.warnings {
                println!("  warning: {w}");
            }
            if !no_sync {
                print_steps(&sync::sync_all(&store, &std::env::current_exe()?), false);
            }
        }
        Cmd::Import { dry_run, hosts, json, no_sync } => {
            use rebro_core::import;
            let mut extra: Vec<String> = store.accounts.iter().map(|a| a.host.clone()).collect();
            extra.extend(hosts);
            if !json {
                eprintln!("Looking for credentials (gh, glab, tea, git helpers, ~/.git-credentials, env)…");
            }
            let found = import::discover(&extra);
            let outcomes = import::import(&mut store, found, dry_run)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&outcomes)?);
            } else {
                for o in &outcomes {
                    let what = match o.result {
                        import::Result_::Added if dry_run => "would add",
                        import::Result_::Updated if dry_run => "would update",
                        import::Result_::Added => "added",
                        import::Result_::Updated => "updated",
                        import::Result_::AlreadyThere => "have it",
                        import::Result_::Duplicate => "dup",
                        import::Result_::Failed => "rejected",
                    };
                    let who = o.account.clone().unwrap_or_else(|| o.host.clone());
                    let detail = if o.detail.is_empty() { String::new() } else { format!("  ({})", o.detail) };
                    println!("{what:<12} {who:<36} from {}{detail}", o.source);
                }
                println!("{}{}", if dry_run { "dry run: " } else { "" }, import::summary(&outcomes));
            }
            let changed = outcomes.iter().any(|o| matches!(o.result, import::Result_::Added | import::Result_::Updated));
            if changed && !dry_run && !no_sync {
                let store = Store::load()?;
                print_steps(&sync::sync_all(&store, &std::env::current_exe()?), json);
            }
        }
        Cmd::Ls { json } => print_accounts(&store.accounts, json),
        Cmd::Check { json } => {
            let accts = rebro_core::check_all(&mut store);
            print_accounts(&accts, json);
            if accts.iter().any(|a| !a.health.as_ref().is_some_and(|h| h.ok)) {
                std::process::exit(2);
            }
        }
        Cmd::Rm { account } => {
            rebro_core::remove_account(&mut store, &account)?;
            println!("removed {account}");
        }
        Cmd::Default { account } => {
            let key = store.find(&account).with_context(|| format!("no account {account}"))?.key();
            rebro_core::set_default(&mut store, &key)?;
            println!("{key} is now the default for its host; run `rebro sync` to hand it to gh/glab");
        }
        Cmd::Owners { account, owners } => {
            let key = store.find(&account).with_context(|| format!("no account {account}"))?.key();
            if let Some(a) = store.accounts.iter_mut().find(|a| a.key() == key) {
                a.owners = owners.clone();
            }
            store.save()?;
            println!("{key}: owners = [{}]", owners.join(", "));
        }
        Cmd::Sync { json } => print_steps(&sync::sync_all(&store, &std::env::current_exe()?), json),
        Cmd::Doctor => doctor(&store)?,
        Cmd::Tui => tui::run(store)?,
        Cmd::Gui => {
            let gui = std::env::current_exe()?.with_file_name(if cfg!(windows) { "rebro-gui.exe" } else { "rebro-gui" });
            std::process::Command::new(&gui).spawn().with_context(|| format!("starting {}", gui.display()))?;
        }
    }
    Ok(())
}

/// `rebro ls | head` closes the pipe early and println! panics: exit quietly like other CLIs.
/// Any other panic gets one readable line instead of a Rust backtrace dump.
fn quiet_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        // "Broken pipe" on unix, os error 232 ("The pipe is being closed") on Windows
        if msg.contains("Broken pipe") || msg.contains("os error 232") || msg.contains("os error 32)") {
            std::process::exit(0);
        }
        if std::env::var_os("RUST_BACKTRACE").is_some() {
            default(info);
        } else {
            let at = info.location().map(|l| format!(" ({}:{})", l.file(), l.line())).unwrap_or_default();
            eprintln!("rebro: internal error{at}: {msg}
(please report; RUST_BACKTRACE=1 for details)");
        }
    }));
}

fn pick_forge(forge: Option<&str>, host: Option<&str>) -> anyhow::Result<Forge> {
    Ok(match (forge, host) {
        (Some(f), _) => Forge::parse(f).with_context(|| format!("unknown forge {f}"))?,
        (None, Some(h)) => Forge::guess(h),
        (None, None) => Forge::GitHub,
    })
}

/// Open the browser for the user and say what to do; device codes also go on the clipboard.
pub fn show_prompt(p: rebro_core::LoginPrompt) {
    match p {
        rebro_core::LoginPrompt::DeviceCode { user_code, url, url_with_code } => {
            copy_to_clipboard(&user_code);
            eprintln!("\n  Your code: {user_code}   (copied to the clipboard)\n  Approve at {url}\n  Waiting for approval…");
            let _ = crate::open_url(url_with_code.as_deref().unwrap_or(&url));
        }
        rebro_core::LoginPrompt::Browser { url } => {
            eprintln!("\n  Approve rebro in the browser that just opened.\n  (If it didn't: {url})\n  Waiting for approval…");
            let _ = crate::open_url(&url);
        }
    }
}

/// Open a URL in the default browser (REBRO_NO_BROWSER=1 skips it, for automated tests).
pub fn open_url(url: &str) -> std::io::Result<()> {
    if std::env::var("REBRO_NO_BROWSER").is_ok_and(|v| v == "1") {
        return Ok(());
    }
    webbrowser::open(url)
}

/// Same as show_prompt but without printing (the TUI owns the screen).
pub fn show_prompt_quiet(p: rebro_core::LoginPrompt) {
    match p {
        rebro_core::LoginPrompt::DeviceCode { user_code, url, url_with_code } => {
            copy_to_clipboard(&user_code);
            let _ = webbrowser::open(url_with_code.as_deref().unwrap_or(&url));
        }
        rebro_core::LoginPrompt::Browser { url } => {
            let _ = webbrowser::open(&url);
        }
    }
}

pub fn copy_to_clipboard(text: &str) {
    #[cfg(windows)]
    {
        use std::io::Write;
        if let Ok(mut child) = std::process::Command::new("clip").stdin(std::process::Stdio::piped()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }
    #[cfg(not(windows))]
    {
        use std::io::Write;
        // macOS: pbcopy; Linux: Wayland, then X11
        let tools: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
            &[("pbcopy", &[])]
        } else {
            &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"]), ("xsel", &["--clipboard", "--input"])]
        };
        for (cmd, args) in tools {
            let Ok(mut child) = std::process::Command::new(cmd)
                .args(*args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            else {
                continue;
            };
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().is_ok_and(|s| s.success()) {
                break;
            }
        }
    }
}

fn print_accounts(accts: &[rebro_core::Account], json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(accts).unwrap_or_default());
        return;
    }
    if accts.is_empty() {
        println!("no accounts yet: rebro import   (credentials gh/git already have)   or   rebro login --forge github");
        return;
    }
    println!("{:<8} {:<34} {:<9} {:<7} {:<12} {}", "forge", "account", "default", "status", "expires", "notes");
    for a in accts {
        let h = a.health.clone().unwrap_or_default();
        let exp = h.expires.clone().unwrap_or_else(|| "-".into());
        let exp = match days_until(&exp) {
            Some(d) if d >= 0 => format!("{exp} ({d}d)"),
            _ => exp,
        };
        let notes = h.error.clone().into_iter().chain(h.warnings.clone()).collect::<Vec<_>>().join("; ");
        println!(
            "{:<8} {:<34} {:<9} {:<7} {:<12} {}",
            a.forge.short(),
            a.key(),
            if a.default { "yes" } else { "" },
            if a.health.is_some() { h.summary() } else { "?".into() },
            exp,
            notes
        );
    }
}

fn print_steps(steps: &[sync::Step], json: bool) {
    if json {
        let v: Vec<_> = steps.iter().map(|s| serde_json::json!({"what": s.what, "ok": s.ok, "detail": s.detail})).collect();
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return;
    }
    for s in steps {
        println!("{} {}: {}", if s.ok { "ok  " } else { "FAIL" }, s.what, s.detail);
    }
}

fn doctor(store: &Store) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let ours = sync::helper_value(&exe);
    println!("accounts file: {}", store.path.display());
    let mut hosts: Vec<&str> = store.accounts.iter().map(|a| a.host.as_str()).collect();
    hosts.dedup();
    for host in hosts {
        let out = std::process::Command::new("git")
            .args(["config", "--global", "--get-all", &format!("credential.https://{host}.helper")])
            .output()?;
        let vals = String::from_utf8_lossy(&out.stdout).to_string();
        let ok = vals.lines().last().map(str::trim) == Some(ours.as_str()) && vals.lines().next() == Some("");
        println!("{} git helper for {host}{}", if ok { "ok  " } else { "FAIL" }, if ok { "" } else { "  (run: rebro sync)" });
    }
    for a in &store.accounts {
        let ok = rebro_core::store::get_secret(&a.key()).is_ok();
        println!("{} credential store has {}", if ok { "ok  " } else { "FAIL" }, a.key());
    }
    if let Ok(o) = std::process::Command::new("gh").args(["auth", "status"]).output() {
        let text = String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr);
        println!("{} gh auth status", if o.status.success() { "ok  " } else { "FAIL" });
        for l in text.lines().filter(|l| l.contains("Logged in") || l.contains("Failed") || l.contains("invalid")) {
            println!("       {}", l.trim());
        }
    }
    Ok(())
}
