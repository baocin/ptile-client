# ptiles-route-compare — offline (ptiles) vs OSRM routing comparison
# Pure stdlib Python HTTP server, no pip deps.
FROM python:3.13-slim

WORKDIR /app

# Copy the app (stdlib only, no pip install needed)
COPY mvp/app.py .
COPY mvp/static/ ./static/

# The app does: Path(__file__).parent.parent / "scripts" -> /scripts/
# So we need scripts/ at the root level
COPY scripts/states.py /scripts/states.py

EXPOSE 9352

# Ptiles data directory mounted at /home/aoi/kino/projects/ptiles/data/states
# Ptiles CLI mounted at /home/aoi/kino/projects/timeline/target/release/ptiles

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD python3 -c "import urllib.request; urllib.request.urlopen('http://localhost:9352/')"

CMD ["python3", "app.py"]
