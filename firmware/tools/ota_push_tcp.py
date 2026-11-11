#!/usr/bin/env python3
"""Push a built app image to the P4 device over TCP 3333 via firmware/begin|chunk|commit.

Usage: python3 ota_push_tcp.py <host> <image.bin>

Safety: writes only the inactive OTA partition; the device verifies SHA-256 and
project identity, then reboots itself. Old firmware keeps running until commit.
Rollback: new image auto-confirms after sustained healthy runtime (8s/20 samples);
a bad image rolls back to the old slot on the next boot.

Hard-won notes (2026-11-10, five failed attempts distilled):
- Do NOT use socket.makefile(); a readline timeout poisons the object
  ("cannot read from timed out object"). Use a manual buffered reader.
- The device TCP session enforces a ~30s application inactivity window and
  other workloads (USB traffic from the PC app) can stall acks, so the
  sender re-sends the CURRENT chunk every 10s while waiting: the device
  answers duplicates with ok:true,duplicate:true and keeps state unchanged,
  which keeps both the TCP session and the OTA transfer alive.
- 1 KiB chunks are reliably accepted; larger chunks are not more robust.
"""
import base64
import hashlib
import json
import socket
import sys
import time

CHUNK = 512   # 设备内部 RAM 紧张时 1KB 分片会中途 reset；512B 降低峰值
KEEPALIVE_EVERY = 10.0


class Reader:
    def __init__(self, sock):
        self.s = sock
        self.buf = b""

    def readline(self, to):
        self.s.settimeout(to)
        while b"\n" not in self.buf:
            data = self.s.recv(65536)
            if not data:
                raise EOFError("device closed connection")
            self.buf += data
        line, self.buf = self.buf.split(b"\n", 1)
        return json.loads(line.decode("utf-8", "replace"))


def main():
    host, image_path = sys.argv[1], sys.argv[2]
    image = open(image_path, "rb").read()
    sha = hashlib.sha256(image).hexdigest()
    size = len(image)
    print(f"image={image_path} bytes={size} sha256={sha}")

    s = socket.create_connection((host, 3333), timeout=15)
    r = Reader(s)
    hello = r.readline(10)
    print(f"hello board={hello['payload']['boardDeviceId']} fw={hello['payload'].get('fw')}")

    def send(topic, payload):
        s.sendall((json.dumps({"topic": topic, "payload": payload}) + "\n").encode())

    def ack_of(phase, tid, to):
        t0 = time.time()
        while time.time() - t0 < to:
            try:
                m = r.readline(to)
            except socket.timeout:
                return None
            if m.get("topic") == "firmware/ack":
                p = m["payload"]
                if p.get("phase") == phase and p.get("transferId") == tid:
                    return p
        return None

    send("firmware/query", {})
    deadline = time.time() + 20
    while time.time() < deadline:
        try:
            m = r.readline(5)
        except socket.timeout:
            break
        if m.get("topic") == "firmware/status":
            if not m["payload"].get("supported"):
                raise RuntimeError("OTA not supported by running firmware")
            print("ota supported")
            break

    tid = f"ota-{int(time.time())}"
    send("firmware/begin", {"transferId": tid, "sha256": sha, "size": size})
    p = ack_of("begin", tid, 90)
    if not p or not p.get("ok"):
        raise RuntimeError(f"begin refused: {p}")
    print(f"begin ok target={p.get('targetPartition')}")

    total = (size + CHUNK - 1) // CHUNK
    t0 = time.time()
    try:
        for seq in range(total):
            block = image[seq * CHUNK:(seq + 1) * CHUNK]
            line = (json.dumps({
                "topic": "firmware/chunk",
                "payload": {
                    "transferId": tid,
                    "seq": seq,
                    "data": base64.b64encode(block).decode(),
                    "decodedSize": len(block),
                },
            }) + "\n").encode()
            s.sendall(line)
            ok = False
            tstart = time.time()
            while time.time() - tstart < 600:
                p = ack_of("chunk", tid, KEEPALIVE_EVERY)
                if p is not None and p.get("ok") and not p.get("duplicate"):
                    if p.get("seq") != seq:
                        raise RuntimeError(f"seq drift: {p.get('seq')} != {seq}")
                    ok = True
                    break
                if time.time() - tstart >= KEEPALIVE_EVERY:
                    s.sendall(line)  # duplicate keepalive; device replies ok+duplicate
            if not ok:
                raise RuntimeError(f"chunk stall at seq={seq}")
            if (seq + 1) % 400 == 0:
                rate = (seq + 1) * CHUNK / max(time.time() - t0, 0.001)
                print(f"  {seq + 1}/{total} {rate / 1024:.0f} KiB/s")
    except (BrokenPipeError, ConnectionResetError, EOFError) as e:
        raise RuntimeError(f"connection dropped mid-transfer (seq={seq}): {e!r}")

    print("all chunks ok, committing")
    send("firmware/commit", {"transferId": tid})
    p = ack_of("commit", tid, 120)
    if p and p.get("ok"):
        print(f"COMMIT OK version={p.get('version')} target={p.get('targetPartition')} "
              f"(device reboots in ~5s)")
    else:
        raise RuntimeError(f"commit refused: {p}")
    s.close()


if __name__ == "__main__":
    main()
