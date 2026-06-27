# model-server-ocr -- PP-OCRv5/v6 ONNX inference server (GPU)
FROM nvidia/cuda:12.8.0-runtime-ubuntu22.04

WORKDIR /app

# Install pip + libGL + opencv deps + cuDNN
RUN apt-get update && apt-get install -y --no-install-recommends python3-pip libgl1 libglib2.0-0 libcudnn9-cuda-12 && \
    rm -rf /var/lib/apt/lists/*

COPY requirements.txt .
RUN pip3 install --no-cache-dir -r requirements.txt

COPY server.py /app/server.py

# Host paddle cache mounts at /root/.paddlex for ONNX models
VOLUME /root/.paddlex

EXPOSE 9361

HEALTHCHECK --interval=30s --timeout=10s --start-period=15s --retries=5 \
  CMD python3 -c "import urllib.request; urllib.request.urlopen('http://localhost:9361/health')"

CMD ["python3", "-m", "uvicorn", "server:app", "--host", "0.0.0.0", "--port", "9361"]
