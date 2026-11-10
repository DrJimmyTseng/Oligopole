# Sidecar Component: `clef-flash-worker` — Local RTX 5090 `Clef-Flash` 9B Prefill Engine

## 1. Purpose & Wire Protocol
`clef-flash-worker` is a lightweight Python `asyncio` TCP server (`127.0.0.1:19509`, `TCP_NODELAY = true`) that keeps Cloudflare `Clef-Flash` (9B, BF16) resident in VRAM on an NVIDIA RTX 5090 (32 GB VRAM) and evaluates batches of up to 64 independent typed questions (`noul`, `choice`, `score`) in a single non-autoregressive prefill forward pass (~30–40ms).

## 2. Framing & Handshake
- **Framing:** `[u32 big-endian payload_len][msgpack payload]` decoded via `msgspec.msgpack`.
- **`PingHandshake`:** Returns model identifier, `bfloat16` precision status, and CUDA device string (checked advisably by `oligopole-ai`).
- **`EvaluateSlowTickBatch`:** Returns calibrated probability vectors per question ID.
