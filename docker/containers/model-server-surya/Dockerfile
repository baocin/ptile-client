# Surya OCR inference server - uses llama.cpp backend for recognition
FROM python:3.12-slim

WORKDIR /app

# No apt - install surya-ocr and friends via pip
RUN pip install --no-cache-dir surya-ocr uvicorn fastapi python-multipart Pillow

# Install llama.cpp binary for Surya's llamacpp backend
# Download from GitHub releases (tiny binary, no GPU deps needed - 
# Surya uses the llama.cpp backend as an HTTP inference server)
RUN pip install --no-cache-dir llama-cpp-python || true

COPY server.py /app/server.py

EXPOSE 9362

# LLAMA_CPP_BINARY will be set via env or docker run -e
CMD ["uvicorn", "server:app", "--host", "0.0.0.0", "--port", "9362"]
