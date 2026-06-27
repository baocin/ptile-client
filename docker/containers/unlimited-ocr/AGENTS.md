# unlimited-ocr

## Purpose

Baidu Unlimited-OCR 3B inference server container. Accepts images and PDFs, returns parsed text via HTTP API.

## Ownership

- Kino infrastructure. Container managed via docker-compose under `~/kino/docker/`.
- Source: https://huggingface.co/baidu/Unlimited-OCR

## Local Contracts

- **Port**: 9362 (host) -> 8000 (container)
- **GPU**: RTX 3090 (device 1, UUID `GPU-3f87dcfd-...`). Pinned via `device_ids: ["1"]` in docker-compose. To run CPU-only, remove the GPU reservation block and set `UNLIMITED_OCR_DEVICE=cpu`.
- **Model weights**: Pre-downloaded to `/app/model/` at image build time via `snapshot_download()`. Flat directory, no HF cache symlinks. Adds ~6.7GB to image.
- **CPU compatibility**: The model's `infer()` method hardcodes `.cuda()` calls. Monkey-patches `torch.Tensor.cuda` and `torch.nn.Module.cuda` during CPU-mode startup to make `.cuda()` a no-op.
- **Stdout capture**: `model.infer()` writes results via TPSTextStreamer to stdout, doesn't return text. The server uses `redirect_stdout` to capture and return the output, filtering TPS lines.
- **Env vars**:
  - `UNLIMITED_OCR_MODEL` — model path (default: `/app/model`)
  - `UNLIMITED_OCR_MAX_LENGTH` — max generation tokens (default: 32768)
  - `UNLIMITED_OCR_DEVICE` — `cpu` (default) or `cuda`
  - `PORT` — uvicorn port (default: 8000)
- **Network**: `kino-infra` Docker bridge network.
- **Inference modes**:
  - `gundam` — image_size=640, crop_mode=True, ngram_window=128. Faster, for single images.
  - `base` — image_size=1024, crop_mode=False, ngram_window=1024. More accurate, for multi-page/PDF.
- **File structure**:
  - `Dockerfile` — image build from `pytorch/pytorch:2.10.0-cuda12.8-cudnn9-runtime`
  - `server.py` — FastAPI server with /ocr, /v1/chat/completions, /health
  - `docker-compose.yml` — service definition for this container only
  - `README.md` — usage docs

## Work Guidance

- Model weights are large (~6.7GB). Builds require `--network host` for DNS or the base pip install step fails. Rebuilds are fast when only server.py changes (cached layers).
- To switch to GPU: change `UNLIMITED_OCR_DEVICE=cuda` in env, add GPU reservation block to docker-compose.yml, and remove the monkey-patching in server.py's CPU branch.
- The `HF_HUB_OFFLINE=1` env var is set in the Dockerfile so offline loading works at runtime.
- Inference on CPU is significantly slower than GPU (~seconds per image depending on content complexity). OCR throughput is not suitable for real-time or batch processing on CPU.

## Verification

```bash
# Health check
curl -s http://localhost:9362/health

# Single image OCR
curl -s -X POST http://localhost:9362/ocr \
  -F "file=@test.png" \
  -F "prompt=document parsing." \
  -F "image_mode=gundam"

# Build
cd ~/kino/docker/containers/unlimited-ocr
docker compose build
```

## Child DOX Index

_(no children — this is a leaf container)_
