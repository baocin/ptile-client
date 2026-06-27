#!/usr/bin/env python3
"""FastAPI OCR inference server using PP-OCRv5/v6 ONNX models on onnxruntime-gpu.

Detection: PP-OCRv6 ONNX (works correctly)
Recognition: PP-OCRv5 English ONNX (from monkt/paddleocr-onnx on HF)
"""

import os
import sys
import time
import uuid
from pathlib import Path
from contextlib import asynccontextmanager

import numpy as np
import cv2
import onnxruntime as ort
from fastapi import FastAPI, UploadFile, Form, HTTPException, Query
from fastapi.responses import FileResponse

MODEL_DIR = Path("/root/.paddlex/onnx_models")
MAX_FILE_SIZE = 50 * 1024 * 1024

det_session = None
rec_session = None
rec_vocab = None


@asynccontextmanager
async def lifespan(app: FastAPI):
    global det_session, rec_session, rec_vocab
    sys.stderr.write("Loading PP-OCR models (GPU)...\n")

    providers = [
        ("CUDAExecutionProvider", {"device_id": 0}),
        "CPUExecutionProvider",
    ]
    opts = ort.SessionOptions()
    opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL

    det_path = MODEL_DIR / "ppocrv5" / "detection" / "det.onnx"
    rec_path = MODEL_DIR / "ppocrv5" / "recognition" / "rec.onnx"

    if not det_path.exists():
        raise RuntimeError(f"Detection model not found: {det_path}")
    if not rec_path.exists():
        raise RuntimeError(f"Recognition model not found: {rec_path}")

    det_session = ort.InferenceSession(str(det_path), opts, providers=providers)
    rec_session = ort.InferenceSession(str(rec_path), opts, providers=providers)

    # Load v5 dict
    with open(MODEL_DIR / "ppocrv5" / "recognition" / "dict.txt") as f:
        lines = f.read().strip().split("\n")
    rec_vocab = {0: ""}
    for i, c in enumerate(lines, 1):
        rec_vocab[i] = c

    sys.stderr.write(
        f"Det: {det_path.name}  Rec: {rec_path.name}  Vocab: {len(rec_vocab)} chars\n"
    )
    sys.stderr.write("Models loaded on GPU.\n")
    yield
    det_session = rec_session = None


app = FastAPI(title="OCR Server", version="1.0.0", lifespan=lifespan)


# ── helpers ─────────────────────────────────────────────────────


def _detect_strip(strip, sh, sw, y_offset):
    """Run PP-OCRv6 detection on a single image strip."""
    det_limit_side_len = 960
    det_min_side = 64
    boxes = []
    ratio = min(det_limit_side_len / max(sh, sw), 1.0)
    nh = int(round(sh * ratio / 32) * 32)
    nw = int(round(sw * ratio / 32) * 32)
    if nh < 32:
        nh = 32
    if nw < 32:
        nw = 32
    if min(nh, nw) < det_min_side:
        ratio2 = det_min_side / min(sh, sw)
        nh = int(round(sh * ratio2 / 32) * 32)
        nw = int(round(sw * ratio2 / 32) * 32)
        if nh < 32:
            nh = 32
        if nw < 32:
            nw = 32

    strip_resized = cv2.resize(strip, (nw, nh))
    img_norm = strip_resized.astype(np.float32)[:, :, ::-1] / 255.0
    mean = np.array([0.485, 0.456, 0.406], dtype=np.float32)
    std = np.array([0.229, 0.224, 0.225], dtype=np.float32)
    img_norm = (img_norm - mean) / std
    img_norm = np.transpose(img_norm, (2, 0, 1))[np.newaxis, ...]

    prob_map = det_session.run(None, {"x": img_norm})[0][0, 0]
    threshold = 0.2
    box_thresh = 0.45
    mask = (prob_map > threshold).astype(np.uint8) * 255
    contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)

    for cnt in contours:
        area = cv2.contourArea(cnt)
        if area < 50:
            continue
        rect = cv2.minAreaRect(cnt)
        box = cv2.boxPoints(rect).astype(np.float32)
        cnt_mask = np.zeros(prob_map.shape, dtype=np.uint8)
        cv2.drawContours(cnt_mask, [cnt.astype(np.int32)], -1, 1, -1)
        conf = float((prob_map * cnt_mask).sum() / max(cnt_mask.sum(), 1))
        if conf < box_thresh:
            continue
        if nw != sw:
            box[:, 0] = box[:, 0] * sw / nw
        if nh != sh:
            box[:, 1] = box[:, 1] * sh / nh
        box[:, 1] += y_offset
        boxes.append((box, conf))
    return boxes


