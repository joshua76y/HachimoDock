#!/usr/bin/env python3
"""语音草稿链路冒烟测试（Wi-Fi PTT → ASR → 草稿确认 → 取消）。

为什么需要它：这条链路跨设备固件、PC 语音桥（8901）、ASR 与渲染，出问题时
只能靠人工按 PTT 观察。本脚本用真实的 HTTP 协议把链路跑一遍，并对设备屏幕
做白屏检测，可在改动固件/桥之后快速回归。

协议要点（2026-11 现行实现）：
  POST /voice/begin?sid=<16 hex>   —— 冻结本次录音的目标 agent
  POST /voice?sid=<16 hex>         —— 上传整段 WAV，必须与 begin 同 peer、同 sid
  GET  /status                     —— voiceDraft 字段反映设备是否进入草稿模式
  GET  /shot                       —— 设备屏幕 24bit BMP，可用于白屏检测
  POST /voice/cancel               —— 仅 127.0.0.1 可用，等效于取消按钮

WAV 必须是严格的 44 字节头（RIFF/WAVE/fmt /data，16 kHz 单声道 s16le）。
macOS `say` 产出的 WAV 带 JUNK 块，ffmpeg 产出带 LIST 块，都会被桥拒绝，
所以这里会重新打包。

用法：
  python3 firmware/tools/voice_e2e_smoke.py
  python3 firmware/tools/voice_e2e_smoke.py --device 192.168.1.97 --keep-draft
"""
from __future__ import annotations

import argparse
import json
import os
import random
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from typing import Optional

OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))
WHITE_RATIO_LIMIT = 0.90  # 超过即视为白屏/渲染异常


def http(method: str, url: str, data: Optional[bytes] = None,
         timeout: float = 30.0) -> tuple[int, bytes]:
    req = urllib.request.Request(url, data=data, method=method)
    if data is not None:
        req.add_header("Content-Type", "audio/wav")
    try:
        with OPENER.open(req, timeout=timeout) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as exc:  # 保留响应体便于断言
        return exc.code, exc.read()


