//! X API trend-context provider for short-form planning.
//!
//! This module keeps trend reads separate from publishing. Trend reads use an
//! app/user bearer token, while posting remains part of the existing
//! social-publishing Twitter/X pipeline.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_BASE_URL: &str = "https://api.x.com";
const DEFAULT_MAX_RESULTS: u8 = 10;

#[derive(Debug, Clone)]
pub struct XClientConfig {
    pub base_url: String,
    pub request_timeout: Duration,
}

impl Default for XClientConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            request_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone)]
pub struct XClient {
    http: reqwest::Client,
    bearer_token: String,
    config: XClientConfig,
}

impl XClient {
    pub fn new(
        bearer_token: impl Into<String>,
        config: XClientConfig,
    ) -> Result<Self, XTrendError> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|error| XTrendError::Network(error.to_string()))?;
        Ok(Self {
            http,
            bearer_token: bearer_token.into(),
            config,
        })
    }

    pub fn from_env_or_keychain(config: XClientConfig) -> Result<Option<Self>, XTrendError> {
        let Some(token) = montage_secrets::get(
            montage_secrets::env_vars::X_BEARER_TOKEN,
            montage_secrets::accounts::X_BEARER_TOKEN,
        )
        .map_err(|error| XTrendError::Secret(error.to_string()))?
        else {
            return Ok(None);
        };
        Self::new(token, config).map(Some)
    }

    pub async fn recent_search(
        &self,
        query: &str,
        max_results: u8,
    ) -> Result<XRecentSearchResponse, XTrendError> {
        let url = format!("{}/2/tweets/search/recent", self.config.base_url);
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.bearer_token)
            .query(&[
                ("query", query),
                ("max_results", &max_results.clamp(10, 100).to_string()),
                ("tweet.fields", "created_at,author_id,public_metrics"),
            ])
            .send()
            .await
            .map_err(|error| XTrendError::Network(error.to_string()))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = truncate(&response.text().await.unwrap_or_default(), 512);
            return Err(XTrendError::Api { status, message });
        }

        response
            .json()
            .await
            .map_err(|error| XTrendError::Parse(error.to_string()))
    }
}