def _recognize_crop(crop):
    """Run PP-OCRv5 recognition on a single crop."""
    rec_h, rec_w = 48, 320
    crop_h, crop_w = crop.shape[:2]
    ratio = rec_h / crop_h
    new_w = int(round(crop_w * ratio))
    new_w = min(new_w, rec_w)
    crop_resized = cv2.resize(crop, (new_w, rec_h))
    if new_w < rec_w:
        crop_padded = cv2.copyMakeBorder(
            crop_resized,
            0,
            0,
            0,
            rec_w - new_w,
            cv2.BORDER_CONSTANT,
            value=[255, 255, 255],
        )
    else:
        crop_padded = crop_resized

    crop_rgb = cv2.cvtColor(crop_padded, cv2.COLOR_BGR2RGB)
    crop_norm = crop_rgb.astype(np.float32) / 255.0
    mean = np.array([0.485, 0.456, 0.406], dtype=np.float32)
    std = np.array([0.229, 0.224, 0.225], dtype=np.float32)
    crop_norm = (crop_norm - mean) / std
    crop_norm = np.transpose(crop_norm, (2, 0, 1))[np.newaxis, ...]

    rec_out = rec_session.run(None, {"x": crop_norm})[0]
    pred_ids = rec_out[0].argmax(axis=1)
    text = ""
    prev = -1
    for p in pred_ids:
        if p != 0 and p != prev:
            text += rec_vocab.get(p, "?")
        prev = p
    conf = round(float(rec_out[0].max(axis=1).mean()), 3)
    return text.strip(), conf


# ── endpoints ───────────────────────────────────────────────────


@app.get("/health")
async def health():
    return {
        "status": "ok",
        "engine": "pp-ocrv5-onnx-gpu",
        "det_loaded": det_session is not None,
        "rec_loaded": rec_session is not None,
    }


@app.get("/ready")
async def ready():
    return {"ready": det_session is not None and rec_session is not None}


