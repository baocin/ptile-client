# model-server-asr -- Parakeet TDT 0.6B inference server (GPU)
FROM docker.io/nvidia/cuda:12.8.0-runtime-ubuntu22.04

# Install cuDNN (needed by onnxruntime CUDAExecutionProvider) + pip + ffmpeg
RUN apt-get update && apt-get install -y --no-install-recommends \
    libcudnn9-cuda-12 \
    python3-pip \
    ffmpeg \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Install Python deps only (apt packages like python3/pip are already in this CUDA image)
COPY requirements.txt .
RUN pip3 install --no-cache-dir -r requirements.txt

# Shared decoder modules
RUN mkdir -p /usr/local/lib/python3.10/site-packages

COPY server.py /app/server.py

EXPOSE 9360

# Pin to GPU 0 inside the container
ENV CUDA_VISIBLE_DEVICES=0

HEALTHCHECK --interval=30s --timeout=5s --start-period=120s --retries=5 \
  CMD python3 -c "import urllib.request; urllib.request.urlopen('http://localhost:9360/health')"

CMD ["python3", "-m", "uvicorn", "server:app", "--host", "0.0.0.0", "--port", "9360"]
