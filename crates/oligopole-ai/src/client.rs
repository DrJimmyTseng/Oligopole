use crate::error::AiError;
use crate::protocol::{
    ClefQuestionDistribution, ClefTypedQuestion, ClefWorkerRequest, ClefWorkerResponse,
    HandshakeVerification,
};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Synchronous `TCP_NODELAY` loopback client communicating with `workers/clef-flash-worker`
/// using 4-byte big-endian length-prefixed `msgpack` frames.
#[derive(Debug)]
pub struct ClefFlashTcpClient {
    stream: TcpStream,
}

impl ClefFlashTcpClient {
    /// Connects to `addr` (e.g. `127.0.0.1:19509`), sets `TCP_NODELAY = true`, and performs `PingHandshake`.
    pub fn connect_and_handshake(
        addr: &str,
    ) -> Result<(Self, HandshakeVerification), AiError> {
        let stream =
            TcpStream::connect(addr).map_err(|e| AiError::WorkerIo(e.to_string()))?;
        stream
            .set_nodelay(true)
            .map_err(|e| AiError::WorkerIo(e.to_string()))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| AiError::WorkerIo(e.to_string()))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| AiError::WorkerIo(e.to_string()))?;

        let mut client = Self { stream };
        let verification = client.ping_handshake()?;
        Ok((client, verification))
    }

    pub fn ping_handshake(&mut self) -> Result<HandshakeVerification, AiError> {
        let req = ClefWorkerRequest::PingHandshake {
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
        };
        match self.send_rpc(&req)? {
            ClefWorkerResponse::PongHandshake {
                model_id,
                dtype,
                cuda_device_name,
                vram_resident,
            } => HandshakeVerification::verify(
                model_id,
                dtype,
                cuda_device_name,
                vram_resident,
            ),
            ClefWorkerResponse::WorkerError { message } => Err(AiError::ProtocolError(message)),
            ClefWorkerResponse::BatchDistributions { .. } => Err(AiError::ProtocolError(
                "unexpected BatchDistributions response to PingHandshake".to_owned(),
            )),
        }
    }

    pub fn evaluate_batch(
        &mut self,
        tick: u64,
        questions: Vec<ClefTypedQuestion>,
    ) -> Result<Vec<ClefQuestionDistribution>, AiError> {
        let req = ClefWorkerRequest::EvaluateSlowTickBatch { tick, questions };
        match self.send_rpc(&req)? {
            ClefWorkerResponse::BatchDistributions { distributions, .. } => Ok(distributions),
            ClefWorkerResponse::WorkerError { message } => Err(AiError::ProtocolError(message)),
            ClefWorkerResponse::PongHandshake { .. } => Err(AiError::ProtocolError(
                "unexpected PongHandshake response to EvaluateSlowTickBatch".to_owned(),
            )),
        }
    }

    fn send_rpc(&mut self, request: &ClefWorkerRequest) -> Result<ClefWorkerResponse, AiError> {
        let payload =
            rmp_serde::to_vec_named(request).map_err(|e| AiError::MsgpackEncode(e.to_string()))?;
        let len_u32 =
            u32::try_from(payload.len()).map_err(|_| AiError::MsgpackEncode("frame > 4GB".to_owned()))?;

        self.stream
            .write_all(&len_u32.to_be_bytes())
            .map_err(|e| AiError::WorkerIo(e.to_string()))?;
        self.stream
            .write_all(&payload)
            .map_err(|e| AiError::WorkerIo(e.to_string()))?;
        self.stream
            .flush()
            .map_err(|e| AiError::WorkerIo(e.to_string()))?;

        let mut len_buf = [0u8; 4];
        self.stream
            .read_exact(&mut len_buf)
            .map_err(|e| AiError::WorkerIo(e.to_string()))?;
        let resp_len = usize::try_from(u32::from_be_bytes(len_buf))
            .map_err(|_| AiError::MsgpackDecode("invalid frame length".to_owned()))?;

        let mut resp_buf = vec![0u8; resp_len];
        self.stream
            .read_exact(&mut resp_buf)
            .map_err(|e| AiError::WorkerIo(e.to_string()))?;

        rmp_serde::from_slice(&resp_buf).map_err(|e| AiError::MsgpackDecode(e.to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::protocol::ClefQuestionType;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn test_clef_flash_tcp_msgpack_handshake_and_batch_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let server_handle = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            conn.set_nodelay(true).unwrap();

            for _ in 0..2 {
                let mut len_buf = [0u8; 4];
                conn.read_exact(&mut len_buf).unwrap();
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut buf = vec![0u8; len];
                conn.read_exact(&mut buf).unwrap();
                let req: ClefWorkerRequest = rmp_serde::from_slice(&buf).unwrap();

                let resp = match req {
                    ClefWorkerRequest::PingHandshake { .. } => ClefWorkerResponse::PongHandshake {
                        model_id: "cloudflare/clef-flash-9b".to_owned(),
                        dtype: "bfloat16".to_owned(),
                        cuda_device_name: "NVIDIA GeForce RTX 4090 (CI Fallback)".to_owned(),
                        vram_resident: true,
                    },
                    ClefWorkerRequest::EvaluateSlowTickBatch { tick, questions } => {
                        let distributions = questions
                            .into_iter()
                            .map(|q| ClefQuestionDistribution {
                                question_id: q.question_id,
                                probs_bps: vec![2_500, 3_500, 2_500, 1_500],
                            })
                            .collect();
                        ClefWorkerResponse::BatchDistributions {
                            tick,
                            inference_micros: 32_500,
                            distributions,
                        }
                    }
                };

                let out = rmp_serde::to_vec_named(&resp).unwrap();
                let out_len = u32::try_from(out.len()).unwrap();
                conn.write_all(&out_len.to_be_bytes()).unwrap();
                conn.write_all(&out).unwrap();
                conn.flush().unwrap();
            }
        });

        let (mut client, handshake) = ClefFlashTcpClient::connect_and_handshake(&addr).unwrap();
        // Verify advisory warning is populated when device name does not contain "5090"
        assert!(handshake.advisory_warning.is_some());

        let dists = client
            .evaluate_batch(
                30,
                vec![ClefTypedQuestion {
                    question_id: "ACME_CAPEX".to_owned(),
                    context_dossier: "CORP=ACME".to_owned(),
                    prompt: "Select joint capital action".to_owned(),
                    question_type: ClefQuestionType::Choice {
                        options: vec![
                            "A".to_owned(),
                            "B".to_owned(),
                            "C".to_owned(),
                            "D".to_owned(),
                        ],
                    },
                }],
            )
            .unwrap();

        assert_eq!(dists.len(), 1);
        assert_eq!(dists.first().unwrap().probs_bps.iter().sum::<u64>(), 10_000);
        server_handle.join().unwrap();
    }
}
