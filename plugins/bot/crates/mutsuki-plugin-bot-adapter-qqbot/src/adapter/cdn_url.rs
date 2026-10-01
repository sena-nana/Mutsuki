use url::Url;

pub(crate) fn upgrade_qq_cdn_https(url: &str) -> String {
    let trimmed = url.trim();
    let https = if trimmed.starts_with("//") {
        format!("https:{trimmed}")
    } else if trimmed
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))
    {
        // `get` rather than `len() >= 7` plus `[..7]`: these URLs are inbound gateway
        // fields, and byte length says nothing about char boundaries, so a value
        // starting with a multi-byte character used to panic here instead of falling
        // through. Past this point byte 7 is known to be a boundary.
        format!("https://{}", &trimmed[7..])
    } else {
        return trimmed.to_owned();
    };
    match Url::parse(&https) {
        Ok(parsed)
            if parsed.scheme() == "https" && is_qq_image_cdn(parsed.host_str().unwrap_or("")) =>
        {
            https
        }
        _ => trimmed.to_owned(),
    }
}

fn is_qq_image_cdn(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    ["qlogo.cn", "qpic.cn", "gtimg.cn", "qq.com.cn", "qq.com"]
        .into_iter()
        .any(|suffix| host == suffix || host.ends_with(&format!(".{suffix}")))
}

#[cfg(test)]
mod tests {
    use super::upgrade_qq_cdn_https;

    #[test]
    fn upgrades_http_qq_cdn_only() {
        assert_eq!(
            upgrade_qq_cdn_https("http://thirdqq.qlogo.cn/g?b=oidb&k=TEST&s=0"),
            "https://thirdqq.qlogo.cn/g?b=oidb&k=TEST&s=0"
        );
        assert_eq!(
            upgrade_qq_cdn_https("//gchat.qpic.cn/pic"),
            "https://gchat.qpic.cn/pic"
        );
        assert_eq!(
            upgrade_qq_cdn_https("https://q.qlogo.cn/qqapp/APP/USER/640"),
            "https://q.qlogo.cn/qqapp/APP/USER/640"
        );
        assert_eq!(
            upgrade_qq_cdn_https("http://example.test/bot.png"),
            "http://example.test/bot.png"
        );
    }

    /// The avatar and attachment URLs reaching this helper are inbound gateway
    /// fields, so a non-ASCII value is remote input rather than a malformed
    /// constant. Byte length says nothing about char boundaries.
    #[test]
    fn non_ascii_url_is_returned_untouched_instead_of_panicking() {
        assert_eq!(upgrade_qq_cdn_https("中文中"), "中文中");
        assert_eq!(upgrade_qq_cdn_https("  中文中文  "), "中文中文");
        assert_eq!(upgrade_qq_cdn_https("héllo-über-cdn"), "héllo-über-cdn");
    }
}
