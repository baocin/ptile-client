#!/usr/bin/env python3
"""TUI for managing inference services. Arrow keys + space to toggle. Auto-refreshing VRAM."""

import subprocess
import shutil
import sys
import os
import time
import select
import threading

SERVICES = [
    ("llama-server-ornith", "Ornith 35B", True),
    ("llama-server-qwen35", "Qwen 35B", True),
    ("llama-server-lfm", "LFM 230M", True),
    ("comfyui", "ComfyUI (SD)", True),
    ("llama-proxy", "Prompt Log Proxy", False),
    ("embed-bge", "BGE Small (embed)", True),
    ("embed-nomic", "Nomic (embed)", True),
    ("embed-bge-m3", "BGE M3 (embed)", True),
    ("model-server-asr", "ASR (Parakeet)", True),
    ("model-server-ocr", "OCR (PP-OCRv6)", True),
]


def run(cmd, timeout=5):
    try:
        return subprocess.run(
            cmd, shell=True, capture_output=True, text=True, timeout=timeout
        ).stdout.strip()
    except:
        return ""


DOCKER_SERVICES = {"model-server-asr", "model-server-ocr"}
LFM_SERVICES = {"llama-server-lfm"}
COMPOSE_DIR = os.path.expanduser("~/kino/docker")


def get_state(svc):
    if svc in DOCKER_SERVICES:
        o = run(f"docker ps --filter name={svc} --format '{{{{.Status}}}}'", timeout=5)
        if "Up" in o:
            return "active", ""
        return "inactive", ""
    a = run(f"systemctl --user is-active {svc}.service 2>/dev/null || echo inactive")
    e = run(f"systemctl --user is-enabled {svc}.service 2>/dev/null || echo disabled")
    return (a or "inactive"), ("yes" if e in ("alias", "enabled") else "")


def get_vram():
    out = run(
        "nvidia-smi --query-gpu=index,name,memory.used,utilization.gpu --format=csv,noheader"
    )
    lines = out.strip().split("\n")
    return [l for l in lines if l] if lines else ["(no GPU data)"]


def toggle(svc, action):
    if svc in DOCKER_SERVICES:
        if action == "start":
            run(f"cd {COMPOSE_DIR} && docker compose up -d --no-deps {svc}", timeout=30)
        else:
            run(f"cd {COMPOSE_DIR} && docker compose stop {svc}", timeout=30)
        return
    run(f"systemctl --user {action} {svc}.service 2>/dev/null", timeout=10)


def read_key():
    if select.select([sys.stdin], [], [], 0.1)[0]:
        c = sys.stdin.read(1)
        if c == "\x1b":
            seq = sys.stdin.read(2)
            return {"[A": "KEY_UP", "[B": "KEY_DOWN"}.get(seq, "ESC")
        return c
    return None


vram_cache = get_vram()
statuses = [(get_state(svc)) for svc, _, _ in SERVICES]


def bg_refresh():
    global vram_cache, statuses
    while True:
        time.sleep(5)
        vram_cache = get_vram()
        statuses = [get_state(svc) for svc, _, _ in SERVICES]


