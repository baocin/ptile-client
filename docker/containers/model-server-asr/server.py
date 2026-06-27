#!/usr/bin/env python3
"""ASR inference server -- Parakeet TDT 0.6B over HTTP.

Endpoints:
  POST /transcribe  -- multipart form with 'file' (WAV) or 'path' (shared-volume path)
                       Returns transcribed text + word timestamps + VAD segments.
  POST /vad         -- multipart form with 'file' (WAV) or 'path' (shared-volume path)
                       Returns VAD speech timestamps only (no ASR decode).
                       Params: threshold (default 0.5), min_speech_ms (default 250)
  GET  /health      -- model loaded + VRAM status
  GET  /ready       -- true once model is loaded
"""

import os
import sys
import tempfile
from pathlib import Path
from contextlib import asynccontextmanager

from fastapi import FastAPI, UploadFile, Form, HTTPException
from fastapi.responses import JSONResponse

sys.path.insert(0, "/usr/local/lib/python3.13/site-packages")

# The decoder modules live on a shared volume at /usr/local/bin/
sys.path.insert(0, "/usr/local/bin/")

MODEL_DIR = Path("/data/models/parakeet-tdt-int8")
MAX_FILE_SIZE = 500 * 1024 * 1024  # 500 MB

decoder = None  # Set during lifespan
vad_model = None  # Cached VAD model


@asynccontextmanager
async def lifespan(app: FastAPI):
    global decoder
    sys.stderr.write("Loading ASR model (Parakeet TDT 0.6B)...\n")
    from asr_decoder import PythonTDTDecoder

    dec = PythonTDTDecoder(model_dir=str(MODEL_DIR))
    dec.load()
    decoder = dec
    sys.stderr.write("ASR model loaded.\n")
    yield
    decoder = None


app = FastAPI(title="ASR Inference Server", version="1.0.0", lifespan=lifespan)


# ── Shared helpers ──


def _get_vad():
    """Get or create the cached VAD model (lazy load on first VAD request)."""
    global vad_model
    if vad_model is None:
        from vad_chunker import get_vad_model

        sys.stderr.write("Loading VAD model (Silero ONNX)...\n")
        vad_model = get_vad_model()
        sys.stderr.write("VAD model loaded.\n")
    return vad_model


def _resolve_wav(file, path):
    """Resolve a WAV path from multipart file or shared volume path.
    Returns (wav_path, cleanup_flag). Caller must clean up if cleanup is True."""
    if file and path:
        raise HTTPException(400, "Provide either 'file' or 'path', not both")
    if not file and not path:
        raise HTTPException(
            400, "Provide either 'file' (multipart) or 'path' (shared volume)"
        )

    if path:
        p = Path(path)
        if not p.exists():
            raise HTTPException(404, f"File not found: {path}")
        if p.stat().st_size > MAX_FILE_SIZE:
            raise HTTPException(413, f"File too large ({p.stat().st_size} bytes)")
        return str(p), False

    data = file.file.read()
    if len(data) > MAX_FILE_SIZE:
        raise HTTPException(413, f"File too large ({len(data)} bytes)")
    suffix = Path(file.filename or "audio.wav").suffix
    fd, wav_path = tempfile.mkstemp(suffix=suffix)
    os.close(fd)
    with open(wav_path, "wb") as f:
        f.write(data)
    return wav_path, True


def _ensure_wav(wav_path: str, cleanup: bool) -> tuple[str, bool]:
    """Convert non-WAV to WAV via ffmpeg. Returns (wav_path, cleanup_flag)."""
    p = Path(wav_path)
    if p.suffix.lower() != ".wav":
        converted = Path("/data/tmp") / (p.stem + ".wav")
        converted.parent.mkdir(parents=True, exist_ok=True)
        ret = os.system(
            f'ffmpeg -y -err_detect ignore_err -i "{wav_path}" '
            f'-ac 1 -ar 16000 -f wav "{converted}" 2>/dev/null'
        )
        if ret != 0 or not converted.exists():
            raise HTTPException(422, "Audio conversion failed (ffmpeg)")
        if cleanup:
            os.unlink(wav_path)
        return str(converted), True
    return wav_path, cleanup


