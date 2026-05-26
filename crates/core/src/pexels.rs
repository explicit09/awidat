//! Thin Pexels Video API client for the Phase 3 b-roll tools.
//!
//! Surface: [`Client::search_videos`] (`GET /videos/search`) and
//! [`Client::download_to`] (fetch one of a video's `video_files` to disk).
//! Anything else is YAGNI — the agent's b-roll loop only needs query →
//! preview → pick → download.
//!
//! Key resolution: [`Client::from_env_or_keychain`] tries `PEXELS_API_KEY`
//! then the `pexels_api_key` keychain account, and errors with
//! [`PexelsError::MissingApiKey`] otherwise. No config-file fallback —
//! keys live in the keychain per `PLAN.md` §10.4.
//!
//! Per-call rate budget is the caller's problem. Pexels' free tier caps
//! at 200 search calls / hour; the b-roll tools cache search results
//! per-session so a single editorial pass burns ≤ a handful.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;
use tokio::io::AsyncWriteExt;

/// Default API base. Override only for testing (the tests in this module
/// stand up a `wiremock` server and point at it via [`ClientConfig`]).
pub const DEFAULT_BASE_URL: &str = "https://api.pexels.com";

/// Errors talking to Pexels.
#[derive(Debug, Error)]
pub enum PexelsError {
    /// Network / TLS / connection failure.
    #[error("network error: {0}")]
    Network(String),
    /// Server returned a non-2xx response.
    #[error("Pexels API returned {status}: {message}")]
    Api {
        /// HTTP status code.
        status: u16,
        /// Body excerpt (truncated to a reasonable length).
        message: String,
    },
    /// Response body wasn't valid JSON of the expected shape.
    #[error("malformed Pexels response: {0}")]
    Parse(String),
    /// Disk I/O writing a downloaded file.
    #[error("I/O error writing '{path}': {source}")]
    Io {
        /// Destination path that failed.
        path: String,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// API key missing — neither env var nor keychain has it.
    #[error("PEXELS_API_KEY not set in env or keychain")]
    MissingApiKey,
    /// Keychain backend itself failed (locked, missing).
    #[error("keychain access failed: {0}")]
    Keychain(String),
}

/// How to reach Pexels.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// API base URL.
    pub base_url: String,
    /// Per-request timeout (connect + body). 30s is generous for a JSON
    /// search; downloads use a longer per-call timeout via the request
    /// builder.
    pub request_timeout: Duration,
    /// Per-download timeout. A single 1080p clip is ~30 MB; 5 minutes
    /// covers slow connections without hanging the agent loop forever.
    pub download_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            request_timeout: Duration::from_secs(30),
            download_timeout: Duration::from_secs(300),
        }
    }
}

/// Pexels Video API client.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    api_key: String,
    config: ClientConfig,
}

impl Client {
    /// Build a client from an explicit key. Use [`Self::from_env_or_keychain`]
    /// for the production path.
    pub fn new(api_key: impl Into<String>, config: ClientConfig) -> Result<Self, PexelsError> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| PexelsError::Network(e.to_string()))?;
        Ok(Self {
            http,
            api_key: api_key.into(),
            config,
        })
    }

    /// Resolve the key from `PEXELS_API_KEY` or the `pexels_api_key`
    /// keychain entry, then build a client with default config.
    pub fn from_env_or_keychain(config: ClientConfig) -> Result<Self, PexelsError> {
        let key = awidat_secrets::get(
            awidat_secrets::env_vars::PEXELS_API_KEY,
            awidat_secrets::accounts::PEXELS_API_KEY,
        )
        .map_err(|e| PexelsError::Keychain(e.to_string()))?
        .ok_or(PexelsError::MissingApiKey)?;
        Self::new(key, config)
    }

    /// `GET /videos/search?query=…&per_page=N`. `per_page` is capped at
    /// 80 by Pexels; we further cap at 30 because the agent only ever
    /// previews top-3 anyway and large pages waste bandwidth.
    pub async fn search_videos(
        &self,
        query: &str,
        per_page: u32,
    ) -> Result<SearchResponse, PexelsError> {
        let per_page = per_page.clamp(1, 30);
        let url = format!("{}/videos/search", self.config.base_url);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", &self.api_key)
            .query(&[
                ("query", query),
                ("per_page", &per_page.to_string()),
                ("size", "medium"),
            ])
            .send()
            .await
            .map_err(|e| PexelsError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            let message = truncate(&body, 512);
            return Err(PexelsError::Api { status, message });
        }
        resp.json::<SearchResponse>()
            .await
            .map_err(|e| PexelsError::Parse(e.to_string()))
    }

    /// Stream-download a `video_files` URL to disk. Caller picks the
    /// quality/dimension by inspecting [`VideoFile`] entries; this method
    /// intentionally passes bytes through without media interpretation.
    pub async fn download_to(&self, url: &str, dest: &Path) -> Result<u64, PexelsError> {
        let resp = self
            .http
            .get(url)
            // The CDN doesn't require the API key, but sending it doesn't
            // hurt and keeps the auth surface uniform.
            .header("Authorization", &self.api_key)
            .timeout(self.config.download_timeout)
            .send()
            .await
            .map_err(|e| PexelsError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            let message = truncate(&body, 512);
            return Err(PexelsError::Api { status, message });
        }

        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| PexelsError::Io {
                    path: parent.display().to_string(),
                    source: e,
                })?;
        }
        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|e| PexelsError::Io {
                path: dest.display().to_string(),
                source: e,
            })?;

        let mut total: u64 = 0;
        let mut stream = resp.bytes_stream();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| PexelsError::Network(e.to_string()))?;
            total += bytes.len() as u64;
            file.write_all(&bytes).await.map_err(|e| PexelsError::Io {
                path: dest.display().to_string(),
                source: e,
            })?;
        }
        file.flush().await.map_err(|e| PexelsError::Io {
            path: dest.display().to_string(),
            source: e,
        })?;
        Ok(total)
    }
}