def draw(selected):
    w = shutil.get_terminal_size().columns
    sys.stdout.write("\033[H\033[J")

    # Title bar
    print("\033[1;37m" + "=" * w + "\033[0m")
    print(f"\033[1;36m{'  S E R V I C E   M A N A G E R  ':^{w}}\033[0m")
    print("\033[1;37m" + "=" * w + "\033[0m")
    print()

    # Table
    print(f"\033[1;90m  {'':3s}  {'SERVICE':26s}  {'GPU':5s}  {'BOOT':4s}\033[0m")
    print(f"\033[1;90m  {'':3s}  {'-------':26s}  {'---':5s}  {'----':4s}\033[0m")

    for i, (svc, label, has_vram) in enumerate(SERVICES):
        state, enabled = statuses[i]
        active = state in ("active", "activating")

        # GPU column — color by card
        if has_vram and active:
            if svc in LFM_SERVICES:
                gpu_label = "2070S"
                gpu_str = f"\033[1;36m{gpu_label:>5s}\033[0m"  # cyan for 2070S
            elif svc.startswith("embed-"):
                gpu_label = "CPU"
                gpu_str = "\033[1;37m  CPU\033[0m"  # white for CPU
            elif svc in DOCKER_SERVICES:
                gpu_label = "2070S"
                gpu_str = f"\033[1;36m{gpu_label:>5s}\033[0m"  # cyan for 2070S
            else:
                gpu_label = "3090"
                gpu_str = f"\033[1;32m{gpu_label:>5s}\033[0m"  # green for 3090
        elif has_vram and not active:
            gpu_str = "\033[1;90m  ---\033[0m"
        else:
            gpu_str = "\033[1;37m  CPU\033[0m"  # white for CPU-always services

        # Boot column
        boot_str = (
            "\033[1;32m  \u2713\033[0m"
            if enabled == "yes"
            else "\033[1;90m  \u2717\033[0m"
        )

        # Highlight selected row
        marker = "\033[1;33m>\033[0m" if i == selected else " "
        rev = "\033[7m" if i == selected else ""
        reset = "\033[0m" if i == selected else ""

        row = f"  {marker} {rev}  {label:26s}  {gpu_str:5s}  {boot_str:4s}{reset}"
        print(row)

    print()
    print("  " + "\033[1;90m" + "-" * (w - 4) + "\033[0m")

    # GPU bars — VRAM + util
    for g in vram_cache:
        parts = g.split(",")
        if len(parts) >= 4:
            idx, name, mem, util = [p.strip() for p in parts[:4]]
            mem_mb = int(mem.split()[0])
            total = 24576 if "3090" in name else 8192
            if mem_mb > total * 0.85:
                color = "\033[1;31m"  # red if >85% full
            elif "3090" in name:
                color = "\033[1;33m"  # yellow for 3090
            else:
                color = "\033[1;36m"  # cyan for 2070S

            # VRAM bar
            fill = int(mem_mb / total * 16)
            bar = "█" * fill + "░" * (16 - fill)

            # Util bar
            util_pct = int(util.strip().rstrip("%"))
            ufill = int(util_pct / 100 * 10)
            ubar = "█" * ufill + "░" * (10 - ufill)

            print(f"  {color}GPU {idx}  [{bar}] {mem:>6s}  [{ubar}] {util:>4s}\033[0m")
        else:
            print(f"  {g}")
    print()

    print(
        "  \033[1;90m[\u2191/\u2193] Navigate  [Space] Toggle  [a] All on  [o] All off  [q] Quit\033[0m"
    )
    sys.stdout.flush()


def main():
    selected = 0
    t = threading.Thread(target=bg_refresh, daemon=True)
    t.start()
    draw(selected)
    if os.name == "posix":
        import tty
        import termios

        fd = sys.stdin.fileno()
        old = termios.tcgetattr(fd)
        try:
            tty.setcbreak(fd)
            _loop(selected)
        finally:
            termios.tcsetattr(fd, termios.TCSADRAIN, old)
    else:
        _loop(selected)


def _loop(selected):
    draw(selected)
    while True:
        key = read_key()
        if key is None:
            draw(selected)
            continue
        elif key == "q":
            break
        elif key == "KEY_UP" and selected > 0:
            selected -= 1
            draw(selected)
        elif key == "KEY_DOWN" and selected < len(SERVICES) - 1:
            selected += 1
            draw(selected)
        elif key == "a":
            for svc, _, _ in SERVICES:
                toggle(svc, "start")
            time.sleep(1.5)
            draw(selected)
        elif key == "o":
            for svc, _, _ in SERVICES:
                toggle(svc, "stop")
            run("kill -9 $(pgrep -f 'llama-server') 2>/dev/null", timeout=3)
            # Also stop any docker containers that might have been started outside compose
            run(
                "docker stop $(docker ps -q --filter name=model-server-) 2>/dev/null",
                timeout=10,
            )
            time.sleep(1.5)
            draw(selected)
        elif key in (" ", "\r", "\n"):  # space or enter
            svc, _, _ = SERVICES[selected]
            state, _ = statuses[selected]
            action = "stop" if state in ("active", "activating") else "start"
            toggle(svc, action)
            time.sleep(0.8)
            draw(selected)


if __name__ == "__main__":
    main()
