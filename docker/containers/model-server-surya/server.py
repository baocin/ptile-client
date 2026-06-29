#!/usr/bin/env python3
"""FastAPI OCR inference server using Surya OCR v2 - direct PyTorch inference."""

import sys
import time
import traceback
from pathlib import Path
from contextlib import asynccontextmanager

from fastapi import FastAPI, UploadFile, Form, HTTPException
from PIL import Image
import io

app = FastAPI(title="Surya OCR Server")

ocr_engine = {"detector": None, "recognizer": None}


@asynccontextmanager
async def lifespan(app):
    sys.stderr.write("Loading Surya detection model...\n")
    t0 = time.time()
    from surya.detection import DetectionPredictor

    ocr_engine["detector"] = DetectionPredictor(
        checkpoint="/root/.surya_models/text_detection/2025_05_07", device="cuda"
    )
    sys.stderr.write(f"Detection model loaded in {time.time() - t0:.1f}s\n")

    sys.stderr.write("Loading Surya recognition model with llama.cpp backend...\n")
    t0 = time.time()
    from surya.inference import SuryaInferenceManager
    from surya.recognition import RecognitionPredictor

    # Use llama.cpp backend instead of default vLLM (which spawns Docker)
    manager = SuryaInferenceManager(method="llamacpp", lazy=False)
    ocr_engine["recognizer"] = RecognitionPredictor(manager)
    sys.stderr.write(f"Recognition model loaded in {time.time() - t0:.1f}s\n")

    sys.stderr.write("Surya models ready.\n")
    yield
    ocr_engine["detector"] = None
    ocr_engine["recognizer"] = None


app = FastAPI(lifespan=lifespan)


@app.get("/health")
async def health():
    return {
        "status": "ok",
        "engine": "surya-ocr-v2",
        "detector_loaded": ocr_engine["detector"] is not None,
        "recognizer_loaded": ocr_engine["recognizer"] is not None,
    }


@app.post("/ocr")
async def ocr(file: UploadFile | None = None, path: str | None = Form(None)):
    if ocr_engine["recognizer"] is None:
        raise HTTPException(503, "Models not loaded")
    if file and path:
        raise HTTPException(400, "Provide either 'file' or 'path', not both")
    if not file and not path:
        raise HTTPException(
            400, "Provide either 'file' (multipart) or 'path' (shared volume)"
        )

    try:
        if path:
            p = Path(path)
            if not p.exists():
                raise HTTPException(404, f"File not found: {path}")
            image = Image.open(p).convert("RGB")
        else:
            data = await file.read()
            image = Image.open(io.BytesIO(data)).convert("RGB")
    except Exception as e:
        raise HTTPException(400, f"Failed to load image: {e}")

    t0 = time.time()
    try:
        sys.stderr.write("Detecting... ")
        detections = ocr_engine["detector"]([image])
        sys.stderr.write(f"done in {time.time() - t0:.1f}s. Recognizing... ")
        predictions = ocr_engine["recognizer"]([image], full_page=True)
        elapsed = time.time() - t0
        sys.stderr.write(f"done in {elapsed:.1f}s\n")

        if (
            not predictions
            or not hasattr(predictions[0], "blocks")
            or not predictions[0].blocks
        ):
            return {"text": "", "regions": [], "elapsed_s": round(elapsed, 3)}

        blocks = predictions[0].blocks
        text_parts = []
        regions = []
        for b in blocks:
            text_content = getattr(b, "html", "") or ""
            if text_content:
                text_parts.append(text_content)
            regions.append(
                {
                    "bbox": getattr(b, "bbox", []),
                    "confidence": getattr(b, "confidence", 0),
                    "label": getattr(b, "label", "text"),
                    "text": text_content,
                }
            )

        full_text = "\n".join(text_parts)
        return {
            "text": full_text,
            "regions": regions,
            "elapsed_s": round(elapsed, 3),
            "engine": "surya-ocr-v2",
        }
    except HTTPException:
        raise
    except Exception as e:
        tb = traceback.format_exc()
        sys.stderr.write(f"Surya OCR error: {e}\n{tb}\n")
        raise HTTPException(500, f"Surya OCR failed: {e}")


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(app, host="0.0.0.0", port=9362, log_level="info")
