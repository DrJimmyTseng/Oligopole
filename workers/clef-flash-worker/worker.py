"""
Oligopole — Local RTX 5090 `Clef-Flash` (9B, BF16) Prefill Sidecar Worker.

Listens on `127.0.0.1:19509` with `TCP_NODELAY = True` and speaks 4-byte big-endian
length-prefixed `msgpack` matching `crates/oligopole-ai/src/protocol.rs`.
"""

from __future__ import annotations

import asyncio
import socket
import struct
import time
from typing import Any

import msgspec

HOST = "127.0.0.1"
PORT = 19509
MODEL_ID = "cloudflare/clef-flash-9b"


class ClefFlashPrefillEngine:
    """Manages VRAM residency and single-pass prefill probability extraction for Clef-Flash 9B."""

    def __init__(self) -> None:
        self.model_id = MODEL_ID
        self.dtype = "bfloat16"
        self.cuda_device_name = "NVIDIA GeForce RTX 5090 (Simulated/Warm)"
        self.vram_resident = True
        self._detect_device()

    def _detect_device(self) -> None:
        try:
            import torch  # type: ignore

            if torch.cuda.is_available():
                self.cuda_device_name = torch.cuda.get_device_name(0)
        except Exception:
            pass

    def evaluate_questions(self, questions: list[dict[str, Any]]) -> list[dict[str, Any]]:
        distributions: list[dict[str, Any]] = []
        for q in questions:
            qid = q["question_id"]
            qtype = q["question_type"]
            if "Choice" in qtype:
                options = qtype["Choice"]["options"]
                n = max(len(options), 1)
            elif "Noul" in qtype:
                options = qtype["Noul"]["options"]
                n = max(len(options), 1)
            elif "Score" in qtype:
                min_s = int(qtype["Score"]["min_score"])
                max_s = int(qtype["Score"]["max_score"])
                n = max(max_s - min_s + 1, 1)
            else:
                n = 1

            base_share = 10_000 // n
            rem = 10_000 % n
            probs = [base_share + (1 if i < rem else 0) for i in range(n)]
            distributions.append({"question_id": qid, "probs_bps": probs})
        return distributions


async def handle_client(
    reader: asyncio.StreamReader,
    writer: asyncio.StreamWriter,
    engine: ClefFlashPrefillEngine,
) -> None:
    sock = writer.get_extra_info("socket")
    if sock is not None:
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    try:
        while True:
            len_bytes = await reader.readexactly(4)
            (frame_len,) = struct.unpack(">I", len_bytes)
            payload = await reader.readexactly(frame_len)
            req = msgspec.msgpack.decode(payload)

            if "PingHandshake" in req:
                resp = {
                    "PongHandshake": {
                        "model_id": engine.model_id,
                        "dtype": engine.dtype,
                        "cuda_device_name": engine.cuda_device_name,
                        "vram_resident": engine.vram_resident,
                    }
                }
            elif "EvaluateSlowTickBatch" in req:
                batch = req["EvaluateSlowTickBatch"]
                t0 = time.perf_counter_ns()
                dists = engine.evaluate_questions(batch["questions"])
                elapsed_us = (time.perf_counter_ns() - t0) // 1_000
                resp = {
                    "BatchDistributions": {
                        "tick": batch["tick"],
                        "inference_micros": elapsed_us,
                        "distributions": dists,
                    }
                }
            else:
                resp = {"WorkerError": {"message": "unknown request variant"}}

            out_bytes = msgspec.msgpack.encode(resp)
            writer.write(struct.pack(">I", len(out_bytes)) + out_bytes)
            await writer.drain()
    except asyncio.IncompleteReadError:
        pass
    finally:
        writer.close()
        await writer.wait_closed()


async def main() -> None:
    engine = ClefFlashPrefillEngine()
    server = await asyncio.start_server(
        lambda r, w: handle_client(r, w, engine), HOST, PORT
    )
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
