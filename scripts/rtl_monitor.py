#!/usr/bin/env python3
"""
RTL-SDR Signal Monitor -- actively scans the RF spectrum and alerts on new signals.

Usage:
  python3 rtl_monitor.py [--continuous] [--interval 60]

  --continuous: loop forever at the given interval
  --interval N: seconds between scans (default 60)

Requires: pyrtlsdr, numpy (in ~/kino/venvs/rtl-monitor)
"""

import asyncio
import json
import os
import sys
import time
from datetime import datetime

import numpy as np

# --- Configuration: frequency bands of interest ---
# (label, start_hz, end_hz, description, max_gain_db)
BANDS = [
    ("FM-broadcast", 87_500_000, 108_000_000, "Commercial FM radio", 12.5),
    ("aircraft-VHF", 108_000_000, 137_000_000, "Airband (VHF COM/NAV)", 20.0),
    ("NOAA-weather", 162_400_000, 162_550_000, "NOAA weather radio", 30.0),
    ("ham-2m", 144_000_000, 148_000_000, "2m amateur band", 30.0),
    ("DMR", 150_000_000, 174_000_000, "VHF business/DMR", 30.0),
    ("ISM-433", 433_050_000, 434_790_000, "ISM 433 MHz (LoRa, doorbells, etc.)", 30.0),
    ("ham-70cm", 420_000_000, 450_000_000, "70cm amateur band", 30.0),
    ("ADS-B", 978_000_000, 1_090_000_000, "Aircraft ADS-B (1090 MHz)", 40.0),
    ("ISM-915", 902_000_000, 928_000_000, "ISM 915 MHz (LoRa US, Zigbee)", 30.0),
    ("GMRS-FRS", 462_000_000, 467_000_000, "GMRS/FRS (US)", 30.0),
    ("pager", 929_000_000, 931_000_000, "POCSAG paging", 30.0),
    ("TV-UHF", 470_000_000, 698_000_000, "UHF TV channels", 30.0),
    ("marine-VHF", 156_000_000, 174_000_000, "Marine VHF radio", 30.0),
]

# Discrete spot-check frequencies (quick check for known things)
SPOT_FREQUENCIES = [
    (1090_000_000, "ADS-B (aircraft)"),
    (978_000_000, "UAT (weather/ADS-B)"),
    (433_900_000, "ISM 433 MHz (common LoRa)"),
    (868_000_000, "ISM 868 MHz (EU LoRa)"),
    (916_000_000, "ISM 915 MHz (US LoRa)"),
    (462_550_000, "GMRS channel 1"),
    (467_000_000, "GMRS/FRS shared"),
    (162_400_000, "NOAA WX1"),
    (162_475_000, "NOAA WX2"),
    (162_550_000, "NOAA WX3"),
]