@app.post("/ocr")
async def ocr(
    file: UploadFile | None = None,
    path: str | None = Form(None),
    preview: bool = Query(False),
):
    if det_session is None or rec_session is None:
        raise HTTPException(503, "Models not loaded")
    if file and path:
        raise HTTPException(400, "Provide either 'file' or 'path', not both")
    if not file and not path:
        raise HTTPException(400, "Provide either 'file' or 'path'")

    image_path = None
    cleanup = False
    try:
        if path:
            p = Path(path)
            if not p.exists():
                raise HTTPException(404, f"File not found: {path}")
            if p.stat().st_size > MAX_FILE_SIZE:
                raise HTTPException(413, "File too large")
            image_path = str(p)
        else:
            import tempfile

            data = await file.read()
            if len(data) > MAX_FILE_SIZE:
                raise HTTPException(413, "File too large")
            suffix = Path(file.filename or "image.jpg").suffix
            fd, image_path = tempfile.mkstemp(suffix=suffix)
            os.close(fd)
            with open(image_path, "wb") as f:
                f.write(data)
            cleanup = True

        t0 = time.time()
        img = cv2.imread(image_path)
        if img is None:
            raise HTTPException(422, "Failed to read image")

        h, w = img.shape[:2]
        det_limit_side_len = 960

        # Detection
        boxes = []
        if h > w * 4:
            # Very tall — split into overlapping vertical strips
            stride = int(det_limit_side_len * h / w * 0.8)
            stride = max(stride, 1)
            for y_start in range(0, h, stride):
                y_end = min(y_start + det_limit_side_len, h)
                boxes.extend(
                    _detect_strip(img[y_start:y_end, :, :], y_end - y_start, w, y_start)
                )
                if y_end == h:
                    break
            boxes.sort(key=lambda b: (b[0][:, 1].min(), b[0][:, 0].min()))
        elif w > h * 4:
            # Very wide — split into horizontal strips
            stride = int(det_limit_side_len * w / h * 0.8)
            stride = max(stride, 1)
            for x_start in range(0, w, stride):
                x_end = min(x_start + det_limit_side_len, w)
                strip = img[:, x_start:x_end, :]
                sh, sw = strip.shape[:2]
                sboxes = _detect_strip(strip, sh, sw, 0)
                sboxes = [((box + [x_start, 0]), conf) for box, conf in sboxes]
                boxes.extend(sboxes)
                if x_end == w:
                    break
            boxes.sort(key=lambda b: (b[0][:, 1].min(), b[0][:, 0].min()))
        else:
            boxes = _detect_strip(img, h, w, 0)
            boxes.sort(key=lambda b: (b[0][:, 1].min(), b[0][:, 0].min()))

        # Recognition
        text_lines = []
        regions = []

        for box_data in boxes:
            box, det_conf = box_data
            x, y, bw, bh = cv2.boundingRect(box.astype(np.int32))
            x1 = max(0, x - 5)
            y1 = max(0, y - 5)
            x2 = min(w, x + bw + 5)
            y2 = min(h, y + bh + 5)
            crop = img[y1:y2, x1:x2]

            if crop.size == 0 or crop.shape[0] < 5 or crop.shape[1] < 5:
                continue

            # Handle inverted colors: light text on dark background
            # PP-OCR models trained on dark text on light background
            gray = cv2.cvtColor(crop, cv2.COLOR_BGR2GRAY)
            dark_frac = (gray < 50).mean()
            light_frac = (gray > 200).mean()
            if dark_frac > 0.5 and light_frac < dark_frac * 0.5:
                # Light-on-dark — invert grayscale, convert back to 3-channel
                inv = 255 - gray
                crop = cv2.cvtColor(inv, cv2.COLOR_GRAY2BGR)

            # Upscale small crops so text is readable by the recognition model
            ch, cw = crop.shape[:2]
            if ch < 40:
                scale = max(2.0, 40.0 / ch)
                new_h, new_w = int(ch * scale), int(cw * scale)
                crop = cv2.resize(crop, (new_w, new_h), interpolation=cv2.INTER_CUBIC)

            text, conf = _recognize_crop(crop)
            if text:
                text_lines.append(text)
                regions.append(
                    {
                        "text": text,
                        "bbox": box.astype(int).tolist(),
                        "conf": conf,
                    }
                )

        elapsed = time.time() - t0
        full_text = "\n".join(text_lines)

        result = {
            "text": full_text,
            "word_count": len(full_text.split()),
            "regions": regions,
            "region_count": len(regions),
            "processing_time_ms": int(elapsed * 1000),
        }

        if preview:
            preview_img = img.copy()
            for region in regions:
                box = np.array(region["bbox"], dtype=np.int32)
                txt = region["text"]
                cv2.polylines(preview_img, [box], True, (0, 255, 0), 2)
                x, y = box.min(axis=0)
                cv2.putText(
                    preview_img,
                    txt,
                    (x + 2, max(y - 2, 12)),
                    cv2.FONT_HERSHEY_SIMPLEX,
                    0.4,
                    (0, 255, 0),
                    1,
                )

            preview_name = f"preview_{uuid.uuid4().hex[:8]}.jpg"
            preview_path = f"/data/tmp/{preview_name}"
            cv2.imwrite(preview_path, preview_img, [cv2.IMWRITE_JPEG_QUALITY, 90])
            result["preview_url"] = f"http://localhost:9361/preview/{preview_name}"

        return result
    except HTTPException:
        raise
    except Exception as e:
        raise HTTPException(500, str(e))
    finally:
        if cleanup and image_path and os.path.exists(image_path):
            try:
                os.unlink(image_path)
            except OSError:
                pass


@app.get("/preview/{name}")
async def serve_preview(name: str):
    p = f"/data/tmp/{name}"
    if not os.path.exists(p):
        raise HTTPException(404, "Preview not found")
    return FileResponse(p, media_type="image/jpeg")


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(app, host="0.0.0.0", port=9361)
