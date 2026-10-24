"""A small Eunomia client built on the standard library."""

from __future__ import annotations

import json
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, Iterable, List, Optional, Sequence


class EunomiaError(Exception):
    """An error response from the API (problem+json), or a transport failure (status 0)."""

    def __init__(
        self,
        status: int,
        code: str,
        detail: str,
        trace_id: Optional[str] = None,
        retry_after: Optional[float] = None,
    ) -> None:
        super().__init__(f"{status} {code}: {detail}" + (f" (trace {trace_id})" if trace_id else ""))
        self.status = status
        self.code = code
        self.detail = detail
        self.trace_id = trace_id
        self.retry_after = retry_after


@dataclass
class Vault:
    id: str
    name: str
    kind: str
    created_at: Optional[str] = None
    role: Optional[str] = None  # only set by list_vaults


@dataclass
class CloneResult(Vault):
    entities_copied: int = 0


@dataclass
class MergeResult(Vault):
    entities: int = 0
    merged_from: List[str] = field(default_factory=list)


@dataclass
class Member:
    email: str
    role: str


@dataclass
class InviteResult:
    vault_id: str
    user_email: str
    role: str


@dataclass
class Invitation:
    vault_id: str
    vault_name: str
    vault_kind: str
    role: str
    created_at: Optional[str] = None


@dataclass
class WriteResult:
    """Outcome of one item in `write_memories`: `result` on success, else `error`."""

    index: int
    item: Dict[str, Any]
    result: Optional[Dict[str, Any]] = None
    error: Optional[EunomiaError] = None

    @property
    def ok(self) -> bool:
        return self.error is None


def _pick(cls: Any, data: Dict[str, Any]) -> Any:
    names = cls.__dataclass_fields__.keys()
    return cls(**{k: v for k, v in data.items() if k in names})


def _parse_retry_after(value: Optional[str]) -> Optional[float]:
    try:
        return max(0.0, float(value)) if value is not None else None
    except ValueError:
        return None