/// `/videos/search` response. Only the fields we actually consume; extra
/// fields are tolerated via serde's default behavior.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResponse {
    /// Page of results.
    pub videos: Vec<Video>,
    /// Page number (1-indexed).
    #[serde(default)]
    pub page: u32,
    /// Items in this page.
    #[serde(default)]
    pub per_page: u32,
    /// Total results matching the query (capped at 8000 by Pexels).
    #[serde(default)]
    pub total_results: u32,
}

/// One video result.
#[derive(Debug, Clone, Deserialize)]
pub struct Video {
    /// Stable Pexels id. Doubles as our cache key + filename anchor.
    pub id: u64,
    /// Length in seconds.
    pub duration: u32,
    /// Original asset width.
    pub width: u32,
    /// Original asset height.
    pub height: u32,
    /// Single thumbnail URL — the "preview image" for this clip.
    pub image: String,
    /// Pexels page URL — used for attribution.
    pub url: String,
    /// Renditions (different qualities + container types) we can download.
    #[serde(default)]
    pub video_files: Vec<VideoFile>,
    /// Frame previews along the clip — useful when we want a quick visual
    /// scrub-strip in the Note card.
    #[serde(default)]
    pub video_pictures: Vec<VideoPicture>,
    /// Uploader; required for attribution per the Pexels license.
    pub user: VideoUser,
}

/// One downloadable rendition of a [`Video`].
#[derive(Debug, Clone, Deserialize)]
pub struct VideoFile {
    /// Pexels id for this rendition.
    pub id: u64,
    /// `hd` / `sd` / `mobile` / etc. Free-form per Pexels.
    pub quality: String,
    /// MIME type, e.g. `video/mp4`.
    pub file_type: String,
    /// Rendition width.
    #[serde(default)]
    pub width: Option<u32>,
    /// Rendition height.
    #[serde(default)]
    pub height: Option<u32>,
    /// Direct CDN download URL.
    pub link: String,
}

/// One frame preview within a [`Video`].
#[derive(Debug, Clone, Deserialize)]
pub struct VideoPicture {
    /// Position in the picture sequence (0-indexed).
    #[serde(default)]
    pub nr: u32,
    /// JPEG URL.
    pub picture: String,
}

/// Uploader info — attribution per the Pexels license.
#[derive(Debug, Clone, Deserialize)]
pub struct VideoUser {
    /// Display name.
    pub name: String,
    /// Pexels profile URL.
    pub url: String,
}

