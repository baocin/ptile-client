# pipeline-daemon v2 — thin orchestrator calling ASR/OCR inference servers
# No ML models loaded locally. Lightweight.
FROM python:3.13-slim

WORKDIR /app

# System deps: ffmpeg (for WAV conversion), imagemagick (for screenshot detection)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ffmpeg \
    imagemagick \
    && rm -rf /var/lib/apt/lists/*

# Copy pipeline scripts
COPY requirements.txt .
RUN pip3 install --no-cache-dir -r requirements.txt
COPY pipeline-daemon-v2 /usr/local/bin/pipeline-daemon
COPY model_client.py /usr/local/bin/model_client.py
RUN chmod +x /usr/local/bin/pipeline-daemon

RUN mkdir -p /data/tmp

EXPOSE 9355

# Volumes:
#   /home/aoi/data/syncthing/oneplus15/Screenshots
#   /home/aoi/data/syncthing/oneplus15/Recordings
#   /home/aoi/kino/data
#   /home/aoi/.local/share  (pipeline_state.db)
#   /data/tmp  (shared with model-server-asr)

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=5 \
  CMD python3 -c "import urllib.request; urllib.request.urlopen('http://localhost:9355/health')"

CMD ["python3", "/usr/local/bin/pipeline-daemon"]
