#!/usr/bin/env python3
"""Updater-only Docker collector. Never persist raw logs or run backend-supplied commands."""
import datetime
import json
import os
from pathlib import Path
import re
import selectors
import signal
import stat
import subprocess
import sys
import time

LIMIT = 1024 * 1024
SERVICES = {"backend", "frontend", "surrealdb", "updater", "backup", "caddy", "secrets-helper", "surreal-upgrade"}


def command(args):
    proc = subprocess.Popen(["docker", "compose", *args], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL, start_new_session=True)
    output = bytearray()
    deadline = time.monotonic() + 6
    with selectors.DefaultSelector() as selector:
        selector.register(proc.stdout, selectors.EVENT_READ)
        try:
            while time.monotonic() < deadline and len(output) < LIMIT:
                if not selector.select(.1):
                    continue
                chunk = os.read(proc.stdout.fileno(), min(65536, LIMIT - len(output)))
                if not chunk:
                    break
                output.extend(chunk)
        finally:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except PermissionError:
                # Some macOS test sandboxes prohibit group signals. The child is still ours.
                proc.kill()
            proc.wait()
            proc.stdout.close()
    return output.decode("utf-8", errors="replace")


# Shared with the backend at compile time. Only these event/target pairs have fields.
EVENTS = json.loads((Path(__file__).resolve().parents[1] / "backend/src/diagnostics/events.json").read_text())


def safe_logs(text):
    result = []
    for line in text.splitlines()[-300:]:
        # A whole structured record is required. Never search prose for embedded JSON.
        try:
            event = json.loads(line)
        except (ValueError, TypeError):
            event = {}
        if not isinstance(event, dict):
            event = {}
        safe = {"message": "<redacted>", "level": "unknown", "target": "<redacted>", "fields": {}}
        if event.get("level") in ("TRACE", "DEBUG", "INFO", "WARN", "ERROR"):
            safe["level"] = event["level"]
        stamp = event.get("timestamp")
        if isinstance(stamp, str) and re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?(?:Z|[+-]\d{2}:\d{2})", stamp):
            try:
                parsed = datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00"))
                if 2000 <= parsed.year <= 2100:
                    safe["timestamp"] = parsed.isoformat()
            except ValueError:
                pass
        name = event.get("name")
        spec = EVENTS.get(name) if isinstance(name, str) else None
        if spec and event.get("target") == spec["target"]:
            safe.update(name=name, target=spec["target"], message=spec["message"])
            fields = event.get("fields")
            if isinstance(fields, dict):
                for key, (low, high) in spec["fields"].items():
                    value = fields.get(key)
                    if type(value) is int and low <= value <= high:
                        safe["fields"][key] = value
        result.append(safe)
    return "\n".join(json.dumps(row) for row in result) + "\n"


def read_nonce(path):
    # The backend can replace entries in this directory. Open without following links,
    # and reject pipes/devices before reading, so this root process cannot be used as a reader.
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, "r") as marker:
            if not stat.S_ISREG(os.fstat(marker.fileno()).st_mode):
                return None
            return marker.read(65).strip()
    except (OSError, UnicodeError):
        return None


def collect(directory):
    root = Path(directory)
    request = root / "diagnostics-requested"
    nonce = read_nonce(request)
    if nonce is None:
        return
    if not re.fullmatch(r"[a-f0-9]{32}", nonce):
        request.unlink(missing_ok=True)
        return
    out = root / "diagnostics"
    if out.is_symlink():
        return
    out.mkdir(exist_ok=True)
    # Fixed filenames, no paths or arguments from the request.
    rows = []
    raw = command(["ps", "--all", "--format", "json"])
    try:
        decoded = json.loads(raw)
        rows = decoded if isinstance(decoded, list) else [decoded]
    except ValueError:
        for line in raw.splitlines():
            try:
                rows.append(json.loads(line))
            except ValueError:
                pass
    ps = []
    for row in rows:
        if not isinstance(row, dict) or row.get("Service") not in SERVICES:
            continue
        state = row.get("State")
        image = row.get("Image", "")
        if not isinstance(image, str):
            image = ""
        ps.append({"service": row["Service"], "state": state if state in {"running", "exited", "restarting", "created", "dead", "paused"} else "unknown",
                   "image": image if re.fullmatch(r"(?:ghcr.io/cashara-io/eunomia-[a-z-]+|surrealdb/surrealdb|docker|caddy):[a-zA-Z0-9_.-]+", image) else "unknown"})
    files = {"compose-ps.json": json.dumps(ps, indent=2)}
    for service in sorted(SERVICES):
        files[service + ".log"] = safe_logs(command(["logs", "--no-color", "--no-log-prefix", "--tail", "300", service]))
    # Docker owns the actual data volume. This command has no user-supplied arguments.
    disk = command(["exec", "-T", "surrealdb", "df", "-Pk", "/data"])
    try:
        free = int(disk.splitlines()[-1].split()[3]) * 1024
    except (ValueError, IndexError):
        free = None
    files["collection.json"] = json.dumps({"collected_at": datetime.datetime.now(datetime.timezone.utc).isoformat(), "data_volume_free_bytes": free,
        "note": "Last 300 lines per service, limited to 1 MiB input each. Only allowlisted event templates and bounded integer fields are retained. Other messages are redacted. Empty logs can mean an unavailable service or Docker command."})
    # Hold an O_NOFOLLOW directory descriptor while publishing. A backend-writable parent
    # must not let a symlink or directory rename redirect this privileged writer.
    import uuid
    fd = os.open(out, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for name, text in [*files.items(), ("ready", nonce)]:
            temp = "." + uuid.uuid4().hex
            handle = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o644, dir_fd=fd)
            with os.fdopen(handle, "w") as stream:
                stream.write(text)
            os.replace(temp, name, src_dir_fd=fd, dst_dir_fd=fd)
    finally:
        os.close(fd)
    if read_nonce(request) == nonce:
        request.unlink(missing_ok=True)


if __name__ == "__main__":
    collect(sys.argv[1])
