use anyhow::{Context, Result};
use reqwest::{Client, StatusCode, Url};
use tracing::{debug, info, warn};

use super::models::*;

const DEFAULT_BASE_URL: &str = "https://scavenger.prod.gd.midnighttge.io/";

/// Result of attempting to register an address
#[derive(Debug, Clone)]
pub enum RegistrationResult {
    /// Address was registered and a receipt was returned
    Registered(RegistrationResponse),
    /// Address was already registered previously
    AlreadyRegistered { message: String },
}

/// API client for the Scavenger Mine service
pub struct ScavengerClient {
    client: Client,
    base_url: Url,
}

impl ScavengerClient {
    /// Create a new API client
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .user_agent("night-miner/1.0")
            .build()
            .context("Failed to create HTTP client")?;

        let base = std::env::var("SCAVENGER_API_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        let base_url = Self::parse_base_url(&base)?;

        debug!("Using Scavenger API base: {}", base_url);

        Ok(Self { client, base_url })
    }

    /// Create a new API client with a custom base URL (useful for testing)
    #[allow(dead_code)]
    pub fn with_base_url(base_url: String) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("night-miner/1.0")
            .build()
            .context("Failed to create HTTP client")?;

        let base_url = Self::parse_base_url(&base_url)?;

        Ok(Self { client, base_url })
    }

    fn parse_base_url(raw: &str) -> Result<Url> {
        let mut url = Url::parse(raw)
            .or_else(|_| Url::parse(&(raw.to_string() + "/")))
            .context("Invalid Scavenger API base URL")?;

        if !url.path().ends_with('/') {
            let mut path = url.path().to_string();
            path.push('/');
            url.set_path(&path);
        }

        Ok(url)
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        self.base_url
            .join(path)
            .context(format!("Failed to build endpoint URL for {}", path))
    }

    /// GET /TandC - Obtain the Token End User Agreement
    pub async fn get_terms_and_conditions(
        &self,
        version: Option<&str>,
    ) -> Result<TermsAndConditions> {
        let url = if let Some(v) = version {
            self.endpoint(&format!("TandC/{}", v))?
        } else {
            self.endpoint("TandC")?
        };

        debug!("Fetching T&C from: {}", url);

        let response = self.client.get(url).send().await?;

        if response.status().is_success() {
            let tandc = response.json::<TermsAndConditions>().await?;
            info!("Successfully fetched T&C version {}", tandc.version);
            Ok(tandc)
        } else {
            let error = response.json::<ApiError>().await?;
            anyhow::bail!("API error: {} - {}", error.error, error.message);
        }
    }

    /// POST /register - Register a Destination address to participate
    pub async fn register(
        &self,
        address: &str,
        signature: &str,
        pubkey: &str,
    ) -> Result<RegistrationResult> {
        let encoded_address = urlencoding::encode(address);
        let encoded_signature = urlencoding::encode(signature);
        let encoded_pubkey = urlencoding::encode(pubkey);

        let url = self.endpoint(&format!(
            "register/{}/{}/{}",
            encoded_address, encoded_signature, encoded_pubkey
        ))?;

        debug!("Registering address: {}", address);
        debug!("Signature: {}", signature);
        debug!("Pubkey: {}", pubkey);
        debug!("Full URL: {}", url);

        let response = self
            .client
            .post(url)
            .json(&serde_json::json!({}))
            .send()
            .await?;

        let status = response.status();

        if status.is_success() {
            let reg_response = response.json::<RegistrationResponse>().await?;
            info!("Successfully registered address: {}", address);
            return Ok(RegistrationResult::Registered(reg_response));
        }

        let error_text = response.text().await?;

        if status == StatusCode::BAD_REQUEST {
            if let Ok(api_error) = serde_json::from_str::<ApiError>(&error_text) {
                let message_lower = api_error.message.to_lowercase();
                if message_lower.contains("already") {
                    warn!(
                        "Address {} already registered: {}",
                        address, api_error.message
                    );
                    return Ok(RegistrationResult::AlreadyRegistered {
                        message: api_error.message,
                    });
                }
            }
        }

        anyhow::bail!(
            "Registration failed (status {}): {}",
            status,
            error_text.trim()
        );
    }

    /// GET /challenge - Fetch the next available challenge
    pub async fn get_challenge(&self) -> Result<ChallengeResponse> {
        let url = self.endpoint("challenge")?;

        debug!("Fetching current challenge");

        let response = self.client.get(url).send().await?;

        if response.status().is_success() {
            let challenge = response.json::<ChallengeResponse>().await?;

            match &challenge.data {
                ChallengeData::Active { challenge, .. } => {
                    info!(
                        "Fetched challenge: {} (Day {}, Challenge {})",
                        challenge.challenge_id, challenge.day, challenge.challenge_number
                    );
                }
                ChallengeData::Before { starts_at } => {
                    info!("Mining hasn't started yet. Starts at: {}", starts_at);
                }
                ChallengeData::After => {
                    info!("Mining period has ended");
                }
            }

            Ok(challenge)
        } else {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to fetch challenge: {}", error_text);
        }
    }

    /// POST /solution - Submit a solution to a challenge
    pub async fn submit_solution(
        &self,
        address: &str,
        challenge_id: &str,
        nonce: &str,
    ) -> Result<SolutionResponse> {
        let encoded_address = urlencoding::encode(address);
        let encoded_challenge = urlencoding::encode(challenge_id);
        let encoded_nonce = urlencoding::encode(nonce);
        let url = self.endpoint(&format!(
            "solution/{}/{}/{}",
            encoded_address, encoded_challenge, encoded_nonce
        ))?;

        debug!("Submitting solution for challenge: {}", challenge_id);

        let response = self
            .client
            .post(url)
            .json(&serde_json::json!({}))
            .send()
            .await?;

        if response.status().is_success() {
            let solution = response.json::<SolutionResponse>().await?;
            info!("Solution accepted for challenge: {}", challenge_id);
            Ok(solution)
        } else {
            let error_text = response.text().await?;
            anyhow::bail!("Solution submission failed: {}", error_text);
        }
    }

    /// GET /work_to_star_rate - Get daily STAR allocation rates
    pub async fn get_work_to_star_rate(&self) -> Result<WorkToStarRate> {
        let url = self.endpoint("work_to_star_rate")?;

        debug!("Fetching work to star rate");

        let response = self.client.get(url).send().await?;

        if response.status().is_success() {
            let rates = response.json::<WorkToStarRate>().await?;
            info!("Fetched {} days of STAR rates", rates.len());
            Ok(rates)
        } else {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to fetch star rates: {}", error_text);
        }
    }

    /// GET /statistics/{address} - Get statistics for an address
    pub async fn get_statistics(&self, address: &str) -> Result<StatisticsResponse> {
        let encoded_address = urlencoding::encode(address);
        let url = self.endpoint(&format!("statistics/{}", encoded_address))?;

        debug!("Fetching statistics for address: {}", address);

        let response = self.client.get(url).send().await?;

        if response.status().is_success() {
            let stats = response.json::<StatisticsResponse>().await?;
            debug!(
                "Address has {} crypto receipts, {} STAR allocation",
                stats.local.crypto_receipts, stats.local.night_allocation
            );
            Ok(stats)
        } else {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to fetch statistics: {}", error_text);
        }
    }
}

impl Default for ScavengerClient {
    fn default() -> Self {
        Self::new().expect("Failed to create default ScavengerClient")
    }
}