class Eunomia:
    """Client for one Eunomia server.

    `base_url` is the server root (for example `http://localhost:3000`), `token` a personal
    API token. A `429` is retried up to `max_retries` times, waiting for `Retry-After`; while
    one thread waits, every other request from this client waits too.
    """

    def __init__(
        self,
        base_url: str,
        token: str,
        *,
        timeout: float = 30.0,
        max_retries: int = 5,
        max_wait: float = 60.0,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self._token = token
        self.timeout = timeout
        self.max_retries = max_retries
        self.max_wait = max_wait
        self._sleep = sleep
        self._lock = threading.Lock()
        self._resume_at = 0.0

    # -- transport ---------------------------------------------------------

    def _wait_for_pause(self) -> None:
        while True:
            with self._lock:
                delay = self._resume_at - time.monotonic()
            if delay <= 0:
                return
            self._sleep(delay)

    def _request(self, method: str, path: str, body: Any = None) -> Any:
        url = self.base_url + path
        data = json.dumps(body).encode() if body is not None else None
        attempt = 0
        while True:
            self._wait_for_pause()
            req = urllib.request.Request(url, data=data, method=method)
            req.add_header("Authorization", f"Bearer {self._token}")
            req.add_header("Accept", "application/json")
            if data is not None:
                req.add_header("Content-Type", "application/json")
            try:
                with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                    raw = resp.read()
                    return json.loads(raw) if raw else None
            except urllib.error.HTTPError as e:
                err = self._error_from(e)
            except (urllib.error.URLError, OSError) as e:
                raise EunomiaError(0, "client.transport", str(e)) from e
            if err.status == 429 and attempt < self.max_retries:
                attempt += 1
                wait = err.retry_after if err.retry_after is not None else 2.0**attempt
                wait = min(wait, self.max_wait)
                with self._lock:
                    self._resume_at = max(self._resume_at, time.monotonic() + wait)
                continue
            raise err

    @staticmethod
    def _error_from(e: urllib.error.HTTPError) -> EunomiaError:
        retry_after = _parse_retry_after(e.headers.get("Retry-After"))
        trace = e.headers.get("X-Trace-Id")
        try:
            p = json.loads(e.read())
            e.close()
            return EunomiaError(
                e.code,
                str(p.get("code", f"http.{e.code}")),
                str(p.get("detail") or p.get("title") or e.reason),
                p.get("trace_id") or trace,
                retry_after,
            )
        except (ValueError, AttributeError):
            e.close()
            return EunomiaError(e.code, f"http.{e.code}", str(e.reason), trace, retry_after)

    @staticmethod
    def _seg(value: str) -> str:
        return urllib.parse.quote(value, safe="")

    # -- vaults ------------------------------------------------------------

    def list_vaults(self) -> List[Vault]:
        """GET /api/vaults"""
        return [_pick(Vault, v) for v in self._request("GET", "/api/vaults")["results"]]

    def get_vault(self, vault_id: str) -> Vault:
        """No single-vault route exists, so this filters `list_vaults`. Raises `vault.not_found`."""
        for v in self.list_vaults():
            if v.id == vault_id or v.name == vault_id:
                return v
        raise EunomiaError(404, "vault.not_found", "Vault not found.")

    def create_vault(self, name: str, kind: str = "org") -> Vault:
        """POST /api/vaults. `kind` is `org` or `personal`."""
        return _pick(Vault, self._request("POST", "/api/vaults", {"name": name, "kind": kind}))

    def rename_vault(self, vault_id: str, name: str) -> Vault:
        """PATCH /api/vaults/{id}"""
        return _pick(Vault, self._request("PATCH", f"/api/vaults/{self._seg(vault_id)}", {"name": name}))

    def delete_vault(self, vault_id: str) -> None:
        """DELETE /api/vaults/{id}"""
        self._request("DELETE", f"/api/vaults/{self._seg(vault_id)}")

    def clone_vault(self, vault_id: str, name: Optional[str] = None, kind: str = "org") -> CloneResult:
        """POST /api/vaults/{id}/clone"""
        body: Dict[str, Any] = {"kind": kind}
        if name is not None:
            body["name"] = name
        return _pick(CloneResult, self._request("POST", f"/api/vaults/{self._seg(vault_id)}/clone", body))

    def merge_vaults(self, vault_a: str, vault_b: str, name: Optional[str] = None, kind: str = "org") -> MergeResult:
        """POST /api/vaults/merge. Creates a new vault; the two sources are left untouched."""
        body: Dict[str, Any] = {"vault_ids": [vault_a, vault_b], "kind": kind}
        if name is not None:
            body["name"] = name
        return _pick(MergeResult, self._request("POST", "/api/vaults/merge", body))

    def list_members(self, vault_id: str) -> List[Member]:
        """GET /api/vaults/{id}/members"""
        res = self._request("GET", f"/api/vaults/{self._seg(vault_id)}/members")["results"]
        return [_pick(Member, m) for m in res]

    def invite_member(self, vault_id: str, email: str, role: str = "member") -> InviteResult:
        """POST /api/vaults/{id}/members"""
        body = {"email": email, "role": role}
        return _pick(InviteResult, self._request("POST", f"/api/vaults/{self._seg(vault_id)}/members", body))

    def remove_member(self, vault_id: str, email: str) -> None:
        """DELETE /api/vaults/{id}/members/{email}"""
        self._request("DELETE", f"/api/vaults/{self._seg(vault_id)}/members/{self._seg(email)}")

    def leave_vault(self, vault_id: str) -> None:
        """POST /api/vaults/{id}/leave"""
        self._request("POST", f"/api/vaults/{self._seg(vault_id)}/leave")

    def list_invitations(self) -> List[Invitation]:
        """GET /api/vaults/invitations"""
        return [_pick(Invitation, i) for i in self._request("GET", "/api/vaults/invitations")["results"]]

    def accept_invitation(self, vault_id: str) -> Vault:
        """POST /api/vaults/{id}/invitations/accept"""
        return _pick(Vault, self._request("POST", f"/api/vaults/{self._seg(vault_id)}/invitations/accept"))

    def decline_invitation(self, vault_id: str) -> None:
        """POST /api/vaults/{id}/invitations/decline"""
        self._request("POST", f"/api/vaults/{self._seg(vault_id)}/invitations/decline")

    # -- memory (POST /api/tools/{name}) -----------------------------------

    def call_tool(self, name: str, args: Optional[Dict[str, Any]] = None) -> Any:
        """Call any registered tool; the methods below wrap three of them."""
        return self._request("POST", f"/api/tools/{self._seg(name)}", args or {})

    def memory_write(
        self,
        subject_name: str,
        subject_kind: str,
        text: str,
        *,
        vault_id: Optional[str] = None,
        type: str = "world",
        source_record_id: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Tool `memory_write`. Returns `{"entity": ..., "memory": ...}`. `vault_id` may be a name."""
        args: Dict[str, Any] = {"subject_name": subject_name, "subject_kind": subject_kind, "text": text, "type": type}
        if vault_id is not None:
            args["vault_id"] = vault_id
        if source_record_id is not None:
            args["source_record_id"] = source_record_id
        return self.call_tool("memory_write", args)

    def recall(
        self,
        query: str,
        *,
        vault_id: Optional[str] = None,
        vault_ids: Optional[Sequence[str]] = None,
        limit: int = 20,
        types: Optional[Sequence[str]] = None,
    ) -> List[Dict[str, Any]]:
        """Tool `recall`. Returns the hits. Without a vault it searches all your vaults."""
        args: Dict[str, Any] = {"query": query, "limit": limit}
        if vault_id is not None:
            args["vault_id"] = vault_id
        if vault_ids is not None:
            args["vault_ids"] = list(vault_ids)
        if types is not None:
            args["types"] = list(types)
        return self.call_tool("recall", args)["results"]

    def entities_search(
        self,
        query: str,
        *,
        vault_id: Optional[str] = None,
        vault_ids: Optional[Sequence[str]] = None,
        kind: Optional[str] = None,
        limit: int = 50,
        offset: int = 0,
    ) -> List[Dict[str, Any]]:
        """Tool `entities_search`. Returns the matching entities."""
        args: Dict[str, Any] = {"query": query, "limit": limit, "offset": offset}
        if vault_id is not None:
            args["vault_id"] = vault_id
        if vault_ids is not None:
            args["vault_ids"] = list(vault_ids)
        if kind is not None:
            args["kind"] = kind
        return self.call_tool("entities_search", args)["results"]

    def write_memories(
        self,
        items: Iterable[Dict[str, Any]],
        *,
        vault_id: Optional[str] = None,
        concurrency: int = 4,
        on_result: Optional[Callable[[WriteResult], None]] = None,
    ) -> List[WriteResult]:
        """Write many facts with a worker pool.

        Each item is a dict with `subject_name`, `subject_kind`, `text` and optionally `type`,
        `source_record_id` and `vault_id` (which overrides the default). A `429` waits for the
        server's `Retry-After` and retries (up to `max_retries` per item); the wait pauses every
        worker. A failed item does not stop the run: you get one `WriteResult` per item, in
        input order, so check `.ok`.
        """
        items = list(items)

        def work(i: int) -> WriteResult:
            item = items[i]
            try:
                extra = {k: item[k] for k in ("type", "source_record_id") if k in item}
                res = self.memory_write(
                    item["subject_name"], item["subject_kind"], item["text"],
                    vault_id=item.get("vault_id", vault_id), **extra,
                )
                out = WriteResult(i, item, result=res)
            except EunomiaError as e:
                out = WriteResult(i, item, error=e)
            except KeyError as e:
                out = WriteResult(i, item, error=EunomiaError(0, "client.bad_item", f"missing {e}"))
            if on_result:
                on_result(out)
            return out

        with ThreadPoolExecutor(max_workers=max(1, concurrency)) as pool:
            return list(pool.map(work, range(len(items))))