class SignalMonitor:
    def __init__(
        self,
        noise_floor_rise=15,
        min_signal_dbfs=-75,
        min_peak_bw=5000,
        min_peak_gap=25000,
    ):
        self.noise_floor_rise = noise_floor_rise
        self.min_signal_dbfs = min_signal_dbfs
        self.min_peak_bw = min_peak_bw  # ignore narrower peaks (noise bins)
        self.min_peak_gap = min_peak_gap  # merge peaks closer than this
        self.history = {}
        self.new_signals = []
        self.scan_count = 0

    async def scan_band(self, sdr, band_label, start_hz, end_hz, max_gain):
        """Scan a band, return (peaks, noise_floor)."""
        bandwidth = end_hz - start_hz
        step_hz = min(2_000_000, bandwidth)

        all_freqs = []
        all_powers = []

        freq = start_hz
        while freq < end_hz:
            chunk_end = min(freq + step_hz, end_hz)
            chunk_center = (freq + chunk_end) // 2

            try:
                sdr.center_freq = chunk_center
                sdr.sample_rate = 2_048_000

                if max_gain is not None:
                    sdr.gain = min(20.0, max_gain)
                else:
                    sdr.gain = "auto"

                await asyncio.sleep(0.05)

                samples = sdr.read_samples(256 * 1024)
                window = np.hanning(len(samples))
                windowed = samples * window

                # 1024-point FFT gives ~2 kHz resolution at 2 MS/s
                spectrum = np.fft.fftshift(np.fft.fft(windowed, n=1024))
                power = 20 * np.log10(np.abs(spectrum) + 1e-12)

                freqs = np.linspace(freq, chunk_end, len(power))
                all_freqs.append(freqs)
                all_powers.append(power)
            except Exception as e:
                print(
                    f"    Error at {chunk_center / 1e6:.1f} MHz: {e}", file=sys.stderr
                )

            freq = chunk_end

        if not all_freqs:
            return [], -100

        all_freqs = np.concatenate(all_freqs)
        all_powers = np.concatenate(all_powers)

        noise_floor = float(np.percentile(all_powers, 10))
        threshold = max(noise_floor + self.noise_floor_rise, self.min_signal_dbfs)

        # Smooth: rolling max over a small window to filter single-bin noise
        window_len = max(3, int(5000 / (bandwidth / len(all_powers))) + 1)  # ~5 kHz
        smoothed = (
            np.maximum.accumulate(
                np.maximum.accumulate(all_powers[::-1])[::-1][:window_len]
            )
            if window_len > 1
            else all_powers
        )

        above = all_powers > threshold

        peaks = []
        i = 0
        while i < len(above):
            if above[i]:
                j = i
                while j < len(above) and above[j]:
                    j += 1

                bw = float(all_freqs[j - 1] - all_freqs[i])
                if bw < self.min_peak_bw:
                    i = j
                    continue

                region_power = all_powers[i:j]
                peak_idx = np.argmax(region_power)
                freq_hz = float(all_freqs[i + peak_idx])
                power_dbfs = float(region_power[peak_idx])

                peaks.append(
                    {
                        "frequency": freq_hz,
                        "power_dBFS": power_dbfs,
                        "bandwidth_hz": bw,
                        "above_noise_db": power_dbfs - noise_floor,
                    }
                )
                i = j
            else:
                i += 1

        # Merge peaks that are within min_peak_gap of each other
        if peaks:
            merged = [peaks[0]]
            for p in peaks[1:]:
                last = merged[-1]
                if abs(p["frequency"] - last["frequency"]) < self.min_peak_gap:
                    # Keep the stronger one
                    if p["power_dBFS"] > last["power_dBFS"]:
                        merged[-1] = p
                else:
                    merged.append(p)
            peaks = merged

        return peaks, noise_floor

    def classify_signal(self, p):
        """Determine signal type from frequency and characteristics."""
        f = p["frequency"]
        bw = p["bandwidth_hz"]
        tags = []

        if 88e6 <= f <= 108e6:
            tags.append("FM-broadcast")
        elif 162.4e6 <= f <= 162.55e6:
            tags.append("NOAA-WX")
        elif 108e6 <= f <= 137e6:
            tags.append("airband")
        elif 144e6 <= f <= 148e6:
            tags.append("ham-2m")
        elif 156e6 <= f <= 174e6:
            tags.append("marine-VHF")
        elif 420e6 <= f <= 450e6:
            tags.append("ham-70cm")
        elif 433e6 <= f <= 435e6:
            tags.append("ISM-433")
        elif 902e6 <= f <= 928e6:
            tags.append("ISM-915")
        elif 978e6 <= f <= 1090e6:
            tags.append("aircraft")
        elif 462e6 <= f <= 467e6:
            tags.append("GMRS-FRS")
        elif 929e6 <= f <= 931e6:
            tags.append("pager")
        elif 470e6 <= f <= 698e6:
            tags.append("TV-UHF")

        if p["power_dBFS"] > -40:
            tags.append("strong")
        elif p["power_dBFS"] > -60:
            tags.append("moderate")
        else:
            tags.append("weak")

        return tags

    def format_freq(self, hz):
        if hz >= 1e9:
            return f"{hz / 1e9:.4f} GHz"
        elif hz >= 1e6:
            return f"{hz / 1e6:.4f} MHz"
        elif hz >= 1e3:
            return f"{hz / 1e3:.2f} kHz"
        return f"{hz:.0f} Hz"

    async def spot_check(self, sdr):
        """Quick energy check on specific frequencies."""
        print("\n  -- Spot checks --")
        for freq, label in SPOT_FREQUENCIES:
            try:
                sdr.center_freq = freq
                await asyncio.sleep(0.02)
                s = sdr.read_samples(128 * 1024)
                power_dbfs = 20 * np.log10(np.mean(np.abs(s)) + 1e-12)
                if power_dbfs > -55:
                    print(
                        f"    {self.format_freq(freq):>14s}  {power_dbfs:>6.1f} dBFS  ACTIVE  [{label}]"
                    )
            except:
                pass

    async def run_scan(self):
        """Run one full spectrum scan."""
        now = datetime.now()
        print(f"\n{'=' * 65}")
        print(f"  Scan #{self.scan_count + 1}  {now.strftime('%Y-%m-%d %H:%M:%S')}")
        print(f"{'=' * 65}")

        import rtlsdr

        sdr = rtlsdr.RtlSdr()

        new_this_scan = []

        try:
            await self.spot_check(sdr)

            for band_label, start, end, desc, max_gain in BANDS:
                print(f"\n  [{band_label}] {desc}")
                print(
                    f"    {self.format_freq(start)} - {self.format_freq(end)}"
                    f"  {f'(gain capped at {max_gain:.0f} dB)' if max_gain else '(auto gain)'}"
                )

                peaks, noise_floor = await self.scan_band(
                    sdr, band_label, start, end, max_gain
                )

                if not peaks:
                    print(f"    Noise floor: {noise_floor:.1f} dBFS  -- baseline")
                    continue

                print(
                    f"    Noise floor: {noise_floor:.1f} dBFS  "
                    f"{len(peaks)} signal(s) detected"
                )

                prev_freqs = self.history.get(band_label, {}).get("freq_set", set())
                curr_freq_set = set()

                for peak in peaks:
                    tags = self.classify_signal(peak)
                    tags_str = ", ".join(tags)

                    # Round to 25 kHz bins for cross-scan matching
                    freq_key = round(peak["frequency"] / 25e3) * 25e3
                    curr_freq_set.add(freq_key)

                    is_new = freq_key not in prev_freqs
                    marker = "  NEW " if is_new else "      "

                    print(
                        f"    {marker} {self.format_freq(peak['frequency']):>14s}  "
                        f"{peak['power_dBFS']:>6.1f} dBFS  "
                        f"+{peak['above_noise_db']:>4.1f} dB  "
                        f"BW: {self.format_freq(peak['bandwidth_hz']):>10s}  "
                        f"[{tags_str}]"
                    )

                    if is_new:
                        new_this_scan.append(peak)

                self.history[band_label] = {
                    "timestamp": time.time(),
                    "freq_set": curr_freq_set,
                    "noise_floor": noise_floor,
                    "peak_count": len(peaks),
                }
        finally:
            sdr.close()

        self.scan_count += 1

        if new_this_scan:
            print(f"\n  >>> {len(new_this_scan)} new signal(s) detected!")
            self.new_signals.extend(new_this_scan)
        else:
            print("\n  No new signals beyond previous scan.")

        return new_this_scan

    def print_summary(self):
        print(f"\n{'=' * 65}")
        print(
            f"  Summary ({self.scan_count} scan(s), "
            f"{len(self.new_signals)} new signals tracked)"
        )
        print(f"{'=' * 65}")
        for band_label, start, end, desc, _ in BANDS:
            data = self.history.get(band_label)
            if data:
                print(
                    f"  {band_label:20s}  NF: {data['noise_floor']:>6.1f} dBFS  "
                    f"peaks: {data.get('peak_count', 0)}"
                )
            else:
                print(f"  {band_label:20s}  -- not scanned")


