//! rebro core: one place for every forge account (GitHub, GitLab, Codeberg/Forgejo, Gitea).
//!
//! - Tokens live in the OS credential store (Windows Credential Manager, macOS Keychain, Secret Service).
//! - Non-secret metadata lives in `accounts.toml` under the user config dir.
//! - `rebro` serves git as a credential helper and hands tokens to `gh` / `glab`, so no tool
//!   needs its own login (and `gh`'s OAuth tokens can no longer be revoked out from under you).

pub mod api;
pub mod credential;
pub mod import;
pub mod model;
pub mod oauth;
pub mod store;
pub mod sync;

pub use model::{Account, Forge, Health};
pub use store::Store;

/// Run `f`, turning a panic into an error message instead of tearing down the thread (or the
/// app). Used at every worker boundary: sign-in threads, the GUI's background jobs, import.
pub fn catch_panic<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|p| {
        let msg = p
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        format!("internal error (please report): {msg}")
    })
}

/// `catch_panic` for fallible work: a panic becomes an ordinary `Err`.
pub fn guarded<T>(f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    catch_panic(f).map_err(|e| anyhow::anyhow!(e))?
}

/// Apps started from Finder, the Dock or a desktop launcher get a bare PATH (macOS:
/// /usr/bin:/bin:/usr/sbin:/sbin), so gh, glab and tea from Homebrew, MacPorts, Nix or ~/.local/bin
/// would look missing to sync and import. Append the usual install dirs that exist.
/// Call first thing in main, before any thread starts (it sets an environment variable).
pub fn extend_path() {
    if !cfg!(unix) {
        return;
    }
    let cur = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<std::path::PathBuf> = std::env::split_paths(&cur).collect();
    let home = dirs::home_dir().unwrap_or_default();
    let extra = [
        "/opt/homebrew/bin".into(),
        "/usr/local/bin".into(),
        "/opt/local/bin".into(),
        "/home/linuxbrew/.linuxbrew/bin".into(),
        "/run/current-system/sw/bin".into(),
        home.join(".local/bin"),
        home.join(".cargo/bin"),
        home.join(".nix-profile/bin"),
    ];
    let before = dirs.len();
    for d in extra {
        if d.is_dir() && !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    if dirs.len() != before {
        if let Ok(p) = std::env::join_paths(dirs) {
            std::env::set_var("PATH", p);
        }
    }
}

/// What the user has to do during Sign in; the UI shows it and opens the browser.
#[derive(Clone, Debug)]
pub enum LoginPrompt {
    /// Enter `user_code` at `url` (GitHub, GitLab). `url_with_code` skips typing when offered.
    DeviceCode { user_code: String, url: String, url_with_code: Option<String> },
    /// Approve at `url`; the browser comes back to rebro on 127.0.0.1 (Gitea, Forgejo/Codeberg).
    Browser { url: String },
}

/// Sign in with OAuth and store the account. Blocks until approved, denied, expired or cancelled;
/// run it off the UI thread. `prompt` is called once, before waiting.
pub fn login(
    forge: Forge,
    host: &str,
    client_id: Option<String>,
    prompt: &dyn Fn(LoginPrompt),
    cancelled: &dyn Fn() -> bool,
) -> anyhow::Result<Account> {
    let host = model::normalize_host(host);
    let client = oauth::client_for(forge, &host, client_id.as_deref()).ok_or_else(|| {
        anyhow::anyhow!(
            "no built-in sign-in for {host}: register an OAuth app there (redirect URI http://127.0.0.1) and pass its client id, or paste a token instead"
        )
    })?;
    let tokens = if client.device_url.is_some() {
        let d = oauth::device_start(&client)?;
        prompt(LoginPrompt::DeviceCode {
            user_code: d.user_code.clone(),
            url: d.verification_uri.clone(),
            url_with_code: d.verification_uri_complete.clone(),
        });
        oauth::device_poll(&client, &d, cancelled)?
    } else {
        oauth::loopback_login(&client, &|url| prompt(LoginPrompt::Browser { url: url.to_string() }), cancelled)?
    };
    let mut store = Store::load()?;
    finish_login(&mut store, forge, &host, client_id, tokens)
}

/// Add or update an account from a pasted token: verifies it against the forge, stores the secret,
/// records metadata, and makes it the host default if it is the first account for that host.
pub fn add_account(store: &mut Store, forge: Forge, host: &str, token: &str, label: Option<String>) -> anyhow::Result<Account> {
    let token = token.trim();
    anyhow::ensure!(!token.is_empty(), "empty token");
    save_account(store, forge, host, token, label, None)
}

/// Store an account obtained with Sign in (OAuth). Refresh token goes in its own credential entry.
pub fn finish_login(
    store: &mut Store,
    forge: Forge,
    host: &str,
    client_id: Option<String>,
    t: oauth::TokenSet,
) -> anyhow::Result<Account> {
    let access = t.access_token.clone();
    save_account(store, forge, host, &access, None, Some((t, client_id)))
}

fn save_account(
    store: &mut Store,
    forge: Forge,
    host: &str,
    token: &str,
    label: Option<String>,
    oauth: Option<(oauth::TokenSet, Option<String>)>,
) -> anyhow::Result<Account> {
    let host = model::normalize_host(host);
    let id = api::verify(forge, &host, token)?;
    store_verified(store, forge, &host, token, &id, label, oauth)
}

/// Store a token already verified as `id` (import verifies in bulk first).
pub(crate) fn store_verified(
    store: &mut Store,
    forge: Forge,
    host: &str,
    token: &str,
    id: &api::Identity,
    label: Option<String>,
    oauth: Option<(oauth::TokenSet, Option<String>)>,
) -> anyhow::Result<Account> {
    let host = model::normalize_host(host);
    let first_on_host = !store.accounts.iter().any(|a| a.host == host);
    let mut acct = store
        .accounts
        .iter()
        .find(|a| a.host == host && a.username.eq_ignore_ascii_case(&id.username))
        .cloned()
        .unwrap_or_else(|| Account::new(forge, &host, &id.username));
    acct.forge = forge;
    acct.label = label.or(acct.label);
    acct.default = acct.default || first_on_host;
    store::set_secret(&acct.key(), token)?;
    match oauth {
        Some((t, client_id)) => {
            acct.auth = Some("oauth".into());
            acct.expires_at = t.expires_at;
            acct.client_id = client_id;
            match &t.refresh_token {
                Some(r) => store::set_secret(&store::refresh_key(&acct.key()), r)?,
                None => {
                    store::delete_secret(&store::refresh_key(&acct.key())).ok();
                }
            }
        }
        None => {
            acct.auth = None;
            acct.expires_at = None;
            store::delete_secret(&store::refresh_key(&acct.key())).ok();
        }
    }
    acct.health = Some(health_for(&acct, id));
    store.upsert(acct.clone());
    store.save()?;
    Ok(acct)
}

fn health_for(acct: &Account, id: &api::Identity) -> Health {
    let mut h = Health::from_identity(id);
    if acct.is_oauth() {
        // our own sign-in client, not gh's shared one: no revocation warning
        h.token_kind = Some("OAuth sign-in".into());
        h.warnings.retain(|w| !w.starts_with("OAuth app token"));
        if acct.expires_at.is_some() {
            h.expires = Some("auto-renewed".into());
        }
    }
    h
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The token to use right now: OAuth tokens that expire within 5 minutes are refreshed first
/// (and the new pair stored), so git never sees an expired token.
pub fn access_token(store: &mut Store, key: &str) -> anyhow::Result<String> {
    use anyhow::Context;
    let acct = store.find_key(key).cloned().ok_or_else(|| anyhow::anyhow!("no account {key}"))?;
    let token = store::get_secret(key).map_err(|e| match e.downcast_ref::<keyring::Error>() {
        // the account list can come from another machine (synced); its tokens don't
        Some(keyring::Error::NoEntry) => anyhow::anyhow!("not signed in on this machine yet: sign in to {key} here"),
        _ => e,
    })?;
    if !(acct.is_oauth() && acct.expires_at.is_some_and(|e| e <= now_secs() + 300)) {
        return Ok(token);
    }
    let rt = store::get_secret(&store::refresh_key(key)).context("token expired and no refresh token stored: sign in again")?;
    let client = oauth::client_for(acct.forge, &acct.host, acct.client_id.as_deref())
        .ok_or_else(|| anyhow::anyhow!("no OAuth client for {}", acct.host))?;
    let t = oauth::refresh(&client, &rt).context("refreshing the sign-in failed: sign in again")?;
    store::set_secret(key, &t.access_token)?;
    if let Some(r) = &t.refresh_token {
        store::set_secret(&store::refresh_key(key), r)?;
    }
    // look it up again: never trust an index across calls that can touch the store
    if let Some(a) = store.accounts.iter_mut().find(|a| a.key() == key) {
        a.expires_at = t.expires_at;
    }
    store.save()?;
    Ok(t.access_token)
}

/// Re-verify every account (refreshing OAuth tokens as needed) and record the result.
pub fn check_all(store: &mut Store) -> Vec<Account> {
    let keys: Vec<String> = store.accounts.iter().map(|a| a.key()).collect();
    for key in keys {
        let health = match access_token(store, &key) {
            Ok(token) => {
                match store.find_key(&key).cloned() {
                    Some(acct) => match api::verify(acct.forge, &acct.host, &token) {
                        Ok(id) => health_for(&acct, &id),
                        Err(e) => Health::failed(&e.to_string()),
                    },
                    None => continue,
                }
            }
            Err(e) => Health::failed(&format!("{e:#}")),
        };
        if let Some(a) = store.accounts.iter_mut().find(|a| a.key() == key) {
            a.health = Some(health);
        }
    }
    let _ = store.save();
    store.accounts.clone()
}

pub fn remove_account(store: &mut Store, key: &str) -> anyhow::Result<()> {
    let acct = store.find(key).ok_or_else(|| anyhow::anyhow!("no account {key}"))?.clone();
    store::delete_secret(&acct.key()).ok();
    store::delete_secret(&store::refresh_key(&acct.key())).ok();
    store.accounts.retain(|a| a.key() != acct.key());
    // keep a default on the host if another account remains
    if acct.default {
        if let Some(next) = store.accounts.iter_mut().find(|a| a.host == acct.host) {
            next.default = true;
        }
    }
    store.save()
}

pub fn set_default(store: &mut Store, key: &str) -> anyhow::Result<()> {
    let host = store.find(key).ok_or_else(|| anyhow::anyhow!("no account {key}"))?.host.clone();
    for a in store.accounts.iter_mut().filter(|a| a.host == host) {
        a.default = a.key() == key;
    }
    store.save()
}

#[cfg(test)]
mod tests {
    #[test]
    fn panics_become_errors() {
        assert_eq!(super::catch_panic(|| 7), Ok(7));
        let e = super::catch_panic(|| -> u8 { panic!("boom") }).unwrap_err();
        assert!(e.contains("boom"), "{e}");
        let e = super::guarded(|| -> anyhow::Result<()> { panic!("{}", String::from("owned")) }).unwrap_err();
        assert!(e.to_string().contains("owned"));
    }
}
