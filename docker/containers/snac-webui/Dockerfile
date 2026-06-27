# snac-webui — NAS dedup web UI (Flask)
FROM python:3.13-slim

WORKDIR /app

# Install runtime deps
RUN pip install --no-cache-dir flask blake3 Pillow imagehash

# Copy only what the webui needs
COPY webui/ ./webui/
# Make scanner/nas_index.py available if it exists (for import)
COPY scanner/ ./scanner/ 2>/dev/null

EXPOSE 10901

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD python3 -c "import urllib.request; urllib.request.urlopen('http://localhost:10901/api/stats')"

CMD ["python3", "webui/app.py"]
