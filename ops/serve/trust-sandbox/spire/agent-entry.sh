#!/bin/sh
# trust-sandbox SPIRE agent entrypoint — wait for the server-minted join token,
# then run the agent with it. The agent attests with the token (node identity
# spiffe://trust.sandbox/agent) and serves the Workload API on the shared socket.
set -eu

CONF=/run/spire/config/agent.conf
TOKEN_FILE=/run/spire/shared/agent-join-token

echo "[agent-entry] waiting for join token"
until [ -s "$TOKEN_FILE" ]; do sleep 1; done
TOKEN="$(cat "$TOKEN_FILE")"
echo "[agent-entry] got token; starting agent"
exec spire-agent run -config "$CONF" -joinToken "$TOKEN"
