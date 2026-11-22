//! Keep provider credentials out of logs and error messages. Xtream and most M3U
//! URLs carry the account in the query (`username=`/`password=`) or the path
//! (`/live/<user>/<pass>/<id>.ts`), and ureq puts the full URL in its errors.

/// Query keys whose values are secrets.
const SECRET_KEYS: &[&str] = &["username", "password", "user", "pass", "token", "key"];

/// Path prefixes Xtream uses for `/<kind>/<user>/<pass>/...` stream URLs.
const XTREAM_KINDS: &[&str] = &["live", "movie", "series", "timeshift"];

/// `url` with credentials replaced by `***`: userinfo, secret query values and
/// the user/password segments of Xtream stream paths.
pub fn redact_url(url: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (url, None),
    };

    let mut out = match base.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = match rest.find('/') {
                Some(i) => rest.split_at(i),
                None => (rest, ""),
            };
            let authority = match authority.rsplit_once('@') {
                Some((_, host)) => format!("***@{host}"),
                None => authority.to_string(),
            };
            format!("{scheme}://{authority}{}", redact_path(path))
        }
        None => base.to_string(),
    };

    if let Some(q) = query {
        let pairs: Vec<String> = q
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((k, _)) if SECRET_KEYS.contains(&k.to_ascii_lowercase().as_str()) => {
                    format!("{k}=***")
                }
                _ => pair.to_string(),
            })
            .collect();
        out.push('?');
        out.push_str(&pairs.join("&"));
    }
    out
}

fn redact_path(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').collect();
    // ["", kind, user, pass, ...]
    if segments.len() > 4 && XTREAM_KINDS.contains(&segments[1]) {
        let mut s = segments.clone();
        s[2] = "***";
        s[3] = "***";
        return s.join("/");
    }
    path.to_string()
}

/// A ureq error as an anyhow error with the URL redacted. Use with `map_err`
/// before adding context, since ureq's own message includes the full URL.
pub fn http_error(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(code, resp) => {
            anyhow::anyhow!("{}: HTTP {code}", redact_url(resp.get_url()))
        }
        ureq::Error::Transport(t) => {
            let url = t.url().map(|u| redact_url(u.as_str()));
            let mut msg = match url {
                Some(u) => format!("{u}: {}", t.kind()),
                None => t.kind().to_string(),
            };
            if let Some(m) = t.message() {
                msg.push_str(&format!(": {m}"));
            }
            anyhow::anyhow!(msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_query_credentials() {
        assert_eq!(
            redact_url("http://p.example:8080/get.php?username=bob&password=s3cret&type=m3u_plus"),
            "http://p.example:8080/get.php?username=***&password=***&type=m3u_plus"
        );
    }

    #[test]
    fn redacts_xtream_stream_paths() {
        assert_eq!(
            redact_url("http://p.example/live/bob/s3cret/123.ts"),
            "http://p.example/live/***/***/123.ts"
        );
    }

    #[test]
    fn redacts_userinfo() {
        assert_eq!(
            redact_url("https://bob:s3cret@epg.example/guide.xml"),
            "https://***@epg.example/guide.xml"
        );
    }

    #[test]
    fn leaves_plain_urls_alone() {
        let url = "https://epg.example/guide.xml?country=us";
        assert_eq!(redact_url(url), url);
    }

    #[test]
    fn http_error_hides_credentials() {
        // Port 9 on localhost is closed, so this fails fast with a transport error.
        let err = ureq::get("http://127.0.0.1:9/player_api.php?username=bob&password=s3cret")
            .call()
            .map_err(http_error)
            .unwrap_err()
            .to_string();
        assert!(!err.contains("s3cret") && !err.contains("bob"), "{err}");
        assert!(err.contains("password=***"), "{err}");
    }
}
