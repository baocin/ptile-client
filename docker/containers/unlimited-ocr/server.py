"""
FastAPI server wrapping baidu/Unlimited-OCR.
Provides /ocr (file upload), /v1/chat/completions (OpenAI-compatible),
and /health endpoints. Model loaded once at startup.
"""

import os
import tempfile
import base64
import re
import time
import logging

import torch
from transformers import AutoTokenizer, AutoModel
import fitz  # PyMuPDF

from fastapi import FastAPI, HTTPException, UploadFile, File, Form
from pydantic import BaseModel

logging.basicConfig(
    level=logging.INFO, format="%(asctime)s [%(levelname)s] %(message)s"
)
logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Config from env
# ---------------------------------------------------------------------------
MODEL_NAME = os.environ.get("UNLIMITED_OCR_MODEL", "/app/model")
MAX_LENGTH = int(os.environ.get("UNLIMITED_OCR_MAX_LENGTH", "32768"))

app = FastAPI(title="Unlimited-OCR Server", version="1.0.0")

model = None
tokenizer = None


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def pdf_to_images(pdf_bytes: bytes, dpi: int = 200) -> list[bytes]:
    """Convert PDF bytes to list of PNG image bytes (CPU only)."""
    doc = fitz.open(stream=pdf_bytes, filetype="pdf")
    mat = fitz.Matrix(dpi / 72, dpi / 72)
    images = []
    for i, page in enumerate(doc):
        pix = page.get_pixmap(matrix=mat)
        images.append(pix.tobytes("png"))
    doc.close()
    return images


def save_temp_image(image_bytes: bytes, suffix: str = ".png") -> str:
    fd, path = tempfile.mkstemp(suffix=suffix)
    os.write(fd, image_bytes)
    os.close(fd)
    return path


def cleanup(paths: list[str]):
    for p in paths:
        try:
            os.remove(p)
        except OSError:
            pass


# ---------------------------------------------------------------------------
# Lifespan
# ---------------------------------------------------------------------------


@app.on_event("startup")
async def load_model():
    global model, tokenizer
    DEVICE = os.environ.get("UNLIMITED_OCR_DEVICE", "cpu")

    logger.info(f"Loading model: {MODEL_NAME} on {DEVICE}")
    t0 = time.time()
    tokenizer = AutoTokenizer.from_pretrained(MODEL_NAME, trust_remote_code=True)

    if DEVICE == "cpu":
        # Load on CPU.
        model = AutoModel.from_pretrained(
            MODEL_NAME,
            trust_remote_code=True,
            use_safetensors=True,
            torch_dtype=torch.bfloat16,
            device_map=None,
        )
        model = model.eval().cpu()

        # Monkey-patch infer() — the model's method hardcodes .cuda()
        # internally. We patch torch.Tensor.cuda to be a no-op so the
        # model code runs on CPU.
        _orig_cuda = torch.Tensor.cuda

        def _noop_cuda(self, device=None, non_blocking=False, **kwargs):
            return (
                self
                if device is None or str(device) == "cpu"
                else _orig_cuda(
                    self, device=device, non_blocking=non_blocking, **kwargs
                )
            )

        torch.Tensor.cuda = _noop_cuda

        _orig_nn_cuda = torch.nn.Module.cuda

        def _noop_module_cuda(self, device=None, **kwargs):
            return (
                self
                if device is None or str(device) == "cpu"
                else _orig_nn_cuda(self, device=device, **kwargs)
            )

        torch.nn.Module.cuda = _noop_module_cuda

        _orig_infer = model.infer

        def _cpu_infer(
            tokenizer_,
            prompt="",
            image_file="",
            output_path="",
            base_size=1024,
            image_size=640,
            crop_mode=True,
            max_length=32768,
            no_repeat_ngram_size=35,
            ngram_window=128,
            save_results=True,
            **kw,
        ):
            return _orig_infer(
                tokenizer_,
                prompt=prompt,
                image_file=image_file,
                output_path=output_path,
                base_size=base_size,
                image_size=image_size,
                crop_mode=crop_mode,
                max_length=max_length,
                no_repeat_ngram_size=no_repeat_ngram_size,
                ngram_window=ngram_window,
                save_results=save_results,
                **kw,
            )

        model.infer = _cpu_infer

    else:
        logger.info(
            "GPU mode: loading with device_map='auto', spill to 2070S if needed"
        )
        model = AutoModel.from_pretrained(
            MODEL_NAME,
            trust_remote_code=True,
            use_safetensors=True,
            torch_dtype=torch.bfloat16,
            device_map="auto",
            max_memory={0: "1GiB", 1: "4GiB"},
        )
        model = model.eval()

    elapsed = time.time() - t0
    logger.info(
        f"Model loaded in {elapsed:.1f}s. Device: {next(model.parameters()).device}"
    )


# ---------------------------------------------------------------------------
# API Models
# ---------------------------------------------------------------------------


