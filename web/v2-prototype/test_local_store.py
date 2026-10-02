"""Focused tests for the local shared-record HTTP service."""

import json
import tempfile
import threading
import unittest
from functools import partial
from http.server import ThreadingHTTPServer
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from run_local import APP_DIRECTORY, LocalRequestHandler, RecordStore


def sample_record(record_id: str = "a" * 64) -> dict[str, object]:
    return {
        "protocolVersion": "yuanlai-local-record-v1",
        "hashAlgorithm": "SHA-256",
        "fingerprint": "b" * 64,
        "fileSize": 123,
        "workTitle": "Shared browser test",
        "claimant": "Local tester",
        "createdAt": "2026-10-02T12:00:00.000Z",
        "nonce": "c" * 32,
        "signerPublicKey": "d" * 64,
        "signature": "e" * 128,
        "recordId": record_id,
    }


def sample_record_v2(record_id: str = "f" * 64) -> dict[str, object]:
    record = sample_record(record_id)
    record.update(
        protocolVersion="yuanlai-local-record-v2",
        claimantRole="creator",
        declarationText="I declare that I created this work.",
    )
    return record


class RecordStoreTests(unittest.TestCase):
    def test_upsert_is_idempotent_and_persists(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store_path = Path(directory) / "records.json"
            store = RecordStore(store_path)
            record = sample_record()

            self.assertTrue(store.upsert(record))
            self.assertFalse(store.upsert(record))
            self.assertEqual(RecordStore(store_path).list_records(), [record])

    def test_v2_record_is_persisted_with_declaration_fields(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store_path = Path(directory) / "records.json"
            record = sample_record_v2()
            store = RecordStore(store_path)

            self.assertTrue(store.upsert(record))
            self.assertEqual(RecordStore(store_path).list_records(), [record])

    def test_v2_record_rejects_invalid_declaration_fields(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = RecordStore(Path(directory) / "records.json")
            invalid = sample_record_v2()
            invalid["claimantRole"] = "verified-author"

            with self.assertRaises(ValueError):
                store.upsert(invalid)

    def test_malformed_protocol_and_role_types_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = RecordStore(Path(directory) / "records.json")
            invalid_protocol = sample_record()
            invalid_protocol["protocolVersion"] = []
            invalid_role = sample_record_v2()
            invalid_role["claimantRole"] = []

            for invalid in (invalid_protocol, invalid_role):
                with self.subTest(record=invalid):
                    with self.assertRaises(ValueError):
                        store.upsert(invalid)

    def test_conflicting_duplicate_id_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store = RecordStore(Path(directory) / "records.json")
            store.upsert(sample_record())
            conflicting = sample_record()
            conflicting["claimant"] = "Different signer"

            with self.assertRaises(ValueError):
                store.upsert(conflicting)

    def test_invalid_record_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            invalid = sample_record()
            invalid["fingerprint"] = "not-a-hash"

            with self.assertRaises(ValueError):
                RecordStore(Path(directory) / "records.json").upsert(invalid)

    def test_corrupt_store_is_not_silently_overwritten(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            store_path = Path(directory) / "records.json"
            store_path.write_text("not json", encoding="utf-8")

            with self.assertRaises(json.JSONDecodeError):
                RecordStore(store_path).upsert(sample_record())
            self.assertEqual(store_path.read_text(encoding="utf-8"), "not json")


class SharedRecordHTTPTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory()
        store = RecordStore(Path(self.temporary_directory.name) / "records.json")
        handler = partial(LocalRequestHandler, store=store, directory=str(APP_DIRECTORY))
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.origin = f"http://127.0.0.1:{self.server.server_port}"

    def tearDown(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
        self.temporary_directory.cleanup()

    def test_two_clients_read_the_same_saved_record(self) -> None:
        record = sample_record()
        request = Request(
            f"{self.origin}/api/records",
            data=json.dumps(record).encode("utf-8"),
            headers={"Content-Type": "application/json", "Origin": self.origin},
            method="POST",
        )
        with urlopen(request) as response:
            self.assertEqual(response.status, 200)

        for _client in range(2):
            with urlopen(f"{self.origin}/api/records") as response:
                self.assertEqual(json.load(response), [record])

    def test_cross_origin_write_is_rejected(self) -> None:
        request = Request(
            f"{self.origin}/api/records",
            data=json.dumps(sample_record()).encode("utf-8"),
            headers={"Content-Type": "application/json", "Origin": "http://example.invalid"},
            method="POST",
        )
        with self.assertRaises(HTTPError) as error:
            urlopen(request)
        self.assertEqual(error.exception.code, 403)


if __name__ == "__main__":
    unittest.main()