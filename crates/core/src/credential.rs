//! git credential helper protocol (`git credential-<helper> get|store|erase`).
//! https://git-scm.com/docs/git-credential#IOFMT

use std::collections::HashMap;
use std::io::{BufRead, Write};

use crate::store::Store;

pub fn parse(input: impl BufRead) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in input.lines().map_while(Result::ok) {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.to_string(), v.to_string());
        }
    }
    map
}

/// `get`: answer with the account's username + token. `store`/`erase`: ignored on purpose, so
/// git can never overwrite or delete the tokens rebro manages (fix those with rebro itself).
pub fn handle(action: &str, input: impl BufRead, mut out: impl Write, store: &mut Store) -> anyhow::Result<()> {
    if action != "get" {
        return Ok(());
    }
    let req = parse(input);
    if req.get("protocol").map(String::as_str) != Some("https") {
        return Ok(());
    }
    let Some(host) = req.get("host") else { return Ok(()) };
    let Some(acct) = store.pick(host, req.get("path").map(String::as_str)).cloned() else { return Ok(()) };
    // If git already named a user (https://user@host/...), only answer for that user.
    let acct = match req.get("username") {
        Some(user) if !user.eq_ignore_ascii_case(&acct.username) && !user.eq_ignore_ascii_case(acct.git_username()) => {
            match store.accounts.iter().find(|a| a.host == acct.host && a.username.eq_ignore_ascii_case(user)) {
                Some(other) => other.clone(),
                None => return Ok(()),
            }
        }
        _ => acct,
    };
    // refreshes OAuth sign-ins that are about to expire
    let token = crate::access_token(store, &acct.key())?;
    writeln!(out, "username={}", acct.git_username())?;
    writeln!(out, "password={token}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_git_input() {
        let m = parse("protocol=https\nhost=github.com\npath=TuanKietTran/edge.git\n\nignored=1\n".as_bytes());
        assert_eq!(m["host"], "github.com");
        assert_eq!(m["path"], "TuanKietTran/edge.git");
        assert!(!m.contains_key("ignored"));
    }

    #[test]
    fn non_https_and_unknown_hosts_get_no_answer() {
        let mut store = Store { path: "unused".into(), accounts: vec![] };
        let mut out = Vec::new();
        handle("get", "protocol=https\nhost=example.org\n\n".as_bytes(), &mut out, &mut store).unwrap();
        handle("get", "protocol=ssh\nhost=github.com\n\n".as_bytes(), &mut out, &mut store).unwrap();
        handle("store", "protocol=https\nhost=github.com\npassword=x\n\n".as_bytes(), &mut out, &mut store).unwrap();
        assert!(out.is_empty());
    }
}
