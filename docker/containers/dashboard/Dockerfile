# dashboard — local services index page with live Docker status
FROM oven/bun:1-slim

WORKDIR /app

# Copy server and static files
COPY server.ts .
COPY index.html /app/static/index.html
COPY downloads/ /app/static/downloads/

EXPOSE 8090

# Mount /var/run/docker.sock for live container status
VOLUME ["/var/run/docker.sock"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=3s --retries=3 \
  CMD bun -e "fetch('http://localhost:8090/api/health').then(r => {if(!r.ok||r.status!==200)process.exit(1)}).catch(() => process.exit(1))"

CMD ["bun", "server.ts"]
