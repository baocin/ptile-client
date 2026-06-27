#!/bin/bash
# Local services health checker — monitors llama, hermes gateway, mem0
# Exit 0 = healthy, non-zero = failure (triggers systemd OnFailure)
set -euo pipefail

LOG_TAG="local-svc-health-check"
TIMEOUT=8
LOG_FILE="/home/aoi/kino/logs/site-health.log"
FAILURES=()

# Ensure log directory exists
mkdir -p "$(dirname "$LOG_FILE")"

# Tee all output to log file
exec > >(tee -a "$LOG_FILE") 2>&1

# ── llama-server check (OR logic: OK if EITHER port healthy) ──
check_llama() {
    local ok_8081=false
    local ok_8086=false

    # Check 35B on port 8081
    local resp
    resp=$(curl -sS --max-time "$TIMEOUT" http://localhost:8081/health 2>/dev/null || true)
    if echo "$resp" | grep -q '"status":"ok"'; then
        ok_8081=true
    fi

    # Check 9B on port 8086
    resp=$(curl -sS --max-time "$TIMEOUT" http://localhost:8086/health 2>/dev/null || true)
    if echo "$resp" | grep -q '"status":"ok"'; then
        ok_8086=true
    fi

    if $ok_8081 && $ok_8086; then
        echo "WARN [llama]: BOTH ports healthy (8081 + 8086) — GPU contention possible"
        return 0
    elif $ok_8081; then
        echo "OK   [llama]: 35B on port 8081 healthy"
        return 0
    elif $ok_8086; then
        echo "OK   [llama]: 9B on port 8086 healthy"
        return 0
    else
        echo "FAIL [llama]: neither port 8081 nor 8086 responded"
        FAILURES+=("llama: both ports down")
        return 1
    fi
}

# ── Hermes gateway check (process + port) ──
check_hermes_gateway() {
    local proc_ok=false
    local port_ok=false

    # Check process alive
    if pgrep -f "hermes.*gateway" > /dev/null 2>&1; then
        proc_ok=true
    fi

    # Check port 8644 responding
    local resp
    resp=$(curl -sS --max-time "$TIMEOUT" http://localhost:8644/health 2>/dev/null || true)
    if echo "$resp" | grep -q '"status"'; then
        port_ok=true
    fi

    if $proc_ok && $port_ok; then
        echo "OK   [hermes-gateway]: process alive, port 8644 responding"
        return 0
    elif $proc_ok; then
        echo "FAIL [hermes-gateway]: process alive but port 8644 not responding"
        FAILURES+=("hermes-gateway: port 8644 down")
        return 1
    elif $port_ok; then
        echo "WARN [hermes-gateway]: port 8644 responding but no matching process found"
        return 0
    else
        echo "FAIL [hermes-gateway]: no process found, port 8644 not responding"
        FAILURES+=("hermes-gateway: process and port down")
        return 1
    fi
}

# ── Embed servers check (bge-small, nomic-embed) ──
check_embed() {
    local all_ok=true

    local resp
    resp=$(curl -sS --max-time "$TIMEOUT" http://localhost:8084/v1/health 2>/dev/null || echo '{"status":"error"}')
    if echo "$resp" | grep -q '"status":"ok"'; then
        echo "OK   [embed/bge-small]: port 8084 healthy"
    else
        echo "FAIL [embed/bge-small]: port 8084 not responding"
        FAILURES+=("embed/bge-small: down")
        all_ok=false
    fi

    resp=$(curl -sS --max-time "$TIMEOUT" http://localhost:8085/v1/health 2>/dev/null || echo '{"status":"error"}')
    if echo "$resp" | grep -q '"status":"ok"'; then
        echo "OK   [embed/nomic]: port 8085 healthy"
    else
        echo "FAIL [embed/nomic]: port 8085 not responding"
        FAILURES+=("embed/nomic: down")
        all_ok=false
    fi

    if $all_ok; then
        return 0
    else
        return 1
    fi
}

# ── NULLVEC API check (port 8900, host-mode) ──
check_nullvec() {
    local resp
    resp=$(curl -sS --max-time "$TIMEOUT" http://localhost:8900/health 2>/dev/null || echo '{"status":"error"}')
    if echo "$resp" | grep -q '"status":"ok"'; then
        local count
        count=$(echo "$resp" | python3 -c "import sys,json;print(json.load(sys.stdin).get('embeddings_count','?'))" 2>/dev/null || echo "?")
        echo "OK   [nullvec]: port 8900 healthy ($count embeddings)"
        return 0
    else
        echo "FAIL [nullvec]: port 8900 not responding"
        FAILURES+=("nullvec: port 8900 down")
        return 1
    fi
}

# ── Additional embed servers check (bge-m3, gte-qwen2, jina) ──
check_embed_extra() {
    local all_ok=true

    for entry in "8087:bge-m3" "8088:gte-qwen2" "8089:jina"; do
        local port="${entry%%:*}"
        local name="${entry##*:}"
        local resp
        resp=$(curl -sS --max-time "$TIMEOUT" "http://localhost:$port/health" 2>/dev/null || echo '{"status":"error"}')
        if echo "$resp" | grep -q '"status":"ok"'; then
            echo "OK   [embed/$name]: port $port healthy"
        else
            echo "FAIL [embed/$name]: port $port not responding"
            FAILURES+=("embed/$name: port $port down")
            all_ok=false
        fi
    done

    if $all_ok; then
        return 0
    else
        return 1
    fi
}

# ── mem0 container check ──
check_mem0() {
    local containers=("mem0-dev-mem0-nomic-1" "mem0-dev-mem0-1" "mem0-dev-postgres-1")
    local all_ok=true

    for container in "${containers[@]}"; do
        local status
        status=$(docker inspect -f '{{.State.Status}}' "$container" 2>/dev/null || echo "not-found")
        if [[ "$status" == "running" ]]; then
            echo "OK   [mem0/$container]: running"
        else
            echo "FAIL [mem0/$container]: status=$status (expected running)"
            FAILURES+=("mem0/$container: $status")
            all_ok=false
        fi
    done

    if $all_ok; then
        return 0
    else
        return 1
    fi
}

# ── Gitea check (localhost:3001) ──
check_gitea() {
    local resp
    resp=$(curl -sS -o /dev/null -w "%{http_code}" --max-time "$TIMEOUT" http://localhost:3001/ 2>/dev/null || echo "000")
    if [[ "$resp" == "200" ]]; then
        echo "OK   [gitea]: port 3001 responding (HTTP $resp)"
        return 0
    else
        echo "FAIL [gitea]: port 3001 returned HTTP $resp"
        FAILURES+=("gitea: HTTP $resp")
        return 1
    fi
}

# ── SNAC web UI check (100.76.212.98:10901) ──
check_snac() {
    local resp
    resp=$(curl -sS -o /dev/null -w "%{http_code}" --max-time "$TIMEOUT" http://100.76.212.98:10901/ 2>/dev/null || echo "000")
    if [[ "$resp" == "200" ]]; then
        echo "OK   [snac]: port 10901 responding (HTTP $resp)"
        return 0
    else
        echo "WARN [snac]: port 10901 returned HTTP $resp (may be up but not on port 10901 yet)"
        return 0  # non-fatal: SNAC may be expected down
    fi
}

# ── Local Services Dashboard check (port 8090) ──
check_dashboard() {
    local resp
    resp=$(curl -sS -o /dev/null -w "%{http_code}" --max-time "$TIMEOUT" http://localhost:8090/ 2>/dev/null || echo "000")
    if [[ "$resp" == "200" ]]; then
        echo "OK   [dashboard]: port 8090 responding (HTTP $resp)"
        return 0
    else
        echo "FAIL [dashboard]: port 8090 returned HTTP $resp"
        FAILURES+=("dashboard: HTTP $resp")
        return 1
    fi
}

# ── TrueNAS Scale check (100.94.73.109:81) ──
check_truenas() {
    local resp
    resp=$(curl -sS -o /dev/null -w "%{http_code}" --max-time "$TIMEOUT" "http://100.94.73.109:81/ui/" 2>/dev/null || echo "000")
    if [[ "$resp" == "200" ]]; then
        echo "OK   [truenas]: 100.94.73.109:81 responding (HTTP $resp)"
        return 0
    else
        echo "WARN [truenas]: 100.94.73.109:81 returned HTTP $resp (may be sleeping)"
        return 0  # non-fatal: NAS may sleep
    fi
}

# ── Main ──
echo "=== Local Services Health Check — $(date -Iseconds) ==="

check_llama || true
check_hermes_gateway || true
check_embed || true
check_embed_extra || true
check_nullvec || true
check_mem0 || true
check_gitea || true
check_snac || true
check_dashboard || true
check_truenas || true

echo "---"
if [[ ${#FAILURES[@]} -eq 0 ]]; then
    echo "RESULT: All local services healthy"
    exit 0
else
    echo "RESULT: ${#FAILURES[@]} failure(s)"
    for f in "${FAILURES[@]}"; do
        echo "  - $f"
    done
    exit 1
fi
