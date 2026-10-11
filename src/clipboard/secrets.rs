//! Spotting copied text that is probably a secret (API key, token, private
//! key, password line), so it can be kept out of the clipboard history.
//!
//! Three checks, any of which flags the text:
//! 1. Known token formats with distinctive prefixes, anywhere in the text.
//! 2. `NAME=value` / `NAME: value` lines whose name mentions a key, token,
//!    secret or password.
//! 3. Text that is a single long, random-looking string.
//!
//! It errs on the side of recording: URLs, paths, UUIDs and hex hashes (git
//! commits, checksums) are not treated as secrets on their own.

/// Prefixes of well-known token formats, with the minimum length of the
/// whole token.
const KNOWN_PREFIXES: &[(&str, usize)] = &[
    ("ghp_", 30),        // GitHub personal access token
    ("gho_", 30),        // GitHub OAuth
    ("ghu_", 30),        // GitHub user-to-server
    ("ghs_", 30),        // GitHub server-to-server
    ("ghr_", 30),        // GitHub refresh
    ("github_pat_", 40), // GitHub fine-grained token
    ("glpat-", 20),      // GitLab
    ("xoxb-", 20),       // Slack bot
    ("xoxp-", 20),       // Slack user
    ("xoxa-", 20),       // Slack app
    ("xoxr-", 20),       // Slack refresh
    ("xapp-", 20),       // Slack app-level
    ("sk_live_", 20),    // Stripe secret
    ("rk_live_", 20),    // Stripe restricted
    ("sk_test_", 20),    // Stripe test secret
    ("sk-ant-", 30),     // Anthropic
    ("sk-proj-", 30),    // OpenAI project key
    ("sk-", 40),         // OpenAI and others
    ("AIza", 39),        // Google API key
    ("npm_", 36),        // npm
    ("pypi-", 50),       // PyPI
    ("SG.", 60),         // SendGrid
    ("shpat_", 30),      // Shopify
    ("dop_v1_", 60),     // DigitalOcean
    ("hf_", 30),         // Hugging Face
];

/// Words in a setting name that mark its value as secret.
const SENSITIVE_NAMES: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "apikey",
    "api_key",
    "api-key",
    "access_key",
    "private_key",
    "client_secret",
    "auth",
];

pub fn looks_like_secret(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    text.contains("PRIVATE KEY-----")
        || text.split(is_separator).any(is_known_token)
        || text.lines().any(is_credential_line)
        || is_random_token(text)
}

fn is_separator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | '`' | ',' | ';' | '(' | ')' | '<' | '>' | '=' | '&' | '?' | '#'
        )
}

fn is_known_token(word: &str) -> bool {
    let token_chars = |s: &str| {
        s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    };
    if KNOWN_PREFIXES
        .iter()
        .any(|(prefix, min)| word.starts_with(prefix) && word.len() >= *min && token_chars(word))
    {
        return true;
    }
    is_aws_key_id(word) || is_jwt(word)
}

/// `AKIA` / `ASIA` followed by 16 upper-case letters or digits.
fn is_aws_key_id(word: &str) -> bool {
    word.len() == 20
        && (word.starts_with("AKIA") || word.starts_with("ASIA"))
        && word[4..]
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// Three base64url parts, the first two starting with `eyJ` (`{"`).
fn is_jwt(word: &str) -> bool {
    let parts: Vec<&str> = word.split('.').collect();
    parts.len() == 3
        && parts[0].starts_with("eyJ")
        && parts[1].starts_with("eyJ")
        && word.len() >= 40
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

/// `API_KEY=abc123...`, `"password": "hunter22"`, `export TOKEN=...`.
fn is_credential_line(line: &str) -> bool {
    let Some((name, value)) = line.split_once('=').or_else(|| line.split_once(':')) else {
        return false;
    };
    let name = name
        .trim()
        .trim_start_matches("export ")
        .trim_matches(|c| c == '"' || c == '\'')
        .to_ascii_lowercase();
    let value = value
        .trim()
        .trim_end_matches(',')
        .trim_matches(|c| c == '"' || c == '\'');
    let sensitive = SENSITIVE_NAMES.iter().any(|word| name.contains(word));
    // Names are short identifiers; this skips prose like "Note: the token".
    let identifier = !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ' '));
    sensitive
        && identifier
        && value.len() >= 8
        && !value.contains(char::is_whitespace)
        && !value.starts_with("http")
        && !matches!(value, "null" | "none" | "true" | "false")
}

