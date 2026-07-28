//! GitHub App authentication: RS256 JWT minting and installation-token caching.

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};

use crate::GitHubIssueReporter;

#[derive(Debug, Serialize)]
pub(crate) struct Claims {
    pub(crate) iat: u64,
    pub(crate) exp: u64,
    pub(crate) iss: String,
}

/// Build the JWT claims for `app_id` relative to `now` (unix seconds).
pub(crate) fn build_claims(app_id: &str, now: u64) -> Claims {
    Claims {
        iat: now - 60,
        exp: now + 600,
        iss: app_id.to_string(),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Deserialize)]
struct TokenResponse {
    token: String,
}

impl GitHubIssueReporter {
    fn generate_jwt(&self) -> anyhow::Result<String> {
        let claims = build_claims(&self.app_id, unix_now());
        let key = EncodingKey::from_rsa_pem(self.private_key.as_bytes())?;
        Ok(encode(&Header::new(Algorithm::RS256), &claims, &key)?)
    }

    async fn installation_token(&self) -> anyhow::Result<String> {
        {
            let guard = self.cached.lock().unwrap();
            if let Some((tok, exp)) = guard.as_ref() {
                if unix_now() < exp.saturating_sub(60) {
                    return Ok(tok.clone());
                }
            }
        }

        let jwt = self.generate_jwt()?;
        let url = format!(
            "https://api.github.com/app/installations/{}/access_tokens",
            self.installation_id
        );
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {jwt}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "house-chatbot")
            .send()
            .await?
            .error_for_status()?
            .json::<TokenResponse>()
            .await?;

        let token = resp.token;
        *self.cached.lock().unwrap() = Some((token.clone(), unix_now() + 3600));
        Ok(token)
    }

    /// Return a bearer token — either the direct token (GITHUB_TOKEN) or a
    /// GitHub App installation token obtained via the JWT flow.
    pub(crate) async fn token(&self) -> anyhow::Result<String> {
        if let Some(token) = &self.direct_token {
            return Ok(token.clone());
        }
        self.installation_token().await
    }

    /// Perform an authenticated GET request to the GitHub API.
    /// Paths starting with `/search/` are treated as root-level API paths;
    /// all others are prefixed with `/repos/{repo}`.
    pub(crate) async fn authed_get(&self, path: &str) -> anyhow::Result<String> {
        let token = self.token().await?;
        let url = if path.starts_with("/search/") {
            format!("https://api.github.com{path}")
        } else {
            format!("https://api.github.com/repos/{}{}", self.repo, path)
        };
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "house-chatbot")
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.text().await?)
    }
}