#[derive(Debug, Error)]
pub enum XTrendError {
    #[error("network error: {0}")]
    Network(String),
    #[error("X API returned {status}: {message}")]
    Api { status: u16, message: String },
    #[error("malformed X API response: {0}")]
    Parse(String),
    #[error("secret resolution failed: {0}")]
    Secret(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XTrendContextRequest {
    pub queries: Vec<String>,
    pub max_results: u8,
}

impl XTrendContextRequest {
    pub fn normalized(mut self) -> Self {
        self.queries = self
            .queries
            .into_iter()
            .map(|query| query.trim().to_string())
            .filter(|query| !query.is_empty())
            .take(5)
            .collect();
        self.max_results = if self.max_results == 0 {
            DEFAULT_MAX_RESULTS
        } else {
            self.max_results.clamp(10, 100)
        };
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XTrendContext {
    pub provider: String,
    pub capabilities: XProviderCapabilities,
    pub signals: Vec<XTrendSignal>,
    pub usage: XTrendUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XProviderCapabilities {
    pub read_trends: XCapabilityState,
    pub publish_posts: XCapabilityState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XCapabilityState {
    pub status: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XTrendSignal {
    pub source: String,
    pub label: String,
    pub keywords: Vec<String>,
    pub weight: f64,
    pub reason: String,
    pub evidence: Vec<XTrendEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XTrendEvidence {
    pub post_id: String,
    pub text: String,
    pub engagement_score: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XTrendUsage {
    pub pass_to: String,
    pub note: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct XRecentSearchResponse {
    #[serde(default)]
    pub data: Vec<XTweet>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct XTweet {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub public_metrics: XPublicMetrics,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct XPublicMetrics {
    #[serde(default)]
    pub retweet_count: u64,
    #[serde(default)]
    pub reply_count: u64,
    #[serde(default)]
    pub like_count: u64,
    #[serde(default)]
    pub quote_count: u64,
}

pub fn missing_credentials_context(queries: Vec<String>) -> XTrendContext {
    XTrendContext {
        provider: "x".to_string(),
        capabilities: capabilities(false),
        signals: Vec::new(),
        usage: XTrendUsage {
            pass_to: "plan_short_form_review.trend_context".to_string(),
            note: format!(
                "Set {} or keychain account {} to enable X trend reads for queries: {}",
                montage_secrets::env_vars::X_BEARER_TOKEN,
                montage_secrets::accounts::X_BEARER_TOKEN,
                queries.join(", ")
            ),
        },
    }
}

pub fn build_context_from_searches(
    searches: Vec<(String, XRecentSearchResponse)>,
) -> XTrendContext {
    let signals = searches
        .into_iter()
        .filter_map(|(query, response)| signal_from_response(query, response))
        .collect();
    XTrendContext {
        provider: "x".to_string(),
        capabilities: capabilities(true),
        signals,
        usage: XTrendUsage {
            pass_to: "plan_short_form_review.trend_context".to_string(),
            note: "Use matched signals as a boost only after hook, standalone clarity, and payoff pass."
                .to_string(),
        },
    }
}

fn capabilities(read_configured: bool) -> XProviderCapabilities {
    XProviderCapabilities {
        read_trends: XCapabilityState {
            status: if read_configured {
                "configured"
            } else {
                "missing_credentials"
            }
            .to_string(),
            reason: if read_configured {
                "X trend reads can use configured bearer credentials."
            } else {
                "X trend reads require X_BEARER_TOKEN or keychain account x_bearer_token."
            }
            .to_string(),
        },
        publish_posts: XCapabilityState {
            status: "social_server".to_string(),
            reason: "Posting uses the existing social publishing Twitter/X provider with OAuth scopes users.read, tweet.write, media.write, and offline.access."
                .to_string(),
        },
    }
}

fn signal_from_response(query: String, response: XRecentSearchResponse) -> Option<XTrendSignal> {
    let mut evidence: Vec<XTrendEvidence> = response
        .data
        .into_iter()
        .map(|tweet| XTrendEvidence {
            post_id: tweet.id,
            text: truncate(&tweet.text, 180),
            engagement_score: tweet.public_metrics.engagement_score(),
        })
        .collect();
    evidence.sort_by(|a, b| b.engagement_score.cmp(&a.engagement_score));
    evidence.truncate(3);
    if evidence.is_empty() {
        return None;
    }
    let engagement = evidence
        .iter()
        .map(|entry| entry.engagement_score)
        .max()
        .unwrap_or(0);
    Some(XTrendSignal {
        source: "x".to_string(),
        label: query.clone(),
        keywords: keywords_for_query(&query),
        weight: engagement_weight(engagement),
        reason: format!("recent X search for '{query}' returned posts with engagement evidence"),
        evidence,
    })
}

impl XPublicMetrics {
    fn engagement_score(&self) -> u64 {
        self.like_count + self.retweet_count * 2 + self.quote_count * 2 + self.reply_count
    }
}

fn keywords_for_query(query: &str) -> Vec<String> {
    let mut keywords = vec![query.to_string()];
    keywords.extend(
        query
            .split(|ch: char| !ch.is_alphanumeric())
            .map(str::trim)
            .filter(|word| word.len() >= 3)
            .map(str::to_string),
    );
    keywords.sort();
    keywords.dedup();
    keywords
}

fn engagement_weight(score: u64) -> f64 {
    match score {
        0..=4 => 0.3,
        5..=24 => 0.5,
        25..=99 => 0.7,
        _ => 0.9,
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_credentials_reports_read_setup_and_publish_handoff() {
        let context = missing_credentials_context(vec!["AI agents".to_string()]);

        assert_eq!(
            context.capabilities.read_trends.status,
            "missing_credentials"
        );
        assert_eq!(context.capabilities.publish_posts.status, "social_server");
        assert_eq!(
            context.usage.pass_to,
            "plan_short_form_review.trend_context"
        );
        assert!(context.usage.note.contains("X_BEARER_TOKEN"));
    }

    #[test]
    fn recent_posts_become_weighted_trend_signals() {
        let context = build_context_from_searches(vec![(
            "AI coding agents".to_string(),
            XRecentSearchResponse {
                data: vec![
                    XTweet {
                        id: "1".to_string(),
                        text: "AI coding agents are changing prototype workflows".to_string(),
                        public_metrics: XPublicMetrics {
                            like_count: 80,
                            retweet_count: 10,
                            reply_count: 4,
                            quote_count: 3,
                        },
                    },
                    XTweet {
                        id: "2".to_string(),
                        text: "quiet lower engagement post".to_string(),
                        public_metrics: XPublicMetrics {
                            like_count: 2,
                            retweet_count: 0,
                            reply_count: 0,
                            quote_count: 0,
                        },
                    },
                ],
            },
        )]);

        let signal = context.signals.first().expect("expected signal");
        assert_eq!(signal.source, "x");
        assert_eq!(signal.label, "AI coding agents");
        assert!(signal.keywords.contains(&"coding".to_string()));
        assert_eq!(signal.weight, 0.9);
        assert_eq!(signal.evidence[0].post_id, "1");
    }
}