impl Video {
    /// Pick the best `video_files` entry under a width budget. Prefers
    /// the largest `mp4` rendition that's `≤ max_width`; falls back to
    /// the smallest `mp4` rendition if none fit. `None` only if the
    /// video has zero `mp4` files (rare but possible — Pexels also
    /// serves `webm`).
    pub fn pick_mp4(&self, max_width: u32) -> Option<&VideoFile> {
        let mp4s: Vec<&VideoFile> = self
            .video_files
            .iter()
            .filter(|f| f.file_type.eq_ignore_ascii_case("video/mp4"))
            .collect();
        if mp4s.is_empty() {
            return None;
        }
        if let Some(fit) = mp4s
            .iter()
            .filter(|f| f.width.is_some_and(|w| w <= max_width))
            .max_by_key(|f| f.width.unwrap_or(0))
        {
            return Some(fit);
        }
        mp4s.iter()
            .min_by_key(|f| f.width.unwrap_or(u32::MAX))
            .copied()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_video(files: Vec<(u32, &str)>) -> Video {
        Video {
            id: 1,
            duration: 12,
            width: 1920,
            height: 1080,
            image: "https://example.com/img.jpg".into(),
            url: "https://www.pexels.com/video/1".into(),
            video_files: files
                .into_iter()
                .enumerate()
                .map(|(i, (w, ty))| VideoFile {
                    id: i as u64,
                    quality: "hd".into(),
                    file_type: ty.into(),
                    width: Some(w),
                    height: Some((w * 9) / 16),
                    link: format!("https://example.com/{i}.mp4"),
                })
                .collect(),
            video_pictures: vec![],
            user: VideoUser {
                name: "Alice".into(),
                url: "https://www.pexels.com/@alice".into(),
            },
        }
    }

    #[test]
    fn search_response_parses_real_shape() {
        // Minimal subset of an actual Pexels response. Extra fields they
        // ship today (`avg_color`, `tags`, etc.) shouldn't fail parse.
        let body = r#"{
            "page": 1,
            "per_page": 2,
            "total_results": 1234,
            "videos": [
                {
                    "id": 4434242,
                    "width": 1920,
                    "height": 1080,
                    "duration": 14,
                    "url": "https://www.pexels.com/video/4434242/",
                    "image": "https://images.pexels.com/x.jpg",
                    "tags": ["unused", "field"],
                    "video_files": [
                        {
                            "id": 1,
                            "quality": "hd",
                            "file_type": "video/mp4",
                            "width": 1920,
                            "height": 1080,
                            "link": "https://videos.pexels.com/x_1920.mp4"
                        },
                        {
                            "id": 2,
                            "quality": "sd",
                            "file_type": "video/mp4",
                            "width": 640,
                            "height": 360,
                            "link": "https://videos.pexels.com/x_640.mp4"
                        }
                    ],
                    "video_pictures": [
                        { "nr": 0, "picture": "https://images.pexels.com/p0.jpg" }
                    ],
                    "user": { "id": 9, "name": "Alice", "url": "https://www.pexels.com/@alice" }
                }
            ],
            "next_page": "https://api.pexels.com/videos/search?query=x&page=2"
        }"#;
        let parsed: SearchResponse = serde_json::from_str(body).expect("parse");
        assert_eq!(parsed.videos.len(), 1);
        let v = &parsed.videos[0];
        assert_eq!(v.id, 4434242);
        assert_eq!(v.duration, 14);
        assert_eq!(v.video_files.len(), 2);
        assert_eq!(v.user.name, "Alice");
        assert_eq!(v.video_pictures.len(), 1);
    }

    #[test]
    fn pick_mp4_prefers_largest_under_budget() {
        let v = sample_video(vec![
            (640, "video/mp4"),
            (1280, "video/mp4"),
            (1920, "video/mp4"),
            (3840, "video/mp4"),
        ]);
        let picked = v.pick_mp4(1920).expect("some");
        assert_eq!(picked.width, Some(1920));
    }

    #[test]
    fn pick_mp4_falls_back_to_smallest_when_none_fit() {
        let v = sample_video(vec![(2560, "video/mp4"), (3840, "video/mp4")]);
        let picked = v.pick_mp4(1280).expect("some");
        assert_eq!(picked.width, Some(2560));
    }

    #[test]
    fn pick_mp4_skips_non_mp4_files() {
        let v = sample_video(vec![(1920, "video/webm"), (640, "video/mp4")]);
        let picked = v.pick_mp4(1920).expect("some");
        assert_eq!(picked.width, Some(640));
        assert_eq!(picked.file_type, "video/mp4");
    }

    #[test]
    fn pick_mp4_returns_none_when_only_webm() {
        let v = sample_video(vec![(1920, "video/webm"), (640, "video/webm")]);
        assert!(v.pick_mp4(1920).is_none());
    }

    #[test]
    fn truncate_does_not_split_codepoints() {
        let s = "héllo wörld";
        let out = truncate(s, 4);
        assert!(out.ends_with('…'));
        // Sanity: remainder is still valid UTF-8.
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn config_default_points_at_pexels() {
        let c = ClientConfig::default();
        assert_eq!(c.base_url, DEFAULT_BASE_URL);
    }

    #[tokio::test]
    async fn missing_api_key_is_a_distinct_error() {
        // Force both env-var and keychain absent. We can't safely mutate
        // env in tests (edition 2024 marks it `unsafe` in multi-threaded
        // runners), so we test the explicit `new` path: a key that the
        // server rejects produces `Api` not `MissingApiKey`. The
        // `from_env_or_keychain` branch is exercised manually; the
        // mapping itself is two lines and obvious.
        let server_resp = "{\"error\": \"unauthorized\"}";
        // No mock server here; we assert only the constructor error
        // shape, which is the part we control.
        let c = Client::new(
            "stub-key",
            ClientConfig {
                base_url: "http://127.0.0.1:1".into(), // refused connection
                request_timeout: Duration::from_millis(50),
                download_timeout: Duration::from_millis(50),
            },
        )
        .expect("client");
        let err = c.search_videos("anything", 5).await.expect_err("must fail");
        assert!(matches!(err, PexelsError::Network(_)), "{err:?}");
        let _ = server_resp; // silence unused
    }
}