/// A single word of 24 to 512 characters that mixes upper case, lower case
/// and digits and is close to random (high character entropy).
fn is_random_token(text: &str) -> bool {
    if !(24..=512).contains(&text.len()) || text.contains(char::is_whitespace) {
        return false;
    }
    let not_secret_shapes = text.contains("://")
        || text.starts_with('/')
        || text.starts_with('~')
        || text.contains('@')
        || is_uuid(text)
        || text.chars().all(|c| c.is_ascii_hexdigit());
    if not_secret_shapes {
        return false;
    }
    let has = |f: fn(&char) -> bool| text.chars().any(|c| f(&c));
    let mixed =
        has(char::is_ascii_uppercase) && has(char::is_ascii_lowercase) && has(char::is_ascii_digit);
    mixed && entropy(text) >= 4.2
}

fn is_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|g| g.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Shannon entropy in bits per character.
fn entropy(text: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    for c in text.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
    }
    let len = text.chars().count() as f64;
    counts
        .values()
        .map(|&n| {
            let p = n as f64 / len;
            -p * p.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake tokens are built from a prefix and a body so the source holds
    /// nothing a secret scanner would flag.
    fn fake(prefix: &str, body: &str) -> String {
        format!("{prefix}{body}")
    }

    #[test]
    fn known_token_formats() {
        for secret in [
            fake("ghp_", "1234567890abcdefghijABCDEFGHIJ123456"),
            fake(
                "github_pat_",
                "11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyz",
            ),
            fake("glpat-", "AbCdEfGhIjKlMnOpQrSt"),
            fake(
                "xoxb-",
                "123456789012-1234567890123-AbCdEfGhIjKlMnOpQrStUvWx",
            ),
            fake("sk_live_", "51HabcdefghijklmnopqrstuvwxyzABCD"),
            fake("sk-ant-", "api03-abcdefghijklmnopqrstuvwxyz0123456789"),
            fake("sk-proj-", "abcdefghijklmnopqrstuvwxyz0123456789ABCD"),
            fake("AIza", "SyA1234567890abcdefghijklmnopqrstuv"),
            fake("AKIA", "IOSFODNN7EXAMPLE"),
            fake(
                "eyJhbGciOiJIUzI1NiJ9.",
                "eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
            ),
        ] {
            assert!(looks_like_secret(&secret), "{secret}");
        }
    }

    #[test]
    fn tokens_inside_sentences_and_code() {
        let github = fake("ghp_", "1234567890abcdefghijABCDEFGHIJ123456");
        let google = fake("AIza", "SyA1234567890abcdefghijklmnopqrstuv");
        assert!(looks_like_secret(&format!("my token is {github} ok?")));
        assert!(looks_like_secret(&format!("const key = \"{google}\";")));
        assert!(looks_like_secret(&format!(
            "https://api.example.com/v1?token={github}&page=2"
        )));
        let begin = fake("-----BEGIN OPENSSH ", "PRIVATE KEY-----");
        assert!(looks_like_secret(&format!(
            "{begin}\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----"
        )));
    }

    #[test]
    fn credential_lines() {
        assert!(looks_like_secret("DATABASE_PASSWORD=hunter2hunter2"));
        assert!(looks_like_secret("export API_KEY=9f8e7d6c5b4a3210"));
        assert!(looks_like_secret(
            "HOST=db.local\nSTRIPE_SECRET=abcdef123456\nPORT=5432"
        ));
        assert!(looks_like_secret(
            "  \"client_secret\": \"Zx9-qQ7_wE3rT5yU\","
        ));
        assert!(!looks_like_secret("PORT=5432"));
        assert!(
            !looks_like_secret("password: hunter"),
            "too short to bother"
        );
        assert!(!looks_like_secret("Note: the token expires tomorrow"));
        assert!(!looks_like_secret("auth_url=https://example.com/login"));
    }

    #[test]
    fn random_looking_strings() {
        assert!(looks_like_secret("Qm9vdHN0cmFwVG9rZW4xMjM0NTY3ODkwYWJj"));
        assert!(looks_like_secret("pX7rT2kL9qW4zM8nB3vC6yH1jF5gD0sA"));
    }

    #[test]
    fn ordinary_text_is_kept() {
        for text in [
            "Hello, world",
            "Meeting notes: ship Phase 2",
            "https://example.com/docs/page?id=AbC123xYz789LmNoPqRsTuV",
            "/Users/me/Library/Application Support/Yoinks/settings.json",
            "550e8400-e29b-41d4-a716-446655440000",
            "3844081559cb6eebda9fe408261c7a098597b68b",
            "a8ae4fa7974189be7701cd03d3ceb1797d5b35f36c6ac65da5d562165306597f",
            "mglanzer@gmail.com",
            "SupercalifragilisticExpialidocious",
            "let total_count = items.len();",
            "",
        ] {
            assert!(!looks_like_secret(text), "{text}");
        }
    }

    #[test]
    fn entropy_values() {
        assert!(entropy("aaaaaaaa") < 0.01);
        assert!((entropy("abcd") - 2.0).abs() < 1e-9);
    }
}