class ChatMessage(BaseModel):
    role: str
    content: str


class ChatRequest(BaseModel):
    model: str = "baidu/Unlimited-OCR"
    messages: list[ChatMessage]
    temperature: float = 0.0
    max_tokens: int = 8192
    image_mode: str = "gundam"


# ---------------------------------------------------------------------------
# Endpoints
# ---------------------------------------------------------------------------


@app.get("/health")
async def health():
    return {
        "status": "ok",
        "model": MODEL_NAME,
        "device": "cuda",
        "model_loaded": model is not None,
    }


def _run_infer(
    image_bytes: bytes,
    prompt: str = "document parsing.",
    image_mode: str = "gundam",
    max_length: int = MAX_LENGTH,
) -> str:
    """Core inference helper. Runs model.infer() on a single image, returns text."""
    if model is None:
        raise HTTPException(503, "Model not loaded yet")

    image_path = save_temp_image(image_bytes)
    output_dir = tempfile.mkdtemp()

    # Capture the model's streamer output (model.infer() writes to stdout
    # via TPSTextStreamer but doesn't return the text)
    import io
    from contextlib import redirect_stdout

    buf = io.StringIO()
    try:
        if image_mode == "gundam":
            base_size, image_size, crop_mode, ngram_window = 1024, 640, True, 128
        else:
            base_size, image_size, crop_mode, ngram_window = 1024, 1024, False, 128

        with redirect_stdout(buf):
            model.infer(
                tokenizer,
                prompt=f"<image>{prompt}",
                image_file=image_path,
                output_path=output_dir,
                base_size=base_size,
                image_size=image_size,
                crop_mode=crop_mode,
                max_length=max_length,
                no_repeat_ngram_size=35,
                ngram_window=ngram_window,
                save_results=False,
            )

        output = buf.getvalue().strip()

        # The model prints output via streamer (detection markup + text).
        # Keep all lines except TPS performance stats.
        lines = [
            l
            for l in output.split("\n")
            if l.strip() and "tps:" not in l.lower() and "tokens/s" not in l.lower()
        ]
        result = "\n".join(lines) if lines else output

        return result
    finally:
        cleanup([image_path])


@app.post("/ocr")
async def ocr(
    file: UploadFile = File(...),
    prompt: str = Form("document parsing."),
    image_mode: str = Form("gundam"),
    max_length: int = Form(MAX_LENGTH),
):
    """OCR a single image or PDF. Returns parsed text."""
    if model is None:
        raise HTTPException(503, "Model not loaded yet")

    contents = await file.read()
    filename = file.filename or ""

    is_pdf = filename.lower().endswith(".pdf") or contents[:4] == b"%PDF"

    if is_pdf:
        logger.info(f"Processing PDF: {filename} ({len(contents)} bytes)")
        page_images = pdf_to_images(contents)
        results = []
        for i, img_bytes in enumerate(page_images):
            logger.info(f"  Page {i + 1}/{len(page_images)}")
            text = _run_infer(
                img_bytes, prompt=prompt, image_mode="base", max_length=max_length
            )
            results.append({"page": i + 1, "text": text})
        return {"pages": results, "total_pages": len(page_images)}
    else:
        text = _run_infer(
            contents, prompt=prompt, image_mode=image_mode, max_length=max_length
        )
        return {"text": text}


@app.post("/v1/chat/completions")
async def chat_completions(req: ChatRequest):
    """OpenAI-compatible endpoint. Expects a base64-encoded image in the last user message."""
    if model is None:
        raise HTTPException(503, "Model not loaded yet")

    # Extract last user message content
    user_content = None
    for msg in reversed(req.messages):
        if msg.role == "user":
            user_content = msg.content
            break

    if not user_content:
        raise HTTPException(400, "No user message found")

    # Extract base64 image from content
    content = user_content if isinstance(user_content, str) else str(user_content)
    m = re.search(r"data:image/(png|jpg|jpeg);base64,([A-Za-z0-9+/=]+)", content)
    if not m:
        raise HTTPException(400, "No valid base64 image found in message content")

    img_data = base64.b64decode(m.group(2))

    # Extract prompt text (remove the image data)
    prompt_text = re.sub(r"<data:image/[^>]+>", "", content).strip()
    if not prompt_text or prompt_text == content:
        prompt_text = "document parsing."

    text = _run_infer(
        img_data,
        prompt=prompt_text,
        image_mode=req.image_mode,
        max_length=req.max_tokens,
    )

    return {
        "id": "unlimited-ocr",
        "object": "chat.completion",
        "created": int(time.time()),
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop",
            }
        ],
        "model": req.model,
        "usage": {
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0,
        },
    }


if __name__ == "__main__":
    import uvicorn

    port = int(os.environ.get("PORT", "8000"))
    uvicorn.run(app, host="0.0.0.0", port=port)
