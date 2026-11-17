//! Strict URL validation shared by all builds of this crate.
//!
//! Only `http` and `https` are accepted. The URL is passed to the host as a
//! single argument, never as a shell command string, but rejecting other
//! schemes still prevents accidental `file:`, `javascript:` or custom-protocol
//! launches.

/// Maximum accepted URL length. Long enough for practical links, short enough
/// to bound host handoff.
pub(crate) const MAX_URL_LENGTH: usize = 8_192;

/// Validates an absolute `http`/`https` URL with a non-empty authority.
pub(crate) fn validate_http_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("URL must not be empty".to_owned());
    }
    if url.len() > MAX_URL_LENGTH {
        return Err(format!("URL exceeds the {MAX_URL_LENGTH}-byte limit"));
    }
    if let Some(character) = url
        .chars()
        .find(|character| character.is_control() || character.is_whitespace())
    {
        return Err(format!(
            "URL contains whitespace or control character U+{:04X}",
            character as u32
        ));
    }
    if url.contains('\\') {
        return Err("URL must use forward slashes, not backslashes".to_owned());
    }

    let separator = url
        .find("://")
        .ok_or_else(|| "URL must include an explicit http:// or https:// scheme".to_owned())?;
    let scheme = &url[..separator];
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err("only http and https URLs are allowed".to_owned());
    }

    let after_scheme = &url[separator + 3..];
    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..authority_end];
    if authority.is_empty() {
        return Err("URL must include a non-empty host".to_owned());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https() {
        assert!(validate_http_url("https://example.com").is_ok());
        assert!(validate_http_url("http://localhost:8080/path?q=1#fragment").is_ok());
        assert!(validate_http_url("HTTP://EXAMPLE.COM/Path").is_ok());
        assert!(validate_http_url("https://example.com/?q=a%20b").is_ok());
    }

    #[test]
    fn rejects_non_web_schemes() {
        for url in [
            "file:///C:/Windows/System32/calc.exe",
            "ftp://example.com/file",
            "javascript:alert(1)",
            "mailto:user@example.com",
            "ms-msdt:/id",
            "shell:AppsFolder",
            "cmd:/c calc",
        ] {
            assert!(
                validate_http_url(url).is_err(),
                "expected rejection for {url}"
            );
        }
    }

    #[test]
    fn rejects_missing_or_empty_authority() {
        for url in [
            "https://",
            "http:///path",
            "https://?query",
            "https://#fragment",
        ] {
            assert!(
                validate_http_url(url).is_err(),
                "expected rejection for {url}"
            );
        }
        assert!(validate_http_url("example.com").is_err());
        assert!(validate_http_url("//example.com").is_err());
    }

    #[test]
    fn rejects_whitespace_controls_and_backslashes() {
        assert!(validate_http_url("https://exa mple.com").is_err());
        assert!(validate_http_url("https://example.com/\nnext").is_err());
        assert!(validate_http_url("https://example.com/\tvalue").is_err());
        assert!(validate_http_url("https://example.com/\u{0000}").is_err());
        assert!(validate_http_url(r"https://example.com\@evil.example").is_err());
    }

    #[test]
    fn rejects_empty_and_oversized_urls() {
        assert!(validate_http_url("").is_err());
        let oversized = format!("https://example.com/{}", "a".repeat(MAX_URL_LENGTH));
        assert!(validate_http_url(&oversized).is_err());
    }
}
