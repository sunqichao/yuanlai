"""Serve the local prototype and a machine-wide record store on loopback only."""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import tempfile
import threading
from datetime import datetime
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit


APP_DIRECTORY = Path(__file__).resolve().parent
MAX_REQUEST_BYTES = 64 * 1024
MAX_STORE_BYTES = 32 * 1024 * 1024
RECORD_FIELDS_V1 = {
    "protocolVersion",
    "hashAlgorithm",
    "fingerprint",
    "fileSize",
    "workTitle",
    "claimant",
    "createdAt",
    "nonce",
    "signerPublicKey",
    "signature",
    "recordId",
}
RECORD_FIELDS_V2 = RECORD_FIELDS_V1 | {"claimantRole", "declarationText"}
RECORD_FIELDS_BY_PROTOCOL = {
    "yuanlai-local-record-v1": RECORD_FIELDS_V1,
    "yuanlai-local-record-v2": RECORD_FIELDS_V2,
}


class InvalidRecord(ValueError):
    """A record does not match the supported local record shape."""


def validate_record(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise InvalidRecord("record fields do not match the supported format")
    protocol_version = value.get("protocolVersion")
    if not isinstance(protocol_version, str):
        raise InvalidRecord("record fields do not match the supported format")
    expected_fields = RECORD_FIELDS_BY_PROTOCOL.get(protocol_version)
    if expected_fields is None or set(value) != expected_fields:
        raise InvalidRecord("record fields do not match the supported format")
    if value["hashAlgorithm"] != "SHA-256":
        raise InvalidRecord("unsupported record protocol or hash algorithm")
    hex_fields = {
        "fingerprint": 64,
        "nonce": 32,
        "signerPublicKey": 64,
        "signature": 128,
        "recordId": 64,
    }
    for field, length in hex_fields.items():
        field_value = value[field]
        if not isinstance(field_value, str) or re.fullmatch(rf"[0-9a-f]{{{length}}}", field_value) is None:
            raise InvalidRecord(f"invalid {field}")
    file_size = value["fileSize"]
    if type(file_size) is not int or not 0 <= file_size <= 9_007_199_254_740_991:
        raise InvalidRecord("invalid fileSize")
    for field in ("workTitle", "claimant"):
        field_value = value[field]
        if not isinstance(field_value, str) or not field_value.strip() or len(field_value) > 100:
            raise InvalidRecord(f"invalid {field}")
    if protocol_version == "yuanlai-local-record-v2":
        claimant_role = value["claimantRole"]
        if not isinstance(claimant_role, str) or claimant_role not in {"creator", "rights-holder", "authorized-agent", "other"}:
            raise InvalidRecord("invalid claimantRole")
        declaration = value["declarationText"]
        if not isinstance(declaration, str) or not declaration.strip() or len(declaration) > 1000:
            raise InvalidRecord("invalid declarationText")
    created_at = value["createdAt"]
    if not isinstance(created_at, str) or not created_at.endswith("Z"):
        raise InvalidRecord("createdAt must be an ISO 8601 UTC timestamp")
    try:
        datetime.fromisoformat(created_at[:-1] + "+00:00")
    except ValueError as error:
        raise InvalidRecord("invalid createdAt") from error
    return value


def default_data_file() -> Path:
    if sys.platform == "darwin":
        data_directory = Path.home() / "Library" / "Application Support" / "YuanLai"
    elif os.name == "nt":
        base = Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData" / "Local"))
        data_directory = base / "YuanLai"
    else:
        base = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share"))
        data_directory = base / "yuanlai"
    return data_directory / "local-records.json"


class RecordStore:
    """Small atomic JSON store shared by browsers using this local service."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self.lock = threading.RLock()

    def _read_unlocked(self) -> list[dict[str, Any]]:
        try:
            raw = self.path.read_bytes()
        except FileNotFoundError:
            return []
        if len(raw) > MAX_STORE_BYTES:
            raise OSError("local record store exceeds the size limit")
        data = json.loads(raw.decode("utf-8"))
        if not isinstance(data, dict) or data.get("formatVersion") != 1 or not isinstance(data.get("records"), list):
            raise ValueError("local record store has an unsupported format")
        records = [validate_record(record) for record in data["records"]]
        if len({record["recordId"] for record in records}) != len(records):
            raise ValueError("local record store contains duplicate record IDs")
        return records

    def list_records(self) -> list[dict[str, Any]]:
        with self.lock:
            return self._read_unlocked()

    def upsert(self, value: Any) -> bool:
        record = validate_record(value)
        with self.lock:
            records = self._read_unlocked()
            existing = next((item for item in records if item["recordId"] == record["recordId"]), None)
            if existing is not None:
                if existing != record:
                    raise InvalidRecord("recordId already exists with different contents")
                return False
            records.append(record)
            data = json.dumps(
                {"formatVersion": 1, "records": records},
                ensure_ascii=False,
                separators=(",", ":"),
                sort_keys=True,
            ).encode("utf-8")
            if len(data) > MAX_STORE_BYTES:
                raise OSError("local record store has reached its size limit")
            self.path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            temporary_path: Path | None = None
            try:
                with tempfile.NamedTemporaryFile(
                    mode="wb",
                    prefix=".local-records-",
                    suffix=".tmp",
                    dir=self.path.parent,
                    delete=False,
                ) as temporary_file:
                    temporary_path = Path(temporary_file.name)
                    temporary_file.write(data)
                    temporary_file.flush()
                    os.fsync(temporary_file.fileno())
                os.replace(temporary_path, self.path)
            finally:
                if temporary_path is not None and temporary_path.exists():
                    temporary_path.unlink()
            return True


class LocalRequestHandler(SimpleHTTPRequestHandler):
    """Static file handler with same-origin local record endpoints."""

    def __init__(self, *args: Any, store: RecordStore, **kwargs: Any) -> None:
        self.store = store
        super().__init__(*args, **kwargs)

    def end_headers(self) -> None:
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        super().end_headers()

    def _local_request_allowed(self, require_origin: bool) -> bool:
        expected_host = f"127.0.0.1:{self.server.server_port}"
        origin = self.headers.get("Origin")
        allowed = self.headers.get("Host") == expected_host and (
            origin == f"http://{expected_host}" if require_origin else origin in (None, f"http://{expected_host}")
        )
        if not allowed:
            self._send_json(403, {"error": "local same-origin access required"})
        return allowed

    def _send_json(self, status: int, value: Any) -> None:
        body = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        parsed_path = urlsplit(self.path).path
        if parsed_path == "/api/records":
            if not self._local_request_allowed(require_origin=False):
                return
            try:
                records = self.store.list_records()
            except (OSError, ValueError, UnicodeError):
                self._send_json(500, {"error": "cannot read local record store"})
                return
            self._send_json(200, records)
            return
        if parsed_path.startswith("/api/"):
            self._send_json(404, {"error": "not found"})
            return
        self.path = parsed_path
        super().do_GET()

    def do_POST(self) -> None:
        if urlsplit(self.path).path != "/api/records":
            self._send_json(404, {"error": "not found"})
            return
        if not self._local_request_allowed(require_origin=True):
            return
        if self.headers.get("Transfer-Encoding") is not None:
            self._send_json(400, {"error": "transfer encoding is not supported"})
            return
        try:
            content_length = int(self.headers.get("Content-Length", ""))
        except ValueError:
            self._send_json(411, {"error": "content length is required"})
            return
        if not 0 < content_length <= MAX_REQUEST_BYTES:
            self._send_json(413, {"error": "record request exceeds the size limit"})
            return
        try:
            value = json.loads(self.rfile.read(content_length).decode("utf-8"))
            inserted = self.store.upsert(value)
        except (UnicodeError, json.JSONDecodeError, InvalidRecord) as error:
            self._send_json(400, {"error": str(error)})
            return
        except (OSError, ValueError):
            self._send_json(500, {"error": "cannot write local record store"})
            return
        self._send_json(200, {"recordId": value["recordId"], "inserted": inserted})


class LocalHTTPServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def main() -> None:
    parser = argparse.ArgumentParser(description="Run the YuanLai local-first browser prototype")
    parser.add_argument("--port", type=int, default=18765, help="loopback HTTP port (default: 18765)")
    parser.add_argument("--data-file", type=Path, default=default_data_file(), help=argparse.SUPPRESS)
    arguments = parser.parse_args()
    if not 0 < arguments.port <= 65535:
        parser.error("--port must be between 1 and 65535")

    store = RecordStore(arguments.data_file.expanduser())
    handler = partial(LocalRequestHandler, store=store, directory=str(APP_DIRECTORY))
    server = LocalHTTPServer(("127.0.0.1", arguments.port), handler)
    print(f"YuanLai local app: http://127.0.0.1:{server.server_port}/")
    print(f"Shared local record file: {store.path}")
    print("This service listens on this computer only. Press Ctrl+C to stop.")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nYuanLai local app stopped.")
    finally:
        server.server_close()


if __name__ == "__main__":
    main()