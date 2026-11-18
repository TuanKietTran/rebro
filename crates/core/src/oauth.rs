//! "Sign in" without creating tokens by hand.
//!
//! - GitHub, GitLab: OAuth device flow (RFC 8628): show a code, the user approves in the browser.
//! - Gitea, Forgejo (Codeberg): authorization code + PKCE with a loopback redirect (RFC 8252).
//!
//! Client IDs are public, pre-registered "native app" clients: the ones git-credential-oauth
//! registered on github.com / gitlab.com / gitea.com / codeberg.org, and the client every Gitea
//! (>= 1.22) and Forgejo instance ships for git credential helpers. Using a client other than gh's
//! keeps these tokens out of gh's 10-tokens-per-app revocation bucket.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context};
use base64::Engine;
use serde_json::Value;
use sha2::Digest;

use crate::model::Forge;

#[derive(Clone, Debug)]
pub struct Client {
    pub client_id: String,
    /// Non-confidential for native apps; only GitHub's registration has one.
    pub client_secret: Option<String>,
    pub auth_url: String,
    pub token_url: String,
    pub device_url: Option<String>,
    pub scopes: Vec<String>,
}

/// Pre-configured client for a host, or None (self-hosted GitLab needs your own client id).
pub fn client_for(forge: Forge, host: &str, client_id: Option<&str>) -> Option<Client> {
    let b = format!("https://{host}");
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    match forge {
        Forge::GitHub if host == "github.com" || client_id.is_some() => Some(Client {
            client_id: client_id.unwrap_or("b895675a4e2cf54d5c6c").into(),
            client_secret: client_id.is_none().then(|| "2b746eea028711749c5062b9fe626fed78d03cc0".into()),
            auth_url: format!("{b}/login/oauth/authorize"),
            token_url: format!("{b}/login/oauth/access_token"),
            device_url: Some(format!("{b}/login/device/code")),
            // read:org is what gh needs on top of git's scopes
            scopes: s(&["repo", "gist", "workflow", "read:org"]),
        }),
        Forge::GitLab if host == "gitlab.com" || client_id.is_some() => Some(Client {
            client_id: client_id
                .unwrap_or("10bfbbf46e5b760b55ce772a262d7a0205eacc417816eb84d37d0fb02c89bb97")
                .into(),
            client_secret: None,
            auth_url: format!("{b}/oauth/authorize"),
            token_url: format!("{b}/oauth/token"),
            device_url: Some(format!("{b}/oauth/authorize_device")),
            // read_user names the account; dropped automatically if the client doesn't allow it
            scopes: s(&["read_repository", "write_repository", "read_user"]),
        }),
        Forge::Forgejo | Forge::Gitea => Some(Client {
            client_id: client_id
                .unwrap_or(match host {
                    "codeberg.org" => "246ca3e8-e974-430c-b9ec-3d4e2b54ad28",
                    "gitea.com" => "e13f8ebc-398d-4091-9481-5a37a76b51f6",
                    // built into every Gitea >= 1.22 / Forgejo instance
                    _ => "a4792ccc-144e-407e-86c9-5e7d8d9c3269",
                })
                .into(),
            client_secret: None,
            auth_url: format!("{b}/login/oauth/authorize"),
            token_url: format!("{b}/login/oauth/access_token"),
            device_url: None,
            scopes: vec![],
        }),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix seconds; None = doesn't expire (GitHub OAuth apps).
    pub expires_at: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    /// Same page with the code pre-filled (GitLab), if the forge offers it.
    pub verification_uri_complete: Option<String>,
    device_code: String,
    interval: u64,
    expires_in: u64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("rebro/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// POST a form, return JSON even for 4xx (OAuth errors come back as JSON bodies).
fn post_form(url: &str, form: &[(&str, &str)]) -> anyhow::Result<Value> {
    match agent().post(url).set("Accept", "application/json").send_form(form) {
        Ok(r) => Ok(r.into_json()?),
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            serde_json::from_str(&body).map_err(|_| anyhow!("HTTP {code} from {url}: {}", body.chars().take(200).collect::<String>()))
        }
        Err(e) => Err(anyhow!("network: {e}")),
    }
}

fn token_set(v: &Value) -> anyhow::Result<TokenSet> {
    if let Some(err) = v["error"].as_str() {
        bail!("{err}: {}", v["error_description"].as_str().unwrap_or(""));
    }
    Ok(TokenSet {
        access_token: v["access_token"].as_str().ok_or_else(|| anyhow!("no access_token in response"))?.to_string(),
        refresh_token: v["refresh_token"].as_str().filter(|s| !s.is_empty()).map(String::from),
        expires_at: v["expires_in"].as_u64().filter(|&s| s > 0).map(|s| now() + s),
    })
}

pub fn device_start(c: &Client) -> anyhow::Result<DeviceCode> {
    let url = c.device_url.as_deref().ok_or_else(|| anyhow!("this forge has no device flow"))?;
    let scope = c.scopes.join(" ");
    let mut v = post_form(url, &[("client_id", &c.client_id), ("scope", &scope)])?;
    if v["error"].as_str() == Some("invalid_scope") && c.scopes.iter().any(|s| s == "read_user") {
        let fewer = c.scopes.iter().filter(|s| *s != "read_user").cloned().collect::<Vec<_>>().join(" ");
        v = post_form(url, &[("client_id", &c.client_id), ("scope", &fewer)])?;
    }
    if let Some(err) = v["error"].as_str() {
        bail!("{err}: {}", v["error_description"].as_str().unwrap_or(""));
    }
    Ok(DeviceCode {
        user_code: v["user_code"].as_str().ok_or_else(|| anyhow!("no user_code"))?.to_string(),
        verification_uri: v["verification_uri"].as_str().ok_or_else(|| anyhow!("no verification_uri"))?.to_string(),
        verification_uri_complete: v["verification_uri_complete"].as_str().map(String::from),
        device_code: v["device_code"].as_str().ok_or_else(|| anyhow!("no device_code"))?.to_string(),
        interval: v["interval"].as_u64().unwrap_or(5),
        expires_in: v["expires_in"].as_u64().unwrap_or(900),
    })
}

/// Blocks until the user approves, denies, or the code expires. `cancelled` is checked each poll.
pub fn device_poll(c: &Client, d: &DeviceCode, cancelled: &dyn Fn() -> bool) -> anyhow::Result<TokenSet> {
    let deadline = Instant::now() + Duration::from_secs(d.expires_in);
    let mut interval = d.interval.max(1);
    loop {
        std::thread::sleep(Duration::from_secs(interval));
        if cancelled() {
            bail!("cancelled");
        }
        if Instant::now() > deadline {
            bail!("the code expired before it was approved; start again");
        }
        let v = post_form(
            &c.token_url,
            &[
                ("client_id", c.client_id.as_str()),
                ("device_code", d.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ],
        )?;
        match v["error"].as_str() {
            Some("authorization_pending") => continue,
            Some("slow_down") => interval += 5,
            Some("access_denied") => bail!("you declined the authorization"),
            Some("expired_token") => bail!("the code expired; start again"),
            _ => return token_set(&v),
        }
    }
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random(n: usize) -> anyhow::Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf).map_err(|e| anyhow!("random: {e}"))?;
    Ok(buf)
}

/// Authorization code + PKCE via a one-shot server on 127.0.0.1. Opens the browser with `open`.
/// Blocks up to 5 minutes for the redirect.
pub fn loopback_login(c: &Client, open: &dyn Fn(&str), cancelled: &dyn Fn() -> bool) -> anyhow::Result<TokenSet> {
    let listener = TcpListener::bind("127.0.0.1:0").context("binding a local port for the sign-in redirect")?;
    listener.set_nonblocking(true)?;
    let redirect = format!("http://127.0.0.1:{}/", listener.local_addr()?.port());
    let verifier = b64url(&random(32)?);
    let challenge = b64url(&sha2::Sha256::digest(verifier.as_bytes()));
    let state = b64url(&random(16)?);
    let mut url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&state={}&code_challenge={}&code_challenge_method=S256",
        c.auth_url,
        urlenc(&c.client_id),
        urlenc(&redirect),
        state,
        challenge
    );
    if !c.scopes.is_empty() {
        url.push_str(&format!("&scope={}", urlenc(&c.scopes.join(" "))));
    }
    open(&url);

    let deadline = Instant::now() + Duration::from_secs(300);
    let code = loop {
        if cancelled() {
            bail!("cancelled");
        }
        if Instant::now() > deadline {
            bail!("no approval within 5 minutes; start again");
        }
        let (mut stream, _) = match listener.accept() {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        stream.set_nonblocking(false)?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line)?;
        // "GET /?code=...&state=... HTTP/1.1"
        let query = line.split_whitespace().nth(1).and_then(|p| p.split_once('?')).map(|(_, q)| q.to_string()).unwrap_or_default();
        let params: std::collections::HashMap<String, String> =
            query.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), urldec(v))).collect();
        let (ok, msg) = match (params.get("code"), params.get("state"), params.get("error")) {
            (_, _, Some(err)) => (None, format!("Sign-in failed: {err}. You can close this tab.")),
            (Some(code), Some(s), _) if *s == state => (Some(code.clone()), "Signed in to rebro. You can close this tab.".to_string()),
            _ => (None, String::new()), // favicon or stray request: keep waiting
        };
        if msg.is_empty() {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            continue;
        }
        let page = format!("<!doctype html><meta charset=utf-8><title>rebro</title><body style=\"font:16px system-ui;margin:3em\"><p>{msg}</p>");
        let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len());
        match ok {
            Some(code) => break code,
            None => bail!("{msg}"),
        }
    };

    let mut form = vec![
        ("client_id", c.client_id.as_str()),
        ("code", code.as_str()),
        ("code_verifier", verifier.as_str()),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect.as_str()),
    ];
    if let Some(secret) = &c.client_secret {
        form.push(("client_secret", secret));
    }
    token_set(&post_form(&c.token_url, &form)?)
}