async def main():
    import argparse

    parser = argparse.ArgumentParser(description="RTL-SDR Signal Monitor")
    parser.add_argument(
        "--continuous",
        "-c",
        action="store_true",
        help="Run continuously, rescanning at interval",
    )
    parser.add_argument(
        "--interval",
        "-i",
        type=int,
        default=60,
        help="Seconds between scans (default: 60)",
    )
    parser.add_argument(
        "--output",
        "-o",
        default=os.path.expanduser("~/kino/rtl_scans.json"),
        help="Output file (default: ~/kino/rtl_scans.json)",
    )
    args = parser.parse_args()

    monitor = SignalMonitor()

    if args.continuous:
        print(f"Continuous monitoring every {args.interval}s. Ctrl+C to stop.")
        try:
            while True:
                await monitor.run_scan()
                monitor.print_summary()
                print(f"\n  Next scan in {args.interval}s...")
                await asyncio.sleep(args.interval)
        except KeyboardInterrupt:
            print("\n  Stopped.")
    else:
        await monitor.run_scan()

    monitor.print_summary()

    # Save scan history
    history_simple = {}
    for k, v in monitor.history.items():
        history_simple[k] = {
            "timestamp": v.get("timestamp", 0),
            "noise_floor": v.get("noise_floor", 0),
            "peak_count": v.get("peak_count", 0),
        }
    with open(args.output, "w") as f:
        json.dump(
            {
                "last_scan": datetime.now().isoformat(),
                "scan_count": monitor.scan_count,
                "total_new_signals": len(monitor.new_signals),
                "bands": history_simple,
            },
            f,
            indent=2,
        )
    print(f"\n  Results saved to {args.output}")


if __name__ == "__main__":
    asyncio.run(main())
