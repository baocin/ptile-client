#!/usr/bin/env python3
"""Generate system dashboard HTML — updated by cron."""

import subprocess, json, os, time, sqlite3
from datetime import datetime

DASHBOARD = os.path.expanduser("~/kino/dashboard/index.html")
os.makedirs(os.path.dirname(DASHBOARD), exist_ok=True)

def sh(cmd, timeout=10):
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        return r.stdout.strip()
    except: return ""

def svc_active(name):
    return sh(["systemctl", "--user", "is-active", name]) == "active"

def pg_count(query):
    r = sh(["docker", "exec", "mem0-dev-postgres-1", "psql", "-U", "postgres", "-d", "postgres",
            "-t", "-A", "-c", query], timeout=10)
    try: return int(r)
    except: return 0

def fetch(url, timeout=5):
    try:
        r = subprocess.run(["curl", "-sS", "--max-time", str(timeout), url],
                          capture_output=True, text=True, timeout=timeout+2)
        return r.stdout
    except: return ""

now = datetime.now().strftime("%Y-%m-%d %H:%M:%S")

uptime = sh(["uptime", "-p"])
load = sh(["cat", "/proc/loadavg"]).split()[:3]
mem = sh(["free", "-h"]).splitlines()[1].split()
ram_used, ram_total = mem[2], mem[1]
disk = sh(["df", "-h", "/"]).splitlines()[1].split()
disk_used, disk_total, disk_pct = disk[2], disk[1], disk[4]

gpu_raw = sh(["nvidia-smi", "--query-gpu=memory.used,memory.free,utilization.gpu",
               "--format=csv,noheader,nounits"], timeout=10)
gpu_parts = gpu_raw.split(",")
gpu_used = gpu_parts[0].strip() if len(gpu_parts) > 0 else "?"
gpu_free = gpu_parts[1].strip() if len(gpu_parts) > 1 else "?"
gpu_util = gpu_parts[2].strip() if len(gpu_parts) > 2 else "?"
gpu_procs = sh(["nvidia-smi", "--query-compute-apps=process_name,used_memory",
                 "--format=csv,noheader,nounits"], timeout=10).splitlines()

services = {}
for s in ["embed-bge", "embed-nomic", "mem0-stack", "mem0-v1-proxy",
           "signal-cli", "gitea", "hermes-dashboard"]:
    services[s] = svc_active(s)
services["hermes-gateway"] = sh(["systemctl", "is-active", "hermes-gateway"]) == "active"
services["llama-35B"] = '"status":"ok"' in fetch("http://localhost:8081/health", 3)
services["llama-9B"] = '"status":"ok"' in fetch("http://localhost:8086/health", 3)

bge_total = pg_count("SELECT COUNT(*) FROM memories WHERE payload->>'user_id'='kino'")
nomic_total = pg_count("SELECT COUNT(*) FROM memories_nomic WHERE payload->>'user_id'='kino'")
bge_tweets = pg_count("SELECT COUNT(*) FROM memories WHERE payload->>'user_id'='kino' AND payload->>'entity_type'='tweet'")
nomic_tweets = pg_count("SELECT COUNT(*) FROM memories_nomic WHERE payload->>'user_id'='kino' AND payload->>'entity_type'='tweet'")
nomic_gap = pg_count("SELECT COUNT(*) FROM memories m WHERE m.payload->>'user_id'='kino' AND m.payload->>'hash' NOT IN (SELECT COALESCE(n.payload->>'hash','') FROM memories_nomic n WHERE n.payload->>'user_id'='kino')")

queue_size = "?"
qdb = os.path.expanduser("~/.hermes/nomic_queue.db")
if os.path.exists(qdb):
    try:
        c = sqlite3.connect(qdb, timeout=3)
        queue_size = str(c.execute("SELECT COUNT(*) FROM nomic_queue").fetchone()[0])
        c.close()
    except: pass

docker_ps = sh(["docker", "ps", "--format", "{{.Names}}"]).splitlines()
nfs_mounts = [l for l in sh(["mount", "-t", "nfs4"]).splitlines() if l]

cron_file = os.path.expanduser("~/.hermes/cron/jobs.json")
cron_jobs = []
if os.path.exists(cron_file):
    try:
        with open(cron_file) as f:
            raw = json.load(f)
            cron_jobs = list(raw.get("jobs", {}).values())
    except: pass

# Build HTML — simple string building
lines = []
def L(s=""): lines.append(s)

L("<!DOCTYPE html>")
L('<html lang="en">')
L("<head>")
L('<meta charset="UTF-8">')
L('<meta name="viewport" content="width=device-width, initial-scale=1.0">')
L("<title>hino-omarchy dashboard</title>")
L("<style>")
L("*{margin:0;padding:0;box-sizing:border-box}")
L("body{font-family:'SF Mono','Cascadia Code',monospace;background:#0a0a0f;color:#c8c8d0;padding:24px}")
L("h1{font-size:20px;color:#e8e8f0;margin-bottom:4px}")
L(".ts{font-size:11px;color:#555;margin-bottom:24px}")
L(".grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(340px,1fr));gap:16px}")
L(".card{background:#12121a;border:1px solid #1e1e2a;border-radius:8px;padding:16px}")
L(".card h2{font-size:12px;text-transform:uppercase;letter-spacing:1px;color:#555;margin-bottom:12px}")
L(".stat{display:flex;justify-content:space-between;padding:4px 0;font-size:12px}")
L(".stat .val{color:#e8e8f0;font-weight:600}")
L(".badge{display:inline-block;padding:2px 8px;border-radius:4px;font-size:10px;font-weight:600}")
L(".badge.ok{background:#14532d;color:#4ade80}")
L(".badge.fail{background:#450a0a;color:#f87171}")
L(".bar{height:6px;background:#1e1e2a;border-radius:3px;margin:4px 0;overflow:hidden}")
L(".bar-fill{height:100%;border-radius:3px}")
L("small{font-size:10px;color:#666}")
L("table{width:100%;font-size:11px;border-collapse:collapse}")
L("td{padding:4px 8px;border-bottom:1px solid #1a1a25}")
L("td:last-child{text-align:right}")
L(".mono{font-family:monospace;font-size:11px}")
L("</style></head><body>")

