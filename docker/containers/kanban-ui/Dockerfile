# kanban-ui — Bun web server for KB kanban board
# Single-stdlib Bun app, no npm deps. Mounts kanban board dir as volume.
FROM oven/bun:1-slim

WORKDIR /app

COPY server.ts .
COPY kanban.js .

# Create log directory  
RUN mkdir -p /var/log/kanban

ENV HOME=/app

EXPOSE 8499

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD bun -e "fetch('http://localhost:8499/api/board').then(r => {if(!r.ok)process.exit(1)}).catch(() => process.exit(1))"

CMD ["bun", "server.ts"]
