# Working llama-server config (as of 2026-06-25)
# Build: upstream llama.cpp, rebuilt with -DGGML_CUDA_GRAPHS=OFF
# Binary: /home/aoi/llama.cpp/build/bin/llama-server
# Model: HauhauCS Qwen3.6-35B-A3B Q4_K_M
# VLM: yes (mmproj enabled)
# GPU: RTX 3090 (24GB), pinned to CUDA0
# VRAM: ~22.9 GB
# Port: 8080
# Context: 131072
# Prompt speed: ~3200-3800 tok/s (25k tokens in ~7-9s)
# Gen speed: ~140-150 tok/s

LD_LIBRARY_PATH=/home/aoi/llama.cpp/build/bin /home/aoi/llama.cpp/build/bin/llama-server \
  --model /home/aoi/kino/active_models/Qwen3.6-35B-A3B-Uncensored-HauhauCS-Aggressive-Q4_K_M.gguf \
  --mmproj /home/aoi/kino/active_models/mmproj-Qwen3.6-35B-A3B-Uncensored-HauhauCS-Aggressive-f16.gguf \
  --host 0.0.0.0 --port 8080 \
  --ctx-size 131072 \
  --jinja \
  --parallel 1 \
  --no-warmup \
  --cache-type-k q8_0 \
  --cache-type-v q8_0 \
  --flash-attn on \
  --reasoning off \
  --device CUDA0 \
  --no-cache-prompt \
  --no-cont-batching \
  -b 4096
