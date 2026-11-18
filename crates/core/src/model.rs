use serde::{Deserialize, Serialize};

use crate::api::Identity;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Forge {
    GitHub,
    GitLab,
    /// Forgejo, e.g. Codeberg.
    Forgejo,
    Gitea,
}

impl Forge {
    pub const ALL: [Forge; 4] = [Forge::GitHub, Forge::GitLab, Forge::Forgejo, Forge::Gitea];

    pub fn name(self) -> &'static str {
        match self {
            Forge::GitHub => "GitHub",
            Forge::GitLab => "GitLab",
            Forge::Forgejo => "Codeberg / Forgejo",
            Forge::Gitea => "Gitea",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Forge::GitHub => "github",
            Forge::GitLab => "gitlab",
            Forge::Forgejo => "forgejo",
            Forge::Gitea => "gitea",
        }
    }

    pub fn default_host(self) -> &'static str {
        match self {
            Forge::GitHub => "github.com",
            Forge::GitLab => "gitlab.com",
            Forge::Forgejo => "codeberg.org",
            Forge::Gitea => "gitea.com",
        }
    }

    pub fn parse(s: &str) -> Option<Forge> {
        match s.to_ascii_lowercase().as_str() {
            "github" | "gh" => Some(Forge::GitHub),
            "gitlab" | "gl" => Some(Forge::GitLab),
            "forgejo" | "codeberg" | "cb" => Some(Forge::Forgejo),
            "gitea" => Some(Forge::Gitea),
            _ => None,
        }
    }

    /// Where to create a long-lived token. GitHub/GitLab pages come pre-filled with the scopes
    /// gh/glab and git need.
    pub fn token_url(self, host: &str) -> String {
        match self {
            // classic PAT: can be set to never expire, and is not an OAuth-app token, so it is not
            // subject to GitHub's 10-tokens-per-app revocation that kept killing `gh auth login`.
            Forge::GitHub if host == "github.com" => {
                "https://github.com/settings/tokens/new?scopes=repo,read:org,workflow,gist&description=rebro".into()
            }
            Forge::GitHub => format!("https://{host}/settings/tokens/new?scopes=repo,read:org,workflow,gist&description=rebro"),
            Forge::GitLab => format!(
                "https://{host}/-/user_settings/personal_access_tokens?name=rebro&scopes=api,read_user,write_repository"
            ),
            Forge::Forgejo | Forge::Gitea => format!("https://{host}/user/settings/applications"),
        }
    }

    /// Guess the forge from a host name (used when a host is given without --forge).
    pub fn guess(host: &str) -> Forge {
        let h = host.to_ascii_lowercase();
        if h.contains("github") {
            Forge::GitHub
        } else if h.contains("gitlab") {
            Forge::GitLab
        } else if h.contains("codeberg") || h.contains("forgejo") {
            Forge::Forgejo
        } else {
            Forge::Gitea
        }
    }
}

impl std::fmt::Display for Forge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Result of the last verification against the forge API. Never contains the token.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Health {
    pub ok: bool,
    pub checked_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    /// e.g. "2027-11-17" or "never"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl Health {
    pub fn from_identity(id: &Identity) -> Health {
        Health {
            ok: true,
            checked_at: now_utc(),
            scopes: id.scopes.clone(),
            expires: id.expires.clone(),
            token_kind: Some(id.kind.to_string()),
            error: None,
            warnings: id.warnings.clone(),
        }
    }

    pub fn failed(msg: &str) -> Health {
        Health { ok: false, checked_at: now_utc(), error: Some(msg.to_string()), ..Default::default() }
    }

    /// One short word for tables.
    pub fn summary(&self) -> String {
        if !self.ok {
            "FAILED".into()
        } else if !self.warnings.is_empty() {
            "warn".into()
        } else {
            "ok".into()
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Account {
    pub forge: Forge,
    pub host: String,
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Used for this host when the repo owner doesn't pick another account.
    #[serde(default)]
    pub default: bool,
    /// Repo owners (users/orgs/groups) that should use this account, for hosts with several accounts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<Health>,
    /// "oauth" when added with Sign in (refreshable), absent for pasted tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    /// OAuth access-token expiry (unix seconds); refreshed automatically before use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// Your own OAuth client id (self-hosted GitLab / GitHub Enterprise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

impl Account {
    pub fn new(forge: Forge, host: &str, username: &str) -> Account {
        Account {
            forge,
            host: normalize_host(host),
            username: username.to_string(),
            label: None,
            default: false,
            owners: vec![],
            health: None,
            auth: None,
            expires_at: None,
            client_id: None,
        }
    }

    pub fn is_oauth(&self) -> bool {
        self.auth.as_deref() == Some("oauth")
    }

    /// Username git should send. GitLab OAuth tokens expect "oauth2".
    pub fn git_username(&self) -> &str {
        if self.is_oauth() && self.forge == Forge::GitLab {
            "oauth2"
        } else {
            &self.username
        }
    }

    /// Stable id, also the credential-store entry name: "host/username".
    pub fn key(&self) -> String {
        format!("{}/{}", self.host, self.username)
    }

    /// Does this account own the repo path "owner/repo(.git)"?
    pub fn matches_owner(&self, owner: &str) -> bool {
        self.username.eq_ignore_ascii_case(owner) || self.owners.iter().any(|o| o.eq_ignore_ascii_case(owner))
    }
}

pub fn normalize_host(host: &str) -> String {
    let h = host.trim().trim_end_matches('/');
    let h = h.strip_prefix("https://").or_else(|| h.strip_prefix("http://")).unwrap_or(h);
    h.split('/').next().unwrap_or(h).to_ascii_lowercase()
}

pub fn now_utc() -> String {
    // RFC 3339-ish without pulling in chrono: seconds since epoch -> civil date (UTC).
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Days from 1970-01-01 to (year, month, day), Howard Hinnant's algorithm.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Days until a "YYYY-MM-DD..." date, negative if past. None if unparseable.
pub fn days_until(date: &str) -> Option<i64> {
    let parts: Vec<i64> = date.get(..10)?.split('-').filter_map(|p| p.parse().ok()).collect();
    if parts.len() != 3 {
        return None;
    }
    let (y, m, d) = (parts[0], parts[1], parts[2]);
    // days_from_civil
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let target = era * 146_097 + doe - 719_468;
    let today = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64 / 86_400;
    Some(target - today)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_normalized() {
        assert_eq!(normalize_host("https://GitHub.com/"), "github.com");
        assert_eq!(normalize_host("codeberg.org/user/repo"), "codeberg.org");
        assert_eq!(normalize_host(" git.example.com:3000 "), "git.example.com:3000");
    }

    #[test]
    fn date_math_round_trips() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_454), (2026, 1, 1));
        assert_eq!(days_until(&now_utc()), Some(0));
    }

    #[test]
    fn owner_matching() {
        let mut a = Account::new(Forge::GitHub, "github.com", "TuanKietTran");
        a.owners = vec!["acme-corp".into()];
        assert!(a.matches_owner("tuankiettran"));
        assert!(a.matches_owner("ACME-corp"));
        assert!(!a.matches_owner("someone"));
    }
}
