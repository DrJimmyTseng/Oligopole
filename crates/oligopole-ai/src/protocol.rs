use serde::{Deserialize, Serialize};

/// Default loopback TCP port for the local RTX 5090 `clef-flash-worker` sidecar.
pub const DEFAULT_CLEF_WORKER_ADDR: &str = "127.0.0.1:19509";

/// Typed non-autoregressive prefill question format supported by Cloudflare `Clef-Flash` 9B.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClefQuestionType {
    /// Multi-way categorical choice returning calibrated probabilities (`sum == 10_000 bps`).
    Choice { options: Vec<String> },
    /// Binary or abstaining (`noul`) decision over discrete stances.
    Noul { options: Vec<String> },
    /// Calibrated integer score distribution (`1..=10`).
    Score { min_score: u8, max_score: u8 },
}

/// A single independent question in a parallel `Clef-Flash` Slow-Tick batch (up to 64 per pass).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClefTypedQuestion {
    pub question_id: String,
    pub context_dossier: String,
    pub prompt: String,
    pub question_type: ClefQuestionType,
}

/// Wire RPC request envelope encoded via `rmp-serde` (`msgpack`) with a 4-byte big-endian length prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClefWorkerRequest {
    PingHandshake { client_version: String },
    EvaluateSlowTickBatch {
        tick: u64,
        questions: Vec<ClefTypedQuestion>,
    },
}

/// Calibrated probability distribution returned for a single `question_id` in basis points (`u64`, summing to `10_000`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClefQuestionDistribution {
    pub question_id: String,
    pub probs_bps: Vec<u64>,
}

/// Wire RPC response envelope returned by `clef-flash-worker`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClefWorkerResponse {
    PongHandshake {
        model_id: String,
        dtype: String,
        cuda_device_name: String,
        vram_resident: bool,
    },
    BatchDistributions {
        tick: u64,
        inference_micros: u64,
        distributions: Vec<ClefQuestionDistribution>,
    },
    WorkerError {
        message: String,
    },
}

/// Result of validating the `PongHandshake` from `clef-flash-worker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeVerification {
    pub model_id: String,
    pub dtype: String,
    pub cuda_device_name: String,
    pub advisory_warning: Option<String>,
}

impl HandshakeVerification {
    /// Validates `PongHandshake` where the `"RTX 5090"` device name check is **advisory**
    /// (records an advisory warning rather than hard-failing when running on fallback hardware or CI).
    pub fn verify(
        model_id: String,
        dtype: String,
        cuda_device_name: String,
        vram_resident: bool,
    ) -> Result<Self, crate::error::AiError> {
        if !vram_resident {
            return Err(crate::error::AiError::ProtocolError(
                "Clef-Flash 9B model is not resident in VRAM".to_owned(),
            ));
        }

        let mut warnings = Vec::new();
        if !cuda_device_name.contains("5090") {
            warnings.push(format!(
                "advisory: expected NVIDIA RTX 5090 device, worker reported '{cuda_device_name}'"
            ));
        }
        if dtype != "bfloat16" {
            warnings.push(format!(
                "advisory: expected bfloat16 precision, worker reported '{dtype}'"
            ));
        }

        let advisory_warning = if warnings.is_empty() {
            None
        } else {
            Some(warnings.join("; "))
        };

        Ok(Self {
            model_id,
            dtype,
            cuda_device_name,
            advisory_warning,
        })
    }
}
