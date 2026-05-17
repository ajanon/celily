#!/usr/bin/python3
"""celily mitmproxy addon.

allowlist enforcement, default-deny, with per-rule request quotas
and header injection.

Connects to the celily config socket at startup. Sends the CA
certificate back over the same connection.
"""

import json
import logging
import os
import socket as sock_mod
import time

from mitmproxy import ctx, dns, http
from mitmproxy.net.dns import response_codes

logger = logging.getLogger(__name__)


class CelilyAddon:
    def __init__(self) -> None:
        self.rules_allow: list[dict] = []
        self.dns_filter: bool = False
        # Per-allow-rule quota state: (bucket_id, count) or
        # None for rules without quotas.
        self._quota_state: list[tuple[int, int] | None] = []

    def _notify_ready(self) -> None:
        """Send READY=1 to systemd via NOTIFY_SOCKET.

        No-op when NOTIFY_SOCKET is unset (e.g. running outside of
        systemd). Failures are silent: the notification is best-effort
        and systemd will time out and mark the unit as failed if the
        datagram is never delivered.
        """
        notify_socket = os.environ.get("NOTIFY_SOCKET")
        if not notify_socket:
            return
        # Abstract sockets start with @ in the environment but need
        # a leading NUL byte for sockaddr_un.
        if notify_socket.startswith("@"):
            notify_socket = "\0" + notify_socket[1:]
        try:
            s = sock_mod.socket(sock_mod.AF_UNIX, sock_mod.SOCK_DGRAM)
            s.sendto(b"READY=1", notify_socket)
            s.close()
        except OSError:
            pass

    def running(self) -> None:
        """Connect to celily config socket, receive config, send CA cert back.

        On any error during the exchange, mitmdump is shut down via
        ctx.master.shutdown().  The process exits without sending
        READY=1, so systemd (Type=notify) marks the unit as failed.
        """
        socket_path = os.environ.get("CELILY_CONFIG_SOCKET")
        if not socket_path:
            # No socket configured. mitmdump started outside of
            # celily (development / testing). Rules stay empty; all traffic
            # is default-deny (no allow rules match).
            self._notify_ready()
            return

        s = sock_mod.socket(sock_mod.AF_UNIX, sock_mod.SOCK_STREAM)
        try:
            s.connect(socket_path)
        except OSError:
            logger.exception("failed to connect to config socket %s", socket_path)
            ctx.master.shutdown()
            return

        data = b""
        while True:
            chunk = s.recv(4096)
            if not chunk:
                break
            data += chunk

        try:
            config = json.loads(data)
        except (json.JSONDecodeError, ValueError):
            logger.exception("failed to parse config from celily")
            s.close()
            ctx.master.shutdown()
            return

        self.rules_allow = config.get("allow", [])
        self.dns_filter = config.get("dns", False)

        # Initialise per-allow-rule quota state.
        self._quota_state = [
            (0, 0) if rule.get("quota") is not None else None
            for rule in self.rules_allow
        ]

        # Read the CA cert from mitmproxy's confdir. It is written
        # during proxy initialization, before running() fires.
        confdir = ctx.options.confdir
        try:
            with open(os.path.join(confdir, "mitmproxy-ca-cert.pem"), "rb") as f:
                ca_cert = f.read().decode("utf-8")
        except (OSError, UnicodeDecodeError):
            logger.exception("cannot read CA cert from %s", confdir)
            s.close()
            ctx.master.shutdown()
            return

        s.send(json.dumps({"ca_cert": ca_cert}).encode())
        s.close()
        self._notify_ready()

    def request(self, flow: http.HTTPFlow) -> None:
        host = flow.request.pretty_host
        path = flow.request.path

        # Allow rules with optional per-rule quota.
        for i, rule in enumerate(self.rules_allow):
            if host != rule["host"]:
                continue
            prefixes = rule.get("path_prefixes")
            if prefixes is not None:
                if isinstance(prefixes, str):
                    prefixes = [prefixes]
                if not any(path.startswith(p) for p in prefixes):
                    continue
            methods = rule.get("methods")
            if methods is not None and flow.request.method not in methods:
                continue

            # Quota check (if this rule has one).
            quota = rule.get("quota")
            if quota is not None:
                state = self._quota_state[i]
                assert state is not None  # ensured by running() init
                window_seconds = quota["window_seconds"]
                max_requests = quota["max_requests"]
                bucket = int(time.time() / window_seconds)
                prev_bucket, count = state
                if bucket != prev_bucket:
                    count = 0
                    self._quota_state[i] = (bucket, 0)
                if count >= max_requests:
                    flow.response = http.Response.make(
                        429,
                        b"Request quota exceeded",
                        {"Content-Type": "text/plain"},
                    )
                    return
                self._quota_state[i] = (bucket, count + 1)

            # Inject static headers first, then auth (if present).
            headers = rule.get("headers", {})
            for name, value in headers.items():
                flow.request.headers[name] = value
            auth = rule.get("auth")
            if auth is not None:
                flow.request.headers[auth["header"]] = auth["value"]
            return  # allowed

        # Default deny.
        flow.response = http.Response.make(
            403,
            b"Blocked by network policy",
            {"Content-Type": "text/plain"},
        )

    def dns_request(self, flow: dns.DNSFlow) -> None:
        # Pass through when DNS filtering is disabled (unconfigured or
        # explicitly turned off). Without this guard, empty allowlists
        # would block all DNS in development mode.
        if not self.dns_filter:
            return

        # Build allowed-host set from HTTP allow rules. Path prefixes
        # have no DNS equivalent; hostname alone is the gate.
        allowed_hosts = {rule["host"] for rule in self.rules_allow}

        # DNS queries can carry multiple questions. Block the entire
        # query if any question's name is not in the allowlist.
        for question in flow.request.questions:
            if question.name not in allowed_hosts:
                flow.response = flow.request.fail(response_codes.NXDOMAIN)
                return

        # All names are allowed; let DnsResolver handle upstream.


addons = [CelilyAddon()]
