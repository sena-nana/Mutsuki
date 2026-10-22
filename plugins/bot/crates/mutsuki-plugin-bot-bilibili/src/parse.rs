//! Parsing of upstream poll responses and browser snapshots into Bilibili items.

use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use url::Url;

use crate::transport::ensure_bilibili_domain;
use crate::{BilibiliError, BilibiliItem, BilibiliPollKind};

pub(crate) fn parse_dynamic_snapshot(html: &str) -> Result<Vec<BilibiliItem>, BilibiliError> {
    let document = Html::parse_document(html);
    let cards = Selector::parse(".bili-dyn-list__item")
        .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
    let links = Selector::parse("a[href]")
        .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
    let images = Selector::parse("img[src]")
        .map_err(|error| BilibiliError::InvalidResponse(error.to_string()))?;
    let mut items = Vec::new();
    for card in document.select(&cards) {
        let Some((id, url)) = card.select(&links).find_map(|link| {
            let href = link.value().attr("href")?;
            dynamic_id_and_url(href)
        }) else {
            continue;
        };
        let title = first_card_text(
            card,
            &[
                ".bili-rich-text",
                ".bili-dyn-card-video__title",
                ".bili-dyn-card-opus__summary",
            ],
        )
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "新动态".into())
        .chars()
        .take(80)
        .collect();
        let image_url = card
            .select(&images)
            .filter_map(|image| image.value().attr("src"))
            .filter_map(normalize_browser_url)
            .find(|url| ensure_bilibili_domain(url).is_ok());
        items.push(BilibiliItem {
            id,
            title,
            url,
            image_url,
        });
    }
    Ok(items)
}

fn first_card_text(card: ElementRef<'_>, selectors: &[&str]) -> Option<String> {
    selectors.iter().find_map(|selector| {
        let selector = Selector::parse(selector).ok()?;
        let text = card
            .select(&selector)
            .next()?
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        (!text.is_empty()).then_some(text)
    })
}

fn dynamic_id_and_url(value: &str) -> Option<(String, String)> {
    let url = normalize_browser_url(value)?;
    ensure_bilibili_domain(&url).ok()?;
    let parsed = Url::parse(&url).ok()?;
    let host = parsed.host_str()?;
    let segments = parsed.path_segments()?.collect::<Vec<_>>();
    let id = match (host, segments.as_slice()) {
        ("t.bilibili.com", [id]) => *id,
        ("bilibili.com" | "www.bilibili.com", ["opus", id]) => *id,
        _ => return None,
    };
    id.chars()
        .all(|character| character.is_ascii_digit())
        .then(|| (id.to_owned(), format!("https://t.bilibili.com/{id}")))
}

fn normalize_browser_url(value: &str) -> Option<String> {
    let value = value.trim();
    if value.starts_with("//") {
        Some(format!("https:{value}"))
    } else if value.starts_with('/') {
        Some(format!("https://www.bilibili.com{value}"))
    } else if value.starts_with("https://") {
        Some(value.to_owned())
    } else {
        None
    }
}

pub(crate) fn parse_poll_items(
    kind: &BilibiliPollKind,
    uid: u64,
    value: Value,
) -> Result<Vec<BilibiliItem>, BilibiliError> {
    match kind {
        BilibiliPollKind::Live => {
            let live = &value["data"]["live_room"];
            let status = live
                .get("liveStatus")
                .or_else(|| live.get("live_status"))
                .and_then(Value::as_i64)
                .unwrap_or(0)
                == 1;
            Ok(vec![BilibiliItem {
                id: status.to_string(),
                title: live
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("直播")
                    .into(),
                url: live
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or(&format!("https://space.bilibili.com/{uid}"))
                    .into(),
                image_url: live
                    .get("cover")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            }])
        }
        BilibiliPollKind::Dynamic => {
            let items = value["data"]["items"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            Ok(items
                .into_iter()
                .filter_map(|item| {
                    let id = item.get("id_str")?.as_str()?.to_owned();
                    let modules = &item["modules"]["module_dynamic"];
                    Some(BilibiliItem {
                        id: id.clone(),
                        title: modules["desc"]["text"]
                            .as_str()
                            .unwrap_or("新动态")
                            .chars()
                            .take(80)
                            .collect(),
                        url: format!("https://t.bilibili.com/{id}"),
                        image_url: modules["major"]["archive"]["cover"]
                            .as_str()
                            .map(ToOwned::to_owned),
                    })
                })
                .collect())
        }
        BilibiliPollKind::Video => {
            let items = value["data"]["list"]["vlist"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            Ok(items
                .into_iter()
                .filter_map(|item| {
                    let bvid = item.get("bvid")?.as_str()?.to_owned();
                    Some(BilibiliItem {
                        id: bvid.clone(),
                        title: item["title"].as_str().unwrap_or("新投稿").into(),
                        url: format!("https://www.bilibili.com/video/{bvid}"),
                        image_url: item["pic"].as_str().map(|url| format!("https:{url}")),
                    })
                })
                .collect())
        }
    }
}

pub(crate) fn fresh_since(items: Vec<BilibiliItem>, previous: &str) -> Vec<BilibiliItem> {
    let mut fresh = items
        .into_iter()
        .take_while(|item| item.id != previous)
        .collect::<Vec<_>>();
    fresh.reverse();
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_snapshot_parser_keeps_bilibili_urls_and_normalizes_cards() {
        let items = parse_dynamic_snapshot(
            r#"<article class="bili-dyn-list__item">
                <a href="https://www.bilibili.com/opus/123456">detail</a>
                <div class="bili-rich-text">  hello   browser fallback </div>
                <img src="//i0.hdslb.com/bfs/archive/cover.jpg">
            </article>
            <article class="bili-dyn-list__item">
                <a href="https://evil.example/opus/999">denied</a>
            </article>"#,
        )
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "123456");
        assert_eq!(items[0].title, "hello browser fallback");
        assert_eq!(items[0].url, "https://t.bilibili.com/123456");
        assert_eq!(
            items[0].image_url.as_deref(),
            Some("https://i0.hdslb.com/bfs/archive/cover.jpg")
        );
    }

    #[test]
    fn dynamic_and_video_items_are_emitted_oldest_first_after_cursor() {
        let item = |id: &str| BilibiliItem {
            id: id.into(),
            title: id.into(),
            url: format!("https://www.bilibili.com/{id}"),
            image_url: None,
        };
        let fresh = fresh_since(vec![item("3"), item("2"), item("1")], "1");
        assert_eq!(
            fresh
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["2", "3"]
        );
    }
}
