//! The `yoinks://` link scheme, e.g. `yoinks://add?path=/Users/me/a.pdf`,
//! `yoinks://add?text=Hello`, `yoinks://add?url=https://example.com`,
//! `yoinks://show`, `yoinks://recall`.
//!
//! Links can be opened by any web page (after the browser asks), so only
//! commands that cannot lose data are accepted: no `clear`.

use std::path::PathBuf;

use super::protocol::Request;

pub fn parse(link: &str) -> Result<Request, String> {
    let rest = link
        .strip_prefix("yoinks://")
        .or_else(|| link.strip_prefix("yoinks:"))
        .ok_or_else(|| format!("not a yoinks link: {link}"))?;
    let (command, query) = rest.split_once('?').unwrap_or((rest, ""));
    let command = command.trim_end_matches('/');
    let params: Vec<(String, String)> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Ok((percent_decode(key)?, percent_decode(value)?))
        })
        .collect::<Result<_, String>>()?;
    let first = |name: &str| {
        params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };

    match command {
        "add" => {
            let paths: Vec<PathBuf> = params
                .iter()
                .filter(|(key, _)| key == "path")
                .map(|(_, value)| PathBuf::from(value))
                .collect();
            if let Some(relative) = paths.iter().find(|p| !p.is_absolute()) {
                return Err(format!("paths must be absolute: {}", relative.display()));
            }
            let request = Request::Add {
                paths,
                text: first("text"),
                url: first("url"),
                title: first("title"),
            };
            match &request {
                Request::Add {
                    paths,
                    text: None,
                    url: None,
                    ..
                } if paths.is_empty() => Err("add needs path, text or url".to_owned()),
                _ => Ok(request),
            }
        }
        "show" => Ok(Request::Show),
        "hide" => Ok(Request::Hide),
        "recall" => Ok(Request::Recall),
        other => Err(format!("unsupported link command: {other:?}")),
    }
}

/// Decodes `%XX` escapes (UTF-8). `+` is kept as is.
fn percent_decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| format!("bad escape in {text:?}"))?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| format!("invalid UTF-8 in {text:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_links() {
        assert_eq!(
            parse("yoinks://add?path=/tmp/a%20b.txt&path=/tmp/c.txt").unwrap(),
            Request::add_paths(vec!["/tmp/a b.txt".into(), "/tmp/c.txt".into()])
        );
        let text = parse("yoinks://add?text=Hello%2C%20w%C3%B6rld").unwrap();
        assert!(matches!(text, Request::Add { text: Some(t), .. } if t == "Hello, wörld"));
        let link = parse("yoinks://add?url=https%3A%2F%2Fexample.com%2F%3Fq%3D1&title=Ex").unwrap();
        assert!(
            matches!(link, Request::Add { url: Some(u), title: Some(t), .. }
            if u == "https://example.com/?q=1" && t == "Ex")
        );
    }

    #[test]
    fn simple_commands() {
        assert_eq!(parse("yoinks://show").unwrap(), Request::Show);
        assert_eq!(parse("yoinks://recall/").unwrap(), Request::Recall);
        assert_eq!(parse("yoinks:hide").unwrap(), Request::Hide);
    }

    #[test]
    fn rejects_unsafe_or_malformed_links() {
        assert!(parse("yoinks://clear").is_err(), "no destructive commands");
        assert!(parse("yoinks://add").is_err());
        assert!(parse("yoinks://add?path=relative/file").is_err());
        assert!(parse("yoinks://add?text=%zz").is_err());
        assert!(parse("https://add?text=x").is_err());
    }
}
