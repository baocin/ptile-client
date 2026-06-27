# ephemera — phone dump viewer with media proxy
# Uses http.server stdlib + kb_index for FTS5 KB search
FROM python:3.13-slim

WORKDIR /app

# Copy server code (single-file app)
COPY server.py .
COPY build.py .
COPY index.html .

# Install kb_index as a proper module so `import kb_index` works
# The file will be mounted at /usr/local/lib/python3.13/site-packages/kb_index.py
# and also available via sys.path

RUN mkdir -p /var/lib/ephemera

EXPOSE 9354

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD python3 -c "import urllib.request; urllib.request.urlopen('http://localhost:9354/')"

# Volume mounts provide:
#   /usr/local/bin/kb_index.py  -> kb_index module
#   /data/kino/recordings       -> KB recordings
#   /data/kino/screenshots      -> KB screenshots
#   /data/ephemera              -> generated JSON output
#   /home/aoi/data/syncthing/oneplus15/Recordings  -> bind recordings
#   /home/aoi/data/syncthing/oneplus15/Screenshots -> bind screenshots

CMD ["python3", "server.py", "9354"]
