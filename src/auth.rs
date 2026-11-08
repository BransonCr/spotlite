use crate::config::{token_path, Config};
use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{SystemTime, UNIX_EPOCH};

const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

pub const SCOPES: &str = "user-read-playback-state user-modify-playback-state \
user-read-currently-playing user-library-read user-library-modify playlist-read-private \
playlist-read-collaborative playlist-modify-private playlist-modify-public user-read-private \
user-top-read user-read-recently-played";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix time (seconds) when the access token stops working.
    pub expires_at: u64,
    /// Scopes the user granted; missing in tokens saved by older versions.
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
    scope: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Token {
    pub fn load() -> Result<Self> {
        let path = token_path();
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("no token at {}; run `spotlite auth` first", path.display()))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save(&self) -> Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        let path = token_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }

    pub fn expired(&self) -> bool {
        now() + 60 >= self.expires_at
    }

    fn from_response(resp: TokenResponse, old: Option<&Token>) -> Result<Self> {
        let refresh_token = resp
            .refresh_token
            .or_else(|| old.map(|t| t.refresh_token.clone()))
            .ok_or_else(|| anyhow!("Spotify returned no refresh token"))?;
        Ok(Token {
            access_token: resp.access_token,
            refresh_token,
            expires_at: now() + resp.expires_in,
            scope: resp.scope.or_else(|| old.and_then(|t| t.scope.clone())),
        })
    }
}

fn random_string(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

/// PKCE S256 challenge for a code verifier (RFC 7636).
pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub fn authorize_url(cfg: &Config, challenge: &str, state: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", &cfg.client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &cfg.redirect_uri())
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", challenge)
        .append_pair("state", state)
        .append_pair("scope", SCOPES)
        .finish();
    format!("{AUTHORIZE_URL}?{query}")
}

/// Reads the auth code from an HTTP request line such as
/// `GET /callback?code=...&state=... HTTP/1.1`.
/// Returns Ok(None) for requests that are not the callback (e.g. /favicon.ico).
pub fn parse_callback(request_line: &str, expected_state: &str) -> Result<Option<String>> {
    let target = request_line.split_whitespace().nth(1).unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/callback" {
        return Ok(None);
    }
    let mut code = None;
    let mut state = None;
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => bail!("Spotify refused the login: {value}"),
            _ => {}
        }
    }
    if state.as_deref() != Some(expected_state) {
        bail!("OAuth state mismatch; try `spotlite auth` again");
    }
    code.map(Some).ok_or_else(|| anyhow!("callback has no code"))
}

fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String> {
    for stream in listener.incoming() {
        let mut stream = stream?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line)?;
        let result = parse_callback(&line, state);
        let body = match &result {
            Ok(Some(_)) => "spotlite is logged in. You can close this tab.",
            Ok(None) => "",
            Err(_) => "spotlite login failed. Look at the terminal.",
        };
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        if let Some(code) = result? {
            return Ok(code);
        }
    }
    bail!("callback listener closed")
}

fn token_request(form: &[(&str, &str)]) -> Result<TokenResponse> {
    match ureq::post(TOKEN_URL).send_form(form) {
        Ok(resp) => Ok(resp.into_json()?),
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("token request failed ({code}): {body}")
        }
        Err(e) => Err(e.into()),
    }
}

/// Runs the Authorization Code + PKCE flow and saves the token.
pub fn login(cfg: &Config) -> Result<Token> {
    let verifier = random_string(64);
    let state = random_string(16);
    let listener = TcpListener::bind(("127.0.0.1", cfg.redirect_port))
        .with_context(|| format!("cannot listen on 127.0.0.1:{}", cfg.redirect_port))?;

    let url = authorize_url(cfg, &challenge_for(&verifier), &state);
    println!("Log in to Spotify in your browser. If it does not open, go to:\n\n{url}\n");
    let _ = open::that_detached(&url);

    let code = wait_for_code(&listener, &state)?;
    let redirect_uri = cfg.redirect_uri();
    let resp = token_request(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", &redirect_uri),
        ("client_id", &cfg.client_id),
        ("code_verifier", &verifier),
    ])?;
    let token = Token::from_response(resp, None)?;
    token.save()?;
    Ok(token)
}

pub fn refresh(cfg: &Config, token: &Token) -> Result<Token> {
    let resp = token_request(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", &token.refresh_token),
        ("client_id", &cfg.client_id),
    ])?;
    let token = Token::from_response(resp, Some(token))?;
    token.save()?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc7636_example() {
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn authorize_url_has_pkce_params() {
        let cfg: Config = toml::from_str(r#"client_id = "abc""#).unwrap();
        let url = authorize_url(&cfg, "CH", "ST");
        assert!(url.starts_with(AUTHORIZE_URL));
        assert!(url.contains("client_id=abc"));
        assert!(url.contains("code_challenge=CH"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8898%2Fcallback"));
    }

    #[test]
    fn callback_parsing() {
        let ok = parse_callback("GET /callback?code=xyz&state=s1 HTTP/1.1", "s1").unwrap();
        assert_eq!(ok.as_deref(), Some("xyz"));
        assert!(parse_callback("GET /favicon.ico HTTP/1.1", "s1").unwrap().is_none());
        assert!(parse_callback("GET /callback?code=xyz&state=bad HTTP/1.1", "s1").is_err());
        assert!(parse_callback("GET /callback?error=access_denied&state=s1 HTTP/1.1", "s1").is_err());
    }

    #[test]
    fn refresh_keeps_old_refresh_token() {
        let old = Token {
            access_token: "x".into(),
            refresh_token: "old".into(),
            expires_at: 0,
            scope: Some("a b".into()),
        };
        let resp = TokenResponse {
            access_token: "a".into(),
            refresh_token: None,
            expires_in: 3600,
            scope: None,
        };
        let token = Token::from_response(resp, Some(&old)).unwrap();
        assert_eq!(token.refresh_token, "old");
        assert_eq!(token.scope.as_deref(), Some("a b"));
        assert!(!token.expired());
    }
}
