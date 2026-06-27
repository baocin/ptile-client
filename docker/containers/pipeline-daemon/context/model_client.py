#!/usr/bin/env python3
"""HTTP client helpers for ASR and OCR inference servers."""

import json
import os
import time
import urllib.request
import urllib.error
from pathlib import Path

ASR_URL = os.environ.get("ASR_SERVER_URL", "http://model-server-asr:9360")
OCR_URL = os.environ.get("OCR_SERVER_URL", "http://model-server-ocr:9361")
REQUEST_TIMEOUT = 7200  # 2 hours for long audio


def wait_for_asr(timeout=120, interval=2):
    """Block until the ASR server reports ready."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            r = urllib.request.urlopen(f"{ASR_URL}/ready", timeout=5)
            if json.loads(r.read()).get("ready"):
                return True
        except Exception:
            pass
        time.sleep(interval)
    return False


def wait_for_ocr(timeout=60, interval=2):
    """Block until the OCR server reports ready."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            r = urllib.request.urlopen(f"{OCR_URL}/ready", timeout=5)
            if json.loads(r.read()).get("ready"):
                return True
        except Exception:
            pass
        time.sleep(interval)
    return False


def transcribe_audio(wav_path: str, timeout=REQUEST_TIMEOUT) -> dict:
    """Transcribe a WAV file via the ASR inference server.

    Args:
        wav_path: Absolute path to WAV file on a volume shared with the ASR container.
        timeout: Request timeout in seconds.

    Returns:
        dict with keys: text, words, processing_time_ms, audio_duration_ms, rtf, token_count
    """
    p = Path(wav_path)
    if not p.exists():
        raise FileNotFoundError(f"Audio file not found: {wav_path}")

    data = urllib.parse.urlencode({"path": wav_path}).encode()
    req = urllib.request.Request(
        f"{ASR_URL}/transcribe",
        data=data,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        method="POST",
    )
    try:
        r = urllib.request.urlopen(req, timeout=timeout)
        return json.loads(r.read())
    except urllib.error.HTTPError as e:
        body = e.read().decode()
        raise RuntimeError(f"ASR server error {e.code}: {body}") from e


def ocr_image(image_path: str, timeout=120) -> dict:
    """OCR an image via the OCR inference server.

    Args:
        image_path: Absolute path to image on shared volume.

    Returns:
        dict with keys: text, word_count, regions, region_count
    """
    p = Path(image_path)
    if not p.exists():
        raise FileNotFoundError(f"Image file not found: {image_path}")

    data = urllib.parse.urlencode({"path": image_path}).encode()
    req = urllib.request.Request(
        f"{OCR_URL}/ocr",
        data=data,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        method="POST",
    )
    try:
        r = urllib.request.urlopen(req, timeout=timeout)
        return json.loads(r.read())
    except urllib.error.HTTPError as e:
        body = e.read().decode()
        raise RuntimeError(f"OCR server error {e.code}: {body}") from e
