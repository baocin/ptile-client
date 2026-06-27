# Model Inference Servers

Two GPU-backed inference servers that expose Parakeet TDT (ASR) and EasyOCR
over HTTP. Any container or CLI client can use them without loading ML models.

## Architecture

```
phone --> syncthing --> pipeline-daemon (orchestrator, light)
                              |  POST /transcribe    |
                              +--> model-server-asr  +--> KB markdown
                              |  POST /ocr           |
                              +--> model-server-ocr  +
```

## Endpoints

### model-server-asr (port 9360, GPU)

- `POST /transcribe` -- multipart `file` (audio) or form `path` (shared volume)
- `GET /health` -- model status + VRAM usage
- `GET /ready` -- true/false once model is loaded

Expects WAV (16kHz mono). Accepts any format (converts via ffmpeg).
Returns: `{text, words, processing_time_ms, audio_duration_ms, rtf, token_count}`

### model-server-ocr (port 9361, GPU)

- `POST /ocr` -- multipart `file` (image) or form `path` (shared volume)
- `GET /health`, `GET /ready`

Returns: `{text, word_count, regions, region_count}`

## CLI Usage

```bash
# Transcribe audio
asr-transcribe recording.mp3
asr-transcribe recording.wav --json

# OCR an image
ocr-transcribe screenshot.jpg
ocr-transcribe screenshot.jpg --json

# Override server URL
ASR_SERVER_URL=http://model-server-asr:9360 asr-transcribe audio.wav
```

## Docker

The servers are defined in `docker-compose.yml` as `model-server-asr` and
`model-server-ocr`. Both require NVIDIA GPU access (nvidia-container-toolkit).

Start all:

```bash
cd ~/kino/docker
docker compose up -d model-server-asr model-server-ocr
# Then restart the pipeline daemon:
docker compose up -d pipeline-daemon
```

Models:

- ASR: `~/.local/share/com.mydatatimeline.timeline/models/parakeet-tdt-int8/`
- OCR: EasyOCR downloads its own model on first start (~1.5GB)

## Dependencies

- ASR: onnxruntime-gpu, FastAPI, uvicorn, soundfile, scipy, ffmpeg
- OCR: easyocr, FastAPI, uvicorn, opencv-python-headless
- Client: curl or Python with urllib
