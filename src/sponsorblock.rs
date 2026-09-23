use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::{collections::HashSet, time::Duration};

const SKIP_SEGMENTS_URL: &str = "https://sponsor.ajay.app/api/skipSegments";
const MAX_RESPONSE_SIZE: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct SponsorBlockClient {
    client: Client,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub category: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SegmentResponse {
    segment: [f64; 2],
    category: String,
    action_type: String,
}

impl SponsorBlockClient {
    pub fn new() -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .user_agent(concat!("ytui/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| format!("could not create SponsorBlock client: {error}"))?;
        Ok(Self { client })
    }

    pub async fn fetch_segments(
        &self,
        video_id: &str,
        categories: &[String],
    ) -> Result<Vec<Segment>, String> {
        if categories.is_empty() {
            return Ok(Vec::new());
        }
        let categories_json = serde_json::to_string(categories)
            .map_err(|error| format!("could not encode SponsorBlock categories: {error}"))?;
        let mut response = self
            .client
            .get(SKIP_SEGMENTS_URL)
            .query(&[
                ("videoID", video_id),
                ("categories", categories_json.as_str()),
                ("actionTypes", "[\"skip\"]"),
            ])
            .send()
            .await
            .map_err(|error| format!("SponsorBlock request failed: {error}"))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        let status = response.status();
        if !status.is_success() {
            return Err(format!("SponsorBlock returned {status}"));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("could not read SponsorBlock response: {error}"))?
        {
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_SIZE {
                return Err("SponsorBlock response exceeded 2 MiB".to_owned());
            }
            body.extend_from_slice(&chunk);
        }
        let body = String::from_utf8(body)
            .map_err(|error| format!("SponsorBlock response was not UTF-8: {error}"))?;
        parse_segments(&body, categories)
    }
}

fn parse_segments(body: &str, categories: &[String]) -> Result<Vec<Segment>, String> {
    let entries = serde_json::from_str::<Vec<serde_json::Value>>(body)
        .map_err(|error| format!("could not parse SponsorBlock response: {error}"))?;
    let categories = categories
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut segments = entries
        .into_iter()
        .filter_map(|entry| serde_json::from_value::<SegmentResponse>(entry).ok())
        .filter(|entry| entry.action_type == "skip" && categories.contains(entry.category.as_str()))
        .filter_map(|entry| {
            let [start, end] = entry.segment;
            (start.is_finite() && end.is_finite() && start >= 0.0 && end > start).then_some(
                Segment {
                    category: entry.category,
                    start,
                    end,
                },
            )
        })
        .collect::<Vec<_>>();
    segments.sort_by(|left, right| left.start.total_cmp(&right.start));
    Ok(segments)
}

pub fn skip_target(position: f64, segments: &[Segment], enabled: &[&str]) -> Option<f64> {
    if !position.is_finite() {
        return None;
    }
    let enabled = enabled.iter().copied().collect::<HashSet<_>>();
    let mut target = segments
        .iter()
        .filter(|segment| enabled.contains(segment.category.as_str()))
        .filter(|segment| segment.start <= position && position < segment.end)
        .map(|segment| segment.end)
        .max_by(f64::total_cmp)?;

    loop {
        let extended = segments
            .iter()
            .filter(|segment| enabled.contains(segment.category.as_str()))
            .filter(|segment| segment.start <= target && target < segment.end)
            .map(|segment| segment.end)
            .max_by(f64::total_cmp)
            .unwrap_or(target);
        if extended <= target {
            return Some(target);
        }
        target = extended;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_valid_requested_skip_segments() {
        let body = r#"[
            {"segment":[10,20],"category":"sponsor","actionType":"skip"},
            {"segment":[1,2],"category":"intro","actionType":"mute"},
            {"segment":[3,4],"category":"outro","actionType":"skip"},
            {"segment":[8,7],"category":"sponsor","actionType":"skip"},
            {"segment":"invalid","category":"sponsor","actionType":"skip"}
        ]"#;

        let segments =
            parse_segments(body, &["sponsor".to_owned()]).expect("response should parse");

        assert_eq!(
            segments,
            [Segment {
                category: "sponsor".to_owned(),
                start: 10.0,
                end: 20.0,
            }]
        );
    }

    #[test]
    fn finds_target_at_start_but_not_end() {
        let segments = [segment("sponsor", 10.0, 20.0)];

        assert_eq!(skip_target(10.0, &segments, &["sponsor"]), Some(20.0));
        assert_eq!(skip_target(20.0, &segments, &["sponsor"]), None);
    }

    #[test]
    fn ignores_disabled_categories() {
        let segments = [segment("intro", 0.0, 10.0)];

        assert_eq!(skip_target(5.0, &segments, &["sponsor"]), None);
    }

    #[test]
    fn extends_target_across_touching_enabled_segments() {
        let segments = [
            segment("sponsor", 10.0, 20.0),
            segment("intro", 15.0, 25.0),
            segment("sponsor", 25.0, 30.0),
        ];

        assert_eq!(
            skip_target(12.0, &segments, &["sponsor", "intro"]),
            Some(30.0)
        );
    }

    fn segment(category: &str, start: f64, end: f64) -> Segment {
        Segment {
            category: category.to_owned(),
            start,
            end,
        }
    }
}