pub fn refresh(c: &Client, refresh_token: &str) -> anyhow::Result<TokenSet> {
    let mut form = vec![("client_id", c.client_id.as_str()), ("grant_type", "refresh_token"), ("refresh_token", refresh_token)];
    if let Some(secret) = &c.client_secret {
        form.push(("client_secret", secret));
    }
    let mut t = token_set(&post_form(&c.token_url, &form)?)?;
    // some forges rotate refresh tokens, others keep the old one valid
    if t.refresh_token.is_none() {
        t.refresh_token = Some(refresh_token.to_string());
    }
    Ok(t)
}

fn urlenc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn urldec(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            // bytes, not &s[..]: a '%' before a multi-byte character must not split it (panic)
            b'%' if i + 3 <= bytes.len() => {
                if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(b);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_codec() {
        assert_eq!(urlenc("http://127.0.0.1:5/"), "http%3A%2F%2F127.0.0.1%3A5%2F");
        assert_eq!(urldec("a%2Fb+c%3D"), "a/b c=");
        assert_eq!(urldec("100%"), "100%");
        // used to panic: slicing &str inside a multi-byte character
        assert_eq!(urldec("%é"), "%é");
        assert_eq!(urldec("x%4é"), "x%4é");
        // used to be left undecoded: an escape at the very end
        assert_eq!(urldec("state%3D"), "state=");
    }

    #[test]
    fn presets() {
        assert!(client_for(Forge::GitHub, "github.com", None).unwrap().device_url.is_some());
        assert!(client_for(Forge::GitLab, "gitlab.example.com", None).is_none());
        assert!(client_for(Forge::GitLab, "gitlab.example.com", Some("abc")).is_some());
        assert_eq!(client_for(Forge::Forgejo, "git.example.org", None).unwrap().client_id, "a4792ccc-144e-407e-86c9-5e7d8d9c3269");
        assert!(client_for(Forge::Forgejo, "codeberg.org", None).unwrap().device_url.is_none());
    }

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        // reference: python -c "import hashlib,base64; print(base64.urlsafe_b64encode(
        //   hashlib.sha256(b'dBjftJeZ4CVP-mJ92K9qXkkIz8NQAyx5j8mWk-WRVKY').digest()).rstrip(b'=').decode())"
        let v = "dBjftJeZ4CVP-mJ92K9qXkkIz8NQAyx5j8mWk-WRVKY";
        assert_eq!(b64url(&sha2::Sha256::digest(v.as_bytes())), "97PmLv77O13onIsPmmVuQ58Xbvc1tcQt4ycG1Va85O4");
    }
}
