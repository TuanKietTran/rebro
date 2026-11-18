use std::path::PathBuf;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::model::Account;

/// Credential-store service name. Override with REBRO_KEYRING_SERVICE (tests).
fn service() -> String {
    std::env::var("REBRO_KEYRING_SERVICE").unwrap_or_else(|_| "rebro".into())
}

pub fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("REBRO_HOME") {
        return PathBuf::from(p);
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("rebro")
}

pub fn set_secret(key: &str, token: &str) -> anyhow::Result<()> {
    keyring::Entry::new(&service(), key)?.set_password(token).context("credential store: write failed")?;
    // read it back: a store that silently drops secrets is exactly the bug we're fixing
    let back = get_secret(key)?;
    anyhow::ensure!(back == token, "credential store did not keep the token");
    Ok(())
}

/// Credential entry holding an OAuth refresh token (separate: Windows caps one entry at 2.5 KB).
pub fn refresh_key(key: &str) -> String {
    format!("{key}#refresh")
}

pub fn get_secret(key: &str) -> anyhow::Result<String> {
    Ok(keyring::Entry::new(&service(), key)?.get_password()?)
}

pub fn delete_secret(key: &str) -> anyhow::Result<()> {
    Ok(keyring::Entry::new(&service(), key)?.delete_credential()?)
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default, rename = "account")]
    accounts: Vec<Account>,
}

/// Per-machine facts about an account: its last check and its OAuth token's expiry. Tokens live in each
/// machine's own credential store, so these differ per machine and stay out of accounts.toml, which may
/// be shared between machines (Syncthing): a check on one machine must not rewrite the shared file.
#[derive(Default, Serialize, Deserialize)]
struct Local {
    #[serde(default, rename = "local")]
    entries: Vec<LocalEntry>,
}

#[derive(Serialize, Deserialize)]
struct LocalEntry {
    key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    health: Option<crate::model::Health>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
}

const HEADER: &str = "# rebro accounts (no secrets: tokens are in the OS credential store)\n";
const LOCAL_HEADER: &str = "# rebro, this machine only: last checks and token expiry (never synced)\n";

pub struct Store {
    pub path: PathBuf,
    pub accounts: Vec<Account>,
}

impl Store {
    pub fn load() -> anyhow::Result<Store> {
        Store::load_from(config_dir().join("accounts.toml"))
    }

    pub fn load_from(path: PathBuf) -> anyhow::Result<Store> {
        let mut accounts = read_accounts(&path)?;
        // Syncthing keeps both sides of a concurrent edit as accounts.sync-conflict-*.toml:
        // fold the accounts it knows that we don't back in, then drop the copy. Nothing is lost.
        let mut merged = false;
        for conflict in conflict_copies(&path) {
            for a in read_accounts(&conflict)? {
                if !accounts.iter().any(|x| x.key() == a.key()) {
                    accounts.push(a);
                }
            }
            std::fs::remove_file(&conflict).ok();
            merged = true;
        }
        let local = local_path(&path);
        if let Ok(text) = std::fs::read_to_string(&local) {
            let l: Local = toml::from_str(&text).with_context(|| format!("parsing {}", local.display()))?;
            for e in l.entries {
                if let Some(a) = accounts.iter_mut().find(|a| a.key() == e.key) {
                    a.health = e.health;
                    a.expires_at = e.expires_at;
                }
            }
        }
        let mut store = Store { path, accounts };
        if merged {
            store.sort();
            store.save()?;
        }
        Ok(store)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let shared: Vec<Account> = self
            .accounts
            .iter()
            .map(|a| Account { health: None, expires_at: None, ..a.clone() })
            .collect();
        let body = format!("{HEADER}{}", toml::to_string_pretty(&File { accounts: shared })?);
        // unchanged content is not rewritten: a sync tool then has nothing to ship
        if std::fs::read_to_string(&self.path).ok().as_deref() != Some(body.as_str()) {
            write_atomic(&self.path, &body)?;
        }
        let local = Local {
            entries: self
                .accounts
                .iter()
                .filter(|a| a.health.is_some() || a.expires_at.is_some())
                .map(|a| LocalEntry { key: a.key(), health: a.health.clone(), expires_at: a.expires_at })
                .collect(),
        };
        write_atomic(&local_path(&self.path), &format!("{LOCAL_HEADER}{}", toml::to_string_pretty(&local)?))
    }

    fn sort(&mut self) {
        self.accounts.sort_by(|a, b| (a.host.as_str(), !a.default, a.username.as_str()).cmp(&(b.host.as_str(), !b.default, b.username.as_str())));
    }

    /// Exact "host/user" lookup.
    pub fn find_key(&self, key: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.key() == key)
    }

    /// Find by "host/user", or by "user" / "host" when unambiguous.
    pub fn find(&self, query: &str) -> Option<&Account> {
        let q = query.to_ascii_lowercase();
        if let Some(a) = self.accounts.iter().find(|a| a.key().to_ascii_lowercase() == q) {
            return Some(a);
        }
        let hits: Vec<&Account> = self
            .accounts
            .iter()
            .filter(|a| a.username.to_ascii_lowercase() == q || a.host == q)
            .collect();
        if let [only] = hits[..] {
            Some(only)
        } else {
            hits.into_iter().find(|a| a.default && a.host == q)
        }
    }

    pub fn upsert(&mut self, acct: Account) {
        match self.accounts.iter_mut().find(|a| a.key() == acct.key()) {
            Some(slot) => *slot = acct,
            None => self.accounts.push(acct),
        }
        self.sort();
    }

    /// Which account git should use for host + optional "owner/repo" path.
    pub fn pick(&self, host: &str, path: Option<&str>) -> Option<&Account> {
        let host = crate::model::normalize_host(host);
        let on_host: Vec<&Account> = self.accounts.iter().filter(|a| a.host == host).collect();
        if let Some(owner) = path.and_then(|p| p.trim_start_matches('/').split('/').next()).filter(|o| !o.is_empty()) {
            if let Some(a) = on_host.iter().find(|a| a.matches_owner(owner)) {
                return Some(a);
            }
        }
        on_host.iter().find(|a| a.default).or(on_host.first()).copied()
    }
}