def repack_wav(path: str) -> bytes:
    """把任意 WAV 重打包成固件/桥要求的 44 字节头格式。"""
    raw = open(path, "rb").read()
    if raw[:4] != b"RIFF" or raw[8:12] != b"WAVE":
        raise SystemExit(f"{path} 不是 WAV")
    i, fmt, pcm = 12, None, None
    while i + 8 <= len(raw):
        cid = raw[i:i + 4]
        size = struct.unpack_from("<I", raw, i + 4)[0]
        body = raw[i + 8:i + 8 + size]
        if cid == b"fmt ":
            fmt = body
        elif cid == b"data":
            pcm = body
        i += 8 + size + (size & 1)
    if not fmt or not pcm:
        raise SystemExit(f"{path} 缺少 fmt/data 块")
    channels, rate, bits = struct.unpack_from("<H", fmt, 2)[0], struct.unpack_from("<I", fmt, 4)[0], struct.unpack_from("<H", fmt, 14)[0]
    if (channels, rate, bits) != (1, 16000, 16):
        raise SystemExit(f"{path} 格式为 {channels}ch/{rate}Hz/{bits}bit，需要 1ch/16000Hz/16bit")
    header = (b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVE" + b"fmt "
              + struct.pack("<IHHIIHH", 16, 1, channels, rate,
                            rate * channels * bits // 8, channels * bits // 8, bits)
              + b"data" + struct.pack("<I", len(pcm)))
    return header + pcm


def build_speech_wav(tmpdir: str) -> bytes:
    """优先用 say+ffmpeg 合成一段有内容的语音；失败则用 1 秒 440Hz 正弦。"""
    say = shutil.which("say")
    ffmpeg = shutil.which("ffmpeg")
    if say and ffmpeg:
        aiff = os.path.join(tmpdir, "probe.aiff")
        raw_wav = os.path.join(tmpdir, "probe.wav")
        if subprocess.run([say, "-o", aiff, "语音链路冒烟测试"],
                          capture_output=True).returncode == 0 \
                and subprocess.run([ffmpeg, "-y", "-i", aiff, "-ar", "16000", "-ac", "1",
                                    "-c:a", "pcm_s16le", raw_wav],
                                   capture_output=True).returncode == 0:
            return repack_wav(raw_wav)
    frames = 16000  # 1s
    samples = b"".join(struct.pack("<h", int(8000 * ((k % 40) / 20.0 - 1)))
                       for k in range(frames))
    header = (b"RIFF" + struct.pack("<I", 36 + len(samples)) + b"WAVE" + b"fmt "
              + struct.pack("<IHHIIHH", 16, 1, 1, 16000, 32000, 2, 16)
              + b"data" + struct.pack("<I", len(samples)))
    return header + samples


def white_ratio(bmp: bytes) -> float:
    if bmp[:2] != b"BM":
        raise SystemExit("截图不是 BMP")
    offset = struct.unpack_from("<I", bmp, 10)[0]
    width = struct.unpack_from("<i", bmp, 18)[0]
    height = struct.unpack_from("<i", bmp, 22)[0]
    row = ((width * 3) + 3) // 4 * 4
    white = total = 0
    for y in range(0, height, 8):
        base = offset + (height - 1 - y) * row
        for x in range(0, width, 8):
            i = base + x * 3
            b, g, r = bmp[i], bmp[i + 1], bmp[i + 2]
            total += 1
            if r > 245 and g > 245 and b > 245:
                white += 1
    return white / max(total, 1)


def parse_json(body: bytes) -> dict:
    try:
        return json.loads(body.decode("utf-8", "replace"))
    except json.JSONDecodeError:
        return {"_raw": body[:200].decode("utf-8", "replace")}


class Case:
    def __init__(self) -> None:
        self.failures: list[str] = []

    def check(self, name: str, ok: bool, detail: str = "") -> None:
        print(f"  [{'PASS' if ok else 'FAIL'}] {name}" + (f" — {detail}" if detail else ""))
        if not ok:
            self.failures.append(f"{name}: {detail}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--device", default="192.168.1.97", help="P4 设备 IP")
    ap.add_argument("--bridge", default="http://127.0.0.1:8901", help="PC 语音桥地址")
    ap.add_argument("--wav", default="", help="自定义 16k 单声道 WAV")
    ap.add_argument("--keep-draft", action="store_true", help="结束后不取消草稿")
    ap.add_argument("--skip-shot", action="store_true", help="跳过截图白屏检测")
    args = ap.parse_args()

    case = Case()
    sid = "%016x" % random.getrandbits(64)
    print(f"sid={sid}")

    with tempfile.TemporaryDirectory() as tmp:
        wav = open(args.wav, "rb").read() if args.wav else build_speech_wav(tmp)
    print(f"WAV {len(wav)} 字节")

    # 1. begin
    t0 = time.time()
    code, body = http("POST", f"{args.bridge}/voice/begin?sid={sid}", b"{}", 10)
    case.check("begin 返回 ok", code == 200 and parse_json(body).get("ok") is True,
               f"HTTP {code} {body[:80]!r} {time.time()-t0:.2f}s")

    # 2. 上传识别
    t0 = time.time()
    code, body = http("POST", f"{args.bridge}/voice?sid={sid}", wav, 120)
    asr = parse_json(body)
    cost = time.time() - t0
    case.check("识别成功且有文本", asr.get("ok") is True and asr.get("text", "").strip() != "",
               f"{cost:.2f}s {json.dumps(asr, ensure_ascii=False)[:120]}")
    case.check("识别结果为草稿", asr.get("draft") is True, str(asr.get("draft")))

    # 3. 设备进入草稿模式
    time.sleep(1.0)
    code, body = http("GET", f"http://{args.device}/status", timeout=6)
    status = parse_json(body)
    case.check("设备 voiceDraft=true", status.get("voiceDraft") is True,
               f"HTTP {code} voiceDraft={status.get('voiceDraft')}")

    # 4. 截图：草稿窗口不应白屏
    if not args.skip_shot:
        code, bmp = http("GET", f"http://{args.device}/shot", timeout=40)
        ratio = white_ratio(bmp) if code == 200 else 1.0
        case.check("草稿窗口非白屏", code == 200 and ratio < WHITE_RATIO_LIMIT,
                   f"HTTP {code} 白色占比 {ratio*100:.0f}%")

    if args.keep_draft:
        print("  [SKIP] 保留草稿（--keep-draft）")
    else:
        # 5. 取消后必须立即退出草稿模式
        code, body = http("POST", f"{args.bridge}/voice/cancel", b"{}", 8)
        case.check("cancel 生效", code == 200 and parse_json(body).get("ok") is True,
                   f"HTTP {code} {body[:80]!r}")
        time.sleep(1.0)
        code, body = http("GET", f"http://{args.device}/status", timeout=6)
        status = parse_json(body)
        case.check("取消后 voiceDraft=false", status.get("voiceDraft") is False,
                   f"voiceDraft={status.get('voiceDraft')}")
        if not args.skip_shot:
            code, bmp = http("GET", f"http://{args.device}/shot", timeout=40)
            ratio = white_ratio(bmp) if code == 200 else 1.0
            case.check("取消后非白屏", code == 200 and ratio < WHITE_RATIO_LIMIT,
                       f"HTTP {code} 白色占比 {ratio*100:.0f}%")

    print()
    if case.failures:
        print(f"FAILED ({len(case.failures)}):")
        for item in case.failures:
            print("  -", item)
        return 1
    print("ALL PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
