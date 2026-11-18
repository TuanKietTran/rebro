//! Verify a token against its forge and learn who it belongs to, its scopes and its expiry.

use std::time::Duration;

use serde_json::Value;

use crate::model::{days_until, Forge};

#[derive(Clone, Debug)]
pub struct Identity {
    pub username: String,
    pub scopes: Option<Vec<String>>,
    /// "YYYY-MM-DD", "never", or None when the forge doesn't say.
    pub expires: Option<String>,
    pub kind: &'static str,
    pub warnings: Vec<String>,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("rebro/", env!("CARGO_PKG_VERSION")))
        .build()
}

pub fn api_base(forge: Forge, host: &str) -> String {
    match forge {
        Forge::GitHub if host == "github.com" => "https://api.github.com".into(),
        Forge::GitHub => format!("https://{host}/api/v3"),
        Forge::GitLab => format!("https://{host}/api/v4"),
        Forge::Forgejo | Forge::Gitea => format!("https://{host}/api/v1"),
    }
}

fn describe(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(401, _) => anyhow::anyhow!("token rejected (401): revoked, expired or mistyped"),
        ureq::Error::Status(403, r) => {
            anyhow::anyhow!("forbidden (403): {}", r.into_string().unwrap_or_default().chars().take(200).collect::<String>())
        }
        ureq::Error::Status(code, r) => {
            anyhow::anyhow!("HTTP {code}: {}", r.into_string().unwrap_or_default().chars().take(200).collect::<String>())
        }
        ureq::Error::Transport(t) => anyhow::anyhow!("network: {t}"),
    }
}

pub fn verify(forge: Forge, host: &str, token: &str) -> anyhow::Result<Identity> {
    let base = api_base(forge, host);
    let mut id = match forge {
        Forge::GitHub => github(&base, token)?,
        Forge::GitLab => gitlab(&base, token)?,
        Forge::Forgejo | Forge::Gitea => gitea(&base, token)?,
    };
    if let Some(days) = id.expires.as_deref().and_then(days_until) {
        if days < 0 {
            id.warnings.push("expired".into());
        } else if days <= 14 {
            id.warnings.push(format!("expires in {days} days"));
        }
    }
    Ok(id)
}

fn github(base: &str, token: &str) -> anyhow::Result<Identity> {
    let resp = agent()
        .get(&format!("{base}/user"))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(describe)?;
    // classic PATs and OAuth tokens list scopes; fine-grained PATs don't send the header
    let scopes = resp
        .header("x-oauth-scopes")
        .map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>());
    // e.g. "2027-02-17 10:00:00 UTC"; absent = no expiry
    let expires = Some(resp.header("github-authentication-token-expiration").map(|s| s.get(..10).unwrap_or(s).to_string()).unwrap_or_else(|| "never".into()));
    let body: Value = resp.into_json()?;
    let kind = match token.get(..4) {
        Some("ghp_") => "classic PAT",
        Some("gith") => "fine-grained PAT",
        Some("gho_") => "OAuth (gh login)",
        _ => "token",
    };
    let mut warnings = vec![];
    if kind == "OAuth (gh login)" {
        warnings.push("OAuth app token: GitHub revokes the oldest beyond 10 per app; use a PAT".into());
    }
    if let Some(s) = &scopes {
        for need in ["repo", "read:org"] {
            if !s.iter().any(|x| x == need || (need == "read:org" && (x == "admin:org" || x == "write:org"))) {
                warnings.push(format!("missing scope {need} (gh needs it)"));
            }
        }
    }
    Ok(Identity {
        username: body["login"].as_str().ok_or_else(|| anyhow::anyhow!("no login in /user"))?.to_string(),
        scopes,
        expires,
        kind,
        warnings,
    })
}

fn gitlab(base: &str, token: &str) -> anyhow::Result<Identity> {
    let a = agent();
    let bearer = format!("Bearer {token}"); // works for PATs and OAuth tokens alike
    let user: Value = match a.get(&format!("{base}/user")).set("Authorization", &bearer).call() {
        Ok(r) => r.into_json()?,
        // repo-only OAuth sign-ins may not read the profile: confirm the token, name it by user id
        Err(ureq::Error::Status(403, _)) => {
            let info: Value = a
                .get(&format!("{}/oauth/token/info", base.trim_end_matches("/api/v4")))
                .set("Authorization", &bearer)
                .call()
                .map_err(describe)?
                .into_json()?;
            let id = info["resource_owner_id"].as_u64().ok_or_else(|| anyhow::anyhow!("token info has no owner"))?;
            serde_json::json!({ "username": format!("user-{id}") })
        }
        Err(e) => return Err(describe(e)),
    };
    let (mut scopes, mut expires, mut kind) = (None, None, "token");
    if let Ok(r) = a.get(&format!("{base}/personal_access_tokens/self")).set("Authorization", &bearer).call() {
        let pat: Value = r.into_json()?;
        scopes = pat["scopes"].as_array().map(|v| v.iter().filter_map(|s| s.as_str().map(String::from)).collect::<Vec<String>>());
        expires = Some(pat["expires_at"].as_str().unwrap_or("never").to_string());
        kind = "PAT";
    }
    let mut warnings = vec![];
    if let Some(s) = &scopes {
        if !s.iter().any(|x| x == "api" || x == "write_repository") {
            warnings.push("missing scope write_repository (git push needs it)".into());
        }
    }
    Ok(Identity {
        username: user["username"].as_str().ok_or_else(|| anyhow::anyhow!("no username in /user"))?.to_string(),
        scopes,
        expires,
        kind,
        warnings,
    })
}

fn gitea(base: &str, token: &str) -> anyhow::Result<Identity> {
    // Gitea/Forgejo accept "Bearer" for both access tokens and OAuth2 tokens
    let resp = agent().get(&format!("{base}/user")).set("Authorization", &format!("Bearer {token}")).call().map_err(describe)?;
    // Gitea/Forgejo tokens don't expire; scoped tokens report scopes on some versions
    let scopes = resp
        .header("x-oauth-scopes")
        .map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>());
    let body: Value = resp.into_json()?;
    Ok(Identity {
        username: body["login"].as_str().ok_or_else(|| anyhow::anyhow!("no login in /user"))?.to_string(),
        scopes,
        expires: Some("never".into()),
        kind: "access token",
        warnings: vec![],
    })
}