fn read_accounts(path: &std::path::Path) -> anyhow::Result<Vec<Account>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(toml::from_str::<File>(&s).with_context(|| format!("parsing {}", path.display()))?.accounts),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn local_path(accounts: &std::path::Path) -> PathBuf {
    accounts.with_file_name("local.toml")
}

fn conflict_copies(accounts: &std::path::Path) -> Vec<PathBuf> {
    let Some(dir) = accounts.parent() else { return vec![] };
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("accounts.sync-conflict-") && n.ends_with(".toml"))
        })
        .collect();
    v.sort();
    v
}

fn write_atomic(path: &std::path::Path, body: &str) -> anyhow::Result<()> {
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Forge;

    fn store(accts: Vec<Account>) -> Store {
        Store { path: PathBuf::from("unused"), accounts: accts }
    }

    #[test]
    fn pick_routes_by_owner_then_default() {
        let mut work = Account::new(Forge::GitHub, "github.com", "work-bot");
        work.owners = vec!["acme".into()];
        let mut me = Account::new(Forge::GitHub, "github.com", "TuanKietTran");
        me.default = true;
        let cb = Account::new(Forge::Forgejo, "codeberg.org", "tkt");
        let s = store(vec![work, me, cb]);
        assert_eq!(s.pick("github.com", Some("acme/app.git")).unwrap().username, "work-bot");
        assert_eq!(s.pick("github.com", Some("TuanKietTran/edge.git")).unwrap().username, "TuanKietTran");
        assert_eq!(s.pick("github.com", Some("someone/else")).unwrap().username, "TuanKietTran");
        assert_eq!(s.pick("github.com", None).unwrap().username, "TuanKietTran");
        assert_eq!(s.pick("codeberg.org", None).unwrap().username, "tkt");
        assert!(s.pick("gitlab.com", None).is_none());
    }

    #[test]
    fn toml_round_trip() {
        let dir = std::env::temp_dir().join(format!("rebro-test-{}", std::process::id()));
        let mut s = Store { path: dir.join("accounts.toml"), accounts: vec![] };
        let mut a = Account::new(Forge::GitLab, "gitlab.com", "me");
        a.default = true;
        s.upsert(a.clone());
        s.save().unwrap();
        let text = std::fs::read_to_string(&s.path).unwrap();
        assert!(text.contains("[[account]]") && !text.to_lowercase().contains("token ="));
        let back: File = toml::from_str(&text).unwrap();
        assert_eq!(back.accounts, vec![a]);
        std::fs::remove_dir_all(dir).ok();
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rebro-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn machine_state_stays_out_of_the_shared_file() {
        let dir = tmpdir("local");
        let mut s = Store { path: dir.join("accounts.toml"), accounts: vec![] };
        let mut a = Account::new(Forge::GitHub, "github.com", "me");
        a.auth = Some("oauth".into());
        s.upsert(a);
        s.save().unwrap();
        let shared_before = std::fs::read(&s.path).unwrap();
        let mtime = std::fs::metadata(&s.path).unwrap().modified().unwrap();

        // a check on this machine: health + expiry change, the shared file must not
        s.accounts[0].health = Some(crate::model::Health::failed("not signed in on this machine"));
        s.accounts[0].expires_at = Some(42);
        std::thread::sleep(std::time::Duration::from_millis(20));
        s.save().unwrap();
        assert_eq!(std::fs::read(&s.path).unwrap(), shared_before);
        assert_eq!(std::fs::metadata(&s.path).unwrap().modified().unwrap(), mtime, "shared file rewritten");

        let back = Store::load_from(dir.join("accounts.toml")).unwrap();
        assert_eq!(back.accounts[0].expires_at, Some(42));
        assert!(back.accounts[0].health.as_ref().is_some_and(|h| !h.ok));
        assert_eq!(back.accounts[0].auth.as_deref(), Some("oauth"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn syncthing_conflict_copies_are_merged_and_removed() {
        let dir = tmpdir("conflict");
        let mine = Store { path: dir.join("accounts.toml"), accounts: vec![Account::new(Forge::GitHub, "github.com", "me")] };
        mine.save().unwrap();
        let theirs = Store {
            path: dir.join("accounts.sync-conflict-20261118-083000-ABCDEFG.toml"),
            accounts: vec![Account::new(Forge::GitHub, "github.com", "me"), Account::new(Forge::Forgejo, "codeberg.org", "tkt")],
        };
        theirs.save().unwrap();
        let s = Store::load_from(dir.join("accounts.toml")).unwrap();
        let keys: Vec<String> = s.accounts.iter().map(|a| a.key()).collect();
        assert_eq!(keys, vec!["codeberg.org/tkt", "github.com/me"]);
        assert!(!theirs.path.exists(), "conflict copy left behind");
        let again = Store::load_from(dir.join("accounts.toml")).unwrap();
        assert_eq!(again.accounts.len(), 2);
        std::fs::remove_dir_all(dir).ok();
    }
}
