# unlimited-ocr -- baidu/Unlimited-OCR inference server (GPU)
# 3B BF16 model, ~6.7GB weights, pinned to 2070S (GPU 0).
# Base: pytorch/pytorch:2.10.0-cuda12.8-cudnn9-runtime (torch 2.10 + CUDA 12.8 pre-installed)

FROM pytorch/pytorch:2.10.0-cuda12.8-cudnn9-runtime

WORKDIR /app

# Install inference deps (transforms, pymupdf, server framework)
RUN pip3 install --break-system-packages --no-cache-dir \
    transformers==4.57.1 \
    Pillow==12.1.1 \
    matplotlib==3.10.8 \
    einops==0.8.2 \
    addict==2.4.0 \
    easydict==1.13 \
    pymupdf==1.27.2.2 \
    psutil==7.2.2 \
    accelerate==1.6.0 \
    fastapi==0.115.0 \
    uvicorn==0.30.0 \
    python-multipart==0.0.9

# Pre-download model weights into /app/model for local loading.
# Downloads to a flat directory (no HF cache symlinks) so transformers
# can load offline without issues.
RUN python3 -c "\
from huggingface_hub import snapshot_download; \
name = 'baidu/Unlimited-OCR'; \
target = '/app/model'; \
print(f'Downloading {name} to {target}...', flush=True); \
snapshot_download(repo_id=name, local_dir=target, local_dir_use_symlinks=False); \
print('Download complete.', flush=True); \
"

COPY server.py /app/server.py

ENV UNLIMITED_OCR_MAX_LENGTH=32768
ENV PORT=8000
ENV HF_HUB_OFFLINE=1
ENV HUGGINGFACE_HUB_CACHE=/root/.cache/huggingface

EXPOSE 8000

CMD ["uvicorn", "server:app", "--host", "0.0.0.0", "--port", "8000"]
