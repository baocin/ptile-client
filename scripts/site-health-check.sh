#!/bin/bash
# Site health checker — validates HTML content, not just HTTP 200
# Exit 0 = healthy, non-zero = failure (triggers systemd OnFailure)
set -euo pipefail

LOG_TAG="site-health-check"
TIMEOUT=12
LOG_FILE="/home/aoi/kino/logs/site-health.log"
FAILURES=()

# Ensure log directory exists
mkdir -p "$(dirname "$LOG_FILE")"

# Tee all output to log file
exec > >(tee -a "$LOG_FILE") 2>&1

check_site() {
    local name="$1"
    local url="$2"
    local expected_content="$3"
    
    local response
    response=$(curl -sS --max-time "$TIMEOUT" -o /tmp/site-check-$$.html -w "%{http_code}" "$url" 2>/tmp/site-check-$$.err) || true
    local http_code="$response"
    local curl_err
    curl_err=$(cat /tmp/site-check-$$.err 2>/dev/null || true)
    
    if [[ "$http_code" != "200" ]]; then
        echo "FAIL [$name]: HTTP $http_code (expected 200)"
        [[ -n "$curl_err" ]] && echo "  curl error: $curl_err"
        FAILURES+=("$name: HTTP $http_code")
        return 1
    fi
    
    if ! grep -q "$expected_content" /tmp/site-check-$$.html 2>/dev/null; then
        local body_preview
        body_preview=$(head -c 500 /tmp/site-check-$$.html 2>/dev/null || echo "(empty)")
        echo "FAIL [$name]: content check failed — expected '$expected_content' not found"
        echo "  body preview: $body_preview"
        FAILURES+=("$name: content mismatch")
        return 1
    fi
    
    local body_size
    body_size=$(wc -c < /tmp/site-check-$$.html 2>/dev/null || echo "0")
    echo "OK   [$name]: 200, $(echo "$body_size" | tr -d ' ') bytes, content verified"
    return 0
}

cleanup() {
    rm -f /tmp/site-check-$$.html /tmp/site-check-$$.err
}
trap cleanup EXIT

echo "=== Site Health Check — $(date -Iseconds) ==="

check_site "steele.red"     "https://steele.red"         "Steele" || true
check_site "mydata"       "https://mydatatimeline.com" "Timeline" || true

echo "---"
if [[ ${#FAILURES[@]} -eq 0 ]]; then
    echo "RESULT: All sites healthy"
    exit 0
else
    echo "RESULT: ${#FAILURES[@]} failure(s)"
    for f in "${FAILURES[@]}"; do
        echo "  - $f"
    done
    exit 1
fi
