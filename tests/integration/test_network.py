"""Network isolation, seen from both ends: curl in the instance, and a
server on the host recording what the proxy let through.

Plain HTTP only for now: the proxy trusts the web PKI only upstream, so
an HTTPS server of our own would need a certificate it does not accept.
"""

import shutil
from pathlib import Path

import pytest

from servers import BODY

STUBS = Path(__file__).parent / "stubs"

# The proxy's answer to a request no rule allows.
BLOCKED = "Blocked by network policy\n"

# The item stubs/rbw.sh knows, and its made-up secret: keep them in sync.
SECRET_NAME = "foo"
SECRET = "celily-it-secret-5c2e9a"

# CONNECT through the proxy, then send what is neither TLS nor HTTP (an
# SSH banner). Prints the proxy's answer, then whatever comes back until
# the tunnel closes.
RAW_TUNNEL = r"""
proxy=${HTTPS_PROXY#http://}
exec 3<>"/dev/tcp/${proxy%:*}/${proxy##*:}"
printf 'CONNECT %s HTTP/1.1\r\nHost: %s\r\n\r\n' "$1" "$1" >&3
IFS= read -r -t 10 line <&3
printf '%s\n' "${line%$'\r'}"
printf 'SSH-2.0-celily-it\r\n' >&3
timeout 10 cat <&3
"""


def http_rule(server, **keys):
    return {"type": "http", "host": server.host, "ports": [server.port], **keys}


def curl(url):
    """argv: GET `url` with curl, through the proxy celily set in the
    environment. Prints the body, then the status on a line of its own."""
    return [
        "curl",
        "--silent",
        "--show-error",
        "--max-time",
        "30",
        "--write-out",
        "\n%{http_code}",
        url,
    ]


def response(result):
    """(status, body) from the output of curl()."""
    assert result.returncode == 0, "curl failed, or celily did"
    body, _, status = result.stdout.rpartition("\n")
    return int(status), body


@pytest.mark.parametrize(
    "rule_host", [None, "127.0.0.2"], ids=["no-rule", "other-host"]
)
def test_unlisted_host_is_blocked(celily, http_server, rule_host):
    if rule_host:
        # Everything but the host matches the request.
        rule = http_rule(http_server) | {"host": rule_host}
        celily.fragment = {"network": {"allow": [rule]}}

    result = celily.run("--", *curl(http_server.url("/blocked")))

    # The proxy's own answer: a client going around it would leave the
    # server untouched too.
    assert response(result) == (403, BLOCKED)
    assert http_server.connections == []


def test_listed_host_is_reached(celily, http_server):
    celily.fragment = {"network": {"allow": [http_rule(http_server)]}}

    result = celily.run("--", *curl(http_server.url("/allowed")))

    assert response(result) == (200, BODY)
    assert [(r.method, r.path) for r in http_server.requests] == [("GET", "/allowed")]


@pytest.fixture
def rbw_stub(celily):
    shutil.copy(STUBS / "rbw.sh", celily.bin / "rbw")


def test_auth_secret_is_injected(celily, http_server, rbw_stub):
    auth = {"secret": SECRET_NAME, "header": "Authorization", "prefix": "Bearer "}
    celily.fragment = {
        "secret_provider": "rbw",
        "network": {"allow": [http_rule(http_server, auth=auth)]},
    }

    # The instance's environment goes to stderr, with celily's log.
    result = celily.run(
        "--", "sh", "-c", 'env >&2 && exec "$@"', "sh", *curl(http_server.url("/"))
    )

    assert response(result) == (200, BODY)
    [request] = http_server.requests
    assert request.headers.get_all("Authorization") == [f"Bearer {SECRET}"]
    # Only the proxy has it: not the instance, not celily's log.
    assert "http_proxy=" in result.stderr, "the environment was not printed"
    assert SECRET not in result.stderr


def test_raw_tunnel_is_closed(celily, http_server):
    """Regression: the proxy used to forward a CONNECT tunnel carrying
    neither TLS nor HTTP to its target, any host and port (SSH out)."""
    # The target is allowed (an https rule: HTTPS is what CONNECT carries),
    # and the tunnel is closed anyway. With the target in no rule, a fix
    # that only checked the rules would pass too.
    rule = http_rule(http_server) | {"type": "https"}
    celily.fragment = {"network": {"allow": [rule]}}
    target = f"{http_server.host}:{http_server.port}"

    result = celily.run("--", "bash", "-c", RAW_TUNNEL, "bash", target)

    # It reached the proxy, which answered the CONNECT.
    assert result.stdout.startswith("HTTP/1."), "no answer to the CONNECT"
    assert http_server.connections == []
