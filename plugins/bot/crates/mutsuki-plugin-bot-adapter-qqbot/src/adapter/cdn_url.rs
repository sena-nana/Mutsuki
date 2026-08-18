use url::Url;

pub(crate) fn upgrade_qq_cdn_https(url: &str) -> String {
    let trimmed = url.trim();
    let https = if trimmed.starts_with("//") {
        format!("https:{trimmed}")
    } else if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("http://") {
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
}