def _cleanup(wav_path, cleanup):
    if cleanup and wav_path and os.path.exists(wav_path):
        try:
            os.unlink(wav_path)
        except OSError:
            pass


# ── Health ──


@app.get("/health")
async def health():
    if decoder is None:
        return JSONResponse({"status": "loading"}, status_code=503)
    import subprocess

    try:
        r = subprocess.run(
            [
                "nvidia-smi",
                "--query-gpu=memory.used,memory.total",
                "--format=csv,noheader,nounits",
            ],
            capture_output=True,
            text=True,
            timeout=5,
        )
        parts = r.stdout.strip().split(", ")
        used = int(parts[0]) if len(parts) >= 1 else 0
        total = int(parts[1]) if len(parts) >= 2 else 0
        return {
            "status": "ok",
            "model": "parakeet-tdt-int8",
            "vram_used_mb": used,
            "vram_total_mb": total,
        }
    except Exception as e:
        return {"status": "ok", "model": "parakeet-tdt-int8", "note": str(e)}


@app.get("/ready")
async def ready():
    return {"ready": decoder is not None}


# ── VAD endpoint (no ASR decode) ──


@app.post("/vad")
async def vad_endpoint(
    file: UploadFile = None,
    path: str = Form(None),
    threshold: float = Form(0.5),
    min_speech_ms: int = Form(250),
    min_silence_ms: int = Form(100),
):
    """Run Silero VAD on audio and return speech segment timestamps.
    No ASR decoding is performed — fast and cheap.
    Returns list of {start_sec, end_sec} segments + audio metadata.
    """
    wav_path, cleanup = _resolve_wav(file, path)
    try:
        wav_path, cleanup = _ensure_wav(wav_path, cleanup)

        import soundfile as sf
        from vad_chunker import get_speech_timestamps

        samples, sr = sf.read(wav_path)
        if samples.ndim > 1:
            samples = samples.mean(axis=1)
        if sr != 16000:
            from scipy import signal

            samples = signal.resample(samples, int(len(samples) * 16000 / sr))
            sr = 16000

        model = _get_vad()
        segments = get_speech_timestamps(
            samples,
            model,
            threshold=threshold,
            min_speech_ms=min_speech_ms,
            min_silence_ms=min_silence_ms,
        )

        # Compute per-segment stats
        total_speech_ms = int(sum((s["end"] - s["start"]) * 1000 for s in segments))
        audio_duration_ms = int(len(samples) / sr * 1000)

        # Compute noise floor and peak power
        import numpy as np

        power = 10 * np.log10(np.mean(samples**2) + 1e-12)
        peak = 10 * np.log10(np.max(samples**2) + 1e-12)

        return {
            "segments": segments,
            "num_segments": len(segments),
            "total_speech_ms": total_speech_ms,
            "audio_duration_ms": audio_duration_ms,
            "speech_ratio": round(total_speech_ms / max(audio_duration_ms, 1), 4),
            "audio_power_db": round(power, 1),
            "audio_peak_db": round(peak, 1),
        }
    except HTTPException:
        raise
    except Exception as e:
        raise HTTPException(500, str(e))
    finally:
        _cleanup(wav_path, cleanup)


# ── Transcribe endpoint (existing, extended with VAD info) ──


@app.post("/transcribe")
async def transcribe(
    file: UploadFile = None,
    path: str = Form(None),
):
    if decoder is None:
        raise HTTPException(503, "Model not loaded")

    wav_path, cleanup = _resolve_wav(file, path)
    try:
        wav_path, cleanup = _ensure_wav(wav_path, cleanup)

        # Run VAD + transcribe via vad_chunker
        from vad_chunker import split_and_transcribe

        result = split_and_transcribe(wav_path, decoder, max_segment_sec=3600)

        if not result or not result.get("text", "").strip():
            return JSONResponse({"text": "", "words": [], "audio_duration_ms": 0})

        return result

    except HTTPException:
        raise
    except Exception as e:
        raise HTTPException(500, str(e))
    finally:
        _cleanup(wav_path, cleanup)
