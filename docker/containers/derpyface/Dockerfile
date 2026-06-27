# derpyface — RTSP face detection daemon + API server
# Uses ONNX models (SCRFD + MobileFaceNet). API port is 8099 (from defaults.py).
FROM python:3.13-slim

WORKDIR /app

# Install system deps for opencv (Debian Trixie compatible)
RUN apt-get update && apt-get install -y --no-install-recommends \
    libgl1 libglib2.0-0 libsm6 libxext6 libgomp1 \
    && rm -rf /var/lib/apt/lists/*

# Copy project
COPY pyproject.toml .
COPY derpyface/ ./derpyface/
COPY main.py .

# Install Python deps (no -e, just pip install deps directly)
RUN pip install --no-cache-dir \
    onnxruntime opencv-python numpy pillow aiohttp>=3.14.1 onvif-zeep>=0.2.12

EXPOSE 8099

# Models mount: /app/models (read-only)
# Data mount: /data (face store, crops, scans)

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
  CMD python3 -c "import urllib.request; urllib.request.urlopen('http://localhost:8099/api/faces?limit=1')"

CMD ["python3", "-m", "derpyface.api_server"]
