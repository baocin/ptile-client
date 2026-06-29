# Surya OCR inference server
# Uses existing installed surya-ocr from base image, no network at build time
FROM model-server-surya:latest

COPY server.py /app/server.py

ENV LLAMA_CPP_BINARY=/usr/local/bin/llama-server
ENV TRANSFORMERS_OFFLINE=1
ENV HF_HUB_DISABLE_SYMLINKS_WARNING=1
ENV HF_HOME=/root/.cache/huggingface

EXPOSE 9362
CMD ["uvicorn", "server:app", "--host", "0.0.0.0", "--port", "9362"]