L("<h1>hino-omarchy &middot; dashboard</h1>")
L('<div class="ts">Updated: ' + now + ' &middot; <a href="#" onclick="location.reload()" style="color:#6a9bcc">refresh</a></div>')
L('<div class="grid">')

# System card
L('<div class="card"><h2>system</h2>')
L('<div class="stat"><span>uptime</span><span class="val">' + uptime + '</span></div>')
L('<div class="stat"><span>load</span><span class="val">' + " ".join(load) + '</span></div>')
L('<div class="stat"><span>ram</span><span class="val">' + ram_used + " / " + ram_total + '</span></div>')
dp = float(disk_pct.strip("%"))
dc = "#4ade80" if dp < 70 else "#facc15" if dp < 90 else "#f87171"
L('<div class="bar"><div class="bar-fill" style="width:' + str(dp) + '%;background:' + dc + '"></div></div>')
L("<small>/ " + disk_used + " / " + disk_total + " (" + disk_pct + ")</small>")
L("</div>")

# GPU card
L('<div class="card"><h2>gpu &middot; rtx 2070 super</h2>')
L('<div class="stat"><span>vram</span><span class="val">' + gpu_used + " MB / " + gpu_free + " MB free</span></div>")
L('<div class="stat"><span>util</span><span class="val">' + gpu_util + "%</span></div>")
for pl in gpu_procs[:4]:
    parts = pl.split(",")
    if len(parts) >= 2:
        L('<div class="stat mono"><span>' + parts[0][:35] + '</span><span>' + parts[1].strip() + ' MB</span></div>')
L("</div>")

# Services card
L('<div class="card"><h2>services</h2>')
for s, ok in sorted(services.items()):
    cls = "ok" if ok else "fail"
    txt = "ACTIVE" if ok else "DOWN"
    L('<div class="stat"><span>' + s + '</span><span class="badge ' + cls + '">' + txt + '</span></div>')
L("</div>")

# Mem0 bge
L('<div class="card"><h2>mem0 &middot; bge (8888)</h2><table>')
L("<tr><td>tweets</td><td>" + f"{bge_tweets:,}" + "</td></tr>")
L("<tr><td>total</td><td>" + f"{bge_total:,}" + "</td></tr>")
L("</table></div>")

# Mem0 nomic
L('<div class="card"><h2>mem0 &middot; nomic (8889)</h2><table>')
L("<tr><td>tweets</td><td>" + f"{nomic_tweets:,}" + "</td></tr>")
L("<tr><td>total</td><td>" + f"{nomic_total:,}" + "</td></tr>")
L("</table></div>")

# Queue
catchup = int(nomic_total / max(bge_total, 1) * 100) if bge_total else 0
L('<div class="card"><h2>queue</h2>')
L('<div class="stat"><span>nomic queue</span><span class="val">' + queue_size + '</span></div>')
L('<div class="stat"><span>bge&rarr;nomic gap</span><span class="val">' + f"{nomic_gap:,}" + '</span></div>')
L('<div class="stat"><span>catch-up</span><span class="val">' + str(catchup) + "%</span></div>")
L("</div>")

# Docker
L('<div class="card"><h2>docker</h2>')
L('<div class="stat"><span>running</span><span class="val">' + str(len(docker_ps)) + '</span></div>')
for c in docker_ps:
    L('<div class="stat mono"><span>' + c[:45] + '</span></div>')
L("</div>")

# NFS
L('<div class="card"><h2>nfs mounts</h2>')
if nfs_mounts:
    for m in nfs_mounts[:8]:
        parts = m.split()
        remote = parts[0] if parts else "?"
        L('<div class="stat mono"><span>' + remote[:35] + '</span></div>')
else:
    L('<div class="stat" style="color:#555">none</div>')
L("</div>")

# Cron
L('<div class="card"><h2>cron jobs</h2>')
if cron_jobs:
    for j in cron_jobs[:15]:
        name = j.get("name", "?")[:25]
        status = j.get("last_status", "?")
        sc = "healthy" if status == "ok" else "warning"
        L('<div class="stat"><span>' + name + '</span><span class="val ' + sc + '">' + status + '</span></div>')
else:
    L('<div class="stat" style="color:#555">no data</div>')
L("</div>")

L("</div></body></html>")

with open(DASHBOARD, "w") as f:
    f.write("\n".join(lines))

print("OK: " + DASHBOARD)
print("Serve: python3 -m http.server 10900 -d ~/kino/dashboard")
print("URL: http://100.76.212.98:10900")
