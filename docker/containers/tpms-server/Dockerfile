# tpms-server — RTL-SDR signal monitor (time-mux rtl_433 + rtl_power)
FROM python:3.13-slim

WORKDIR /app

# Install system deps for RTL-SDR
RUN apt-get update && apt-get install -y --no-install-recommends \
    librtlsdr-dev rtl-sdr rtl-433 cmake build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Copy project
COPY pyproject.toml .
COPY server.py .
COPY src/ ./src/
COPY web-dashboard/ ./web-dashboard/

# Install Python deps
RUN pip install --no-cache-dir pyrtlsdr numpy scipy matplotlib pyyaml

RUN mkdir -p /data

EXPOSE 8092

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD python3 -c "import urllib.request; urllib.request.urlopen('http://localhost:8092/api/status')"

CMD ["python3", "server.py", "--port", "8092"]
