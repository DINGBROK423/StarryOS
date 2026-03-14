#!/usr/bin/env python3

import argparse
import socket
import subprocess
import sys
import threading
import time


def wait_for_prompt(sock: socket.socket, timeout: float = 20.0):
    prompt = "starry:~#"
    data = ""
    deadline = time.time() + timeout
    while time.time() < deadline:
        chunk = sock.recv(4096).decode("utf-8", errors="ignore")
        if not chunk:
            break
        print(chunk, end="")
        data += chunk
        if prompt in data:
            return data
    raise RuntimeError("Timed out waiting for shell prompt")


def run_cmd(sock: socket.socket, cmd: str, timeout: float = 20.0) -> str:
    marker = "__OND_DEMAND_DONE__"
    full_cmd = f"{cmd}; echo {marker}\r\n"
    sock.sendall(full_cmd.encode())

    output = ""
    deadline = time.time() + timeout
    while time.time() < deadline:
        chunk = sock.recv(4096).decode("utf-8", errors="ignore")
        if not chunk:
            break
        print(chunk, end="")
        output += chunk
        if marker in output:
            return output
    raise RuntimeError(f"Timed out running command: {cmd}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--arch", default="riscv64")
    parser.add_argument("--port", default="4444")
    args = parser.parse_args()

    qemu = subprocess.Popen(
        [
            "make",
            f"ARCH={args.arch}",
            "ACCEL=n",
            "justrun",
            f"QEMU_ARGS=-monitor none -serial tcp::{args.port},server=on",
        ],
        stderr=subprocess.PIPE,
        text=True,
    )

    ready = threading.Event()

    def worker():
        for line in qemu.stderr:
            print(line, file=sys.stderr, end="")
            if "QEMU waiting for connection" in line:
                ready.set()
        ready.set()

    t = threading.Thread(target=worker, daemon=True)
    t.start()

    sock = None
    try:
        if not ready.wait(timeout=15):
            raise RuntimeError("QEMU did not start in time")
        if qemu.poll() is not None:
            raise RuntimeError("QEMU exited early")

        sock = socket.create_connection(("localhost", int(args.port)), timeout=10)
        wait_for_prompt(sock)

        # 1) Before first /proc access, procfs should not be loaded.
        out = run_cmd(sock, "dmesg | grep \"\\[ondemand\\] loading module 'procfs'\" >/dev/null")
        if "not found" in out.lower():
            pass

        # 2) Trigger first access.
        out = run_cmd(sock, "cat /proc/meminfo >/dev/null; echo RC:$?")
        if "RC:0" not in out:
            raise RuntimeError("cat /proc/meminfo failed, procfs did not load")

        # 3) Verify load log exists.
        out = run_cmd(sock, "dmesg | grep \"\\[ondemand\\] loading module 'procfs'\"")
        if "loading module 'procfs'" not in out:
            raise RuntimeError("No procfs loading log found")

        # 4) Wait and verify unload happened.
        out = run_cmd(sock, "sleep 7; dmesg | grep \"\\[ondemand\\] unload handle\"")
        if "unload handle" not in out:
            raise RuntimeError("No unload log found (idle unload may not be working)")

        print("\n\x1b[32m✔ On-demand procfs load/unload test passed\x1b[0m")
        run_cmd(sock, "exit")
    finally:
        if sock is not None:
            try:
                sock.close()
            except Exception:
                pass
        try:
            qemu.wait(2)
        except subprocess.TimeoutExpired:
            qemu.terminate()
            qemu.wait()


if __name__ == "__main__":
    main()
