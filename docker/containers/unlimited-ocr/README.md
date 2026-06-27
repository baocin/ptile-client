# unlimited-ocr

Baidu Unlimited-OCR 3B inference server. CPU-only, port 9362.

See AGENTS.md for full documentation.

## Quick start

```bash
cd ~/kino/docker/containers/unlimited-ocr
docker compose up -d

# OCR a single image
curl -X POST http://localhost:9362/ocr \
  -F "file=@document.png" \
  -F "prompt=document parsing." \
  -F "image_mode=gundam"
```

## Endpoints

| Endpoint               | Method | Description                       |
| ---------------------- | ------ | --------------------------------- |
| `/health`              | GET    | Model loaded + device info        |
| `/ocr`                 | POST   | Upload image/PDF, get parsed text |
| `/v1/chat/completions` | POST   | OpenAI-compatible, base64 image   |
