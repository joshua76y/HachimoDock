#!/usr/bin/env python3
"""HachimoDock 的按需 Kokoro 原生 worker；仅监听 loopback。"""

import argparse
import gc
import io
import json
import os
from pathlib import Path
import struct
import tarfile
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.request import urlopen
import wave

MODEL_NAME = "kokoro-multi-lang-v1_1"
MODEL_ARCHIVE_URL = (
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/"
    "kokoro-multi-lang-v1_1.tar.bz2"
)
REQUIRED_FILES = ("model.onnx", "voices.bin", "tokens.txt", "lexicon-us-en.txt", "lexicon-zh.txt")
DEVICE_SAMPLE_RATE = 16_000


def model_is_ready(model_dir: Path) -> bool:
    return model_dir.is_dir() and (model_dir / "espeak-ng-data").is_dir() and all(
        (model_dir / name).is_file() for name in REQUIRED_FILES
    )


def resample_to_device_rate(samples, sample_rate: int):
    """Kokoro v1.1 固定输出 24kHz；设备音频协议固定收 16kHz/PCM16。"""
    if sample_rate == DEVICE_SAMPLE_RATE:
        return sample_rate, list(samples)
    if sample_rate <= 0 or not samples:
        raise ValueError("Kokoro 返回的采样率或音频为空")
    source = list(samples)
    count = max(1, round(len(source) * DEVICE_SAMPLE_RATE / sample_rate))
    step = sample_rate / DEVICE_SAMPLE_RATE
    output = []
    for index in range(count):
        position = min(index * step, len(source) - 1)
        left = int(position)
        right = min(left + 1, len(source) - 1)
        fraction = position - left
        output.append(float(source[left]) * (1 - fraction) + float(source[right]) * fraction)
    return DEVICE_SAMPLE_RATE, output


def safe_extract(archive: tarfile.TarFile, destination: Path) -> None:
    destination = destination.resolve()
    for member in archive.getmembers():
        name = Path(member.name)
        if name.is_absolute() or ".." in name.parts or member.issym() or member.islnk():
            raise ValueError("模型压缩包包含不安全路径")
    for member in archive.getmembers():
        archive.extract(member, destination)


def install(model_root: Path) -> None:
    target = model_root / MODEL_NAME
    if model_is_ready(target):
        return
    if target.exists():
        raise RuntimeError(f"发现不完整的模型目录：{target}；请手动删除后重试")
    model_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".kokoro-download-", dir=model_root) as temp:
        temp_path = Path(temp)
        archive_path = temp_path / "model.tar.bz2"
        with urlopen(MODEL_ARCHIVE_URL, timeout=45) as response, archive_path.open("wb") as output:
            while True:
                block = response.read(1024 * 1024)
                if not block:
                    break
                output.write(block)
        extract_root = temp_path / "extract"
        extract_root.mkdir()
        with tarfile.open(archive_path, "r:bz2") as archive:
            safe_extract(archive, extract_root)
        extracted = extract_root / MODEL_NAME
        if not model_is_ready(extracted):
            raise RuntimeError("模型下载完成但文件不完整")
        os.replace(extracted, target)


class KokoroWorker:
    def __init__(self, model_dir: Path):
        self.model_dir = model_dir
        self.tts = None
        self.lock = threading.Lock()
        self.last_used = 0.0

    def loaded(self) -> bool:
        with self.lock:
            if self.tts is not None and time.monotonic() - self.last_used > 600:
                self.tts = None
                gc.collect()
            return self.tts is not None

    def engine(self):
        with self.lock:
            if self.tts is None:
                import sherpa_onnx
                config = sherpa_onnx.OfflineTtsConfig(
                    model=sherpa_onnx.OfflineTtsModelConfig(
                        kokoro=sherpa_onnx.OfflineTtsKokoroModelConfig(
                            model=str(self.model_dir / "model.onnx"),
                            voices=str(self.model_dir / "voices.bin"),
                            tokens=str(self.model_dir / "tokens.txt"),
                            data_dir=str(self.model_dir / "espeak-ng-data"),
                            lexicon=f"{self.model_dir / 'lexicon-us-en.txt'},{self.model_dir / 'lexicon-zh.txt'}",
                        ),
                        num_threads=1,
                        debug=False,
                    )
                )
                if not config.validate():
                    raise RuntimeError("Kokoro 模型配置无效")
                self.tts = sherpa_onnx.OfflineTts(config)
            self.last_used = time.monotonic()
            return self.tts

    def synthesize(self, text: str, sid: int, speed: float) -> bytes:
        text = text.strip()
        if not text:
            raise ValueError("没有可合成的文字")
        audio = self.engine().generate(text=text, sid=sid, speed=max(0.25, min(speed, 4.0)))
        sample_rate, samples = resample_to_device_rate(audio.samples, audio.sample_rate)
        raw = bytearray()
        for sample in samples:
            raw.extend(struct.pack("<h", max(-32768, min(32767, round(float(sample) * 32767)))))
        with io.BytesIO() as buffer:
            with wave.open(buffer, "wb") as wav:
                wav.setnchannels(1)
                wav.setsampwidth(2)
                wav.setframerate(sample_rate)
                wav.writeframes(bytes(raw))
            return buffer.getvalue()


def serve(port: int, model_dir: Path) -> None:
    if not model_is_ready(model_dir):
        raise RuntimeError("Kokoro 模型尚未安装")
    worker = KokoroWorker(model_dir)

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            return

        def json(self, code: int, data: dict) -> None:
            body = json.dumps(data, ensure_ascii=False).encode("utf-8")
            self.send_response(code)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if self.path == "/health":
                self.json(200, {"ok": True, "loaded": worker.loaded()})
                return
            self.json(404, {"ok": False, "error": "not found"})

        def do_POST(self):
            if self.path == "/shutdown":
                self.json(200, {"ok": True})
                threading.Thread(target=self.server.shutdown, daemon=True).start()
                return
            if self.path != "/tts":
                self.json(404, {"ok": False, "error": "not found"})
                return
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if length <= 0 or length > 65536:
                    raise ValueError("请求内容长度无效")
                payload = json.loads(self.rfile.read(length).decode("utf-8"))
                wav = worker.synthesize(str(payload.get("text", "")), int(payload.get("sid", 3)), float(payload.get("speed", 1.0)))
                self.send_response(200)
                self.send_header("Content-Type", "audio/wav")
                self.send_header("Content-Length", str(len(wav)))
                self.end_headers()
                self.wfile.write(wav)
            except Exception as error:  # UI must receive actionable failures.
                self.json(500, {"ok": False, "error": str(error)})

    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--install", action="store_true")
    parser.add_argument("--serve", action="store_true")
    parser.add_argument("--model-root", required=True)
    parser.add_argument("--port", type=int, default=8891)
    args = parser.parse_args()
    root = Path(args.model_root).expanduser().resolve()
    if args.install:
        install(root)
        return
    if args.serve:
        serve(args.port, root / MODEL_NAME)
        return
    parser.error("choose --install or --serve")


if __name__ == "__main__":
    main()
